// Independent #103 / N7 QA: saved choices, consent, provider boundaries and status races.
// Uses the real App and React DOM with mocked IPC; no real model or user settings are touched.
import assert from "node:assert/strict";
import { after, afterEach, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { JSDOM } from "jsdom";
import React, { act } from "react";
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import { createServer } from "vite";

const dom = new JSDOM("<!doctype html><html><body></body></html>", { url: "http://localhost/" });
globalThis.window = dom.window;
globalThis.document = dom.window.document;
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const { createRoot } = await import("react-dom/client");
let server, App, root, container;

before(async () => {
  server = await createServer({
    root: fileURLToPath(new URL("../../", import.meta.url)),
    configFile: false, logLevel: "silent", appType: "custom",
    server: { middlewareMode: true, hmr: false, watch: null },
  });
  ({ default: App } = await server.ssrLoadModule("/src/App.tsx"));
});

afterEach(async () => {
  if (root) await act(async () => root.unmount());
  root = undefined;
  container?.remove();
  clearMocks();
});

after(async () => {
  await server?.close();
  dom.window.close();
  delete globalThis.window;
  delete globalThis.document;
  delete globalThis.IS_REACT_ACT_ENVIRONMENT;
});

const BIG = 2_497_280_736;
const SMALL = 1_396_198_496;
const model = (id, extra = {}) => ({
  id,
  name: id === "qwen3_5_2b" ? "Qwen3.5 2B" : "Qwen3 4B Instruct",
  file: id === "qwen3_5_2b" ? "Qwen_Qwen3.5-2B-Q4_K_M.gguf" : "Qwen_Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
  source: "huggingface.co/bartowski/x",
  size: id === "qwen3_5_2b" ? SMALL : BIG,
  state: "missing",
  downloading: false,
  verifying: false,
  error: null,
  ...extra,
});
// Each snapshot is newer than the last, as the backend's revisions are.
let revs = 0;
const status = (big = {}, small = {}, extra = {}) => ({
  rev: ++revs,
  chosen: "qwen3_4b_instruct_2507",
  picked: false,
  suggested: "qwen3_4b_instruct_2507",
  dir: "/Users/me/Library/Application Support/dev.mcclease.queue/models",
  models: [model("qwen3_4b_instruct_2507", big), model("qwen3_5_2b", small)],
  ...extra,
});
const LOCAL_MISSING = { provider: "local", unavailable: "model_missing" };

async function mount(overrides = {}) {
  const calls = [];
  mockIPC((command, args) => {
    calls.push([command, args]);
    if (command in overrides) return overrides[command](args);
    switch (command) {
      case "queue_runner_state": return false;
      case "settings_download_folder": return { path: "/Users/me/Downloads", error: null };
      case "settings_format": return "default";
      case "queue_storage_problem": return null;
      case "queue_list": return [];
      case "settings_naming_template": return "title";
      case "naming_previews": return [];
      case "settings_naming_provider": return "local";
      case "naming_status": return LOCAL_MISSING;
      case "model_status": return status();
      default: return null;
    }
  }, { shouldMockEvents: true });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(App)));
  return calls;
}

const el = (role) => container.querySelector(`[data-role="${role}"]`);
const text = () => el("model-banner-text")?.textContent;
const click = (role) => act(async () => el(role).click());
const openSettings = () => act(async () => container.querySelector('button[aria-label="Settings"]').click());


const FOUR = "qwen3_4b_instruct_2507";
const TWO = "qwen3_5_2b";
const radio = (id) => container.querySelector(`input[name="local-model"][value="${id}"]`);
const mutations = (calls) => calls.filter(([c]) => /^(?:settings_set_|model_download|model_delete|model_cancel)/.test(c));

for (const suggested of [FOUR, null]) {
  test(`#103 QA: saved 2B wins with recommendation ${suggested}; downloading still needs a click`, async () => {
    const snapshot = () => status({}, {}, { chosen: TWO, picked: true, suggested });
    const calls = await mount({ model_status: snapshot, model_download: snapshot });
    assert.match(text(), /Qwen3\.5 2B, 1\.4 GB from huggingface\.co/);
    assert.equal(/may be slow/.test(text()), suggested === null);
    await openSettings();
    assert.equal(radio(TWO).checked, true);
    assert.equal(radio(FOUR).checked, false);
    assert.equal(Boolean(el("model-note")), suggested === null);
    const recommended = el("model-suggested");
    assert.equal(Boolean(recommended), suggested !== null);
    if (recommended) assert.equal(recommended.closest("label").querySelector("input").value, FOUR);
    assert.deepEqual(mutations(calls), [], "opening the picker never saves, switches or downloads");
    await click("model-download");
    assert.deepEqual(mutations(calls), [["model_download", { model: TWO }]]);
  });
}

test("#103 QA: no recommendation keeps the note even with a ready saved model", async () => {
  const calls = await mount({
    model_status: () => status({}, { state: "ready" }, { chosen: TWO, picked: true, suggested: null }),
    naming_status: () => ({ provider: "local", unavailable: null }),
  });
  assert.equal(el("naming-banner"), null);
  await openSettings();
  assert.equal(radio(TWO).checked, true);
  assert.ok(el("model-note"));
  assert.equal(el("model-state").textContent, "Downloaded.");
  assert.equal(el("model-download"), null);
  assert.deepEqual(mutations(calls), []);
});

test("#103 QA: a ready 2B does not replace an unsaved default 4B in the picker", async () => {
  const calls = await mount({ model_status: () => status({}, { state: "ready" }, { suggested: null }) });
  assert.match(text(), /Qwen3 4B Instruct, 2\.5 GB/);
  await openSettings();
  assert.equal(radio(FOUR).checked, true);
  assert.equal(radio(TWO).checked, false);
  assert.equal(el("model-state").textContent, "Not downloaded.");
  assert.deepEqual(mutations(calls), []);
});

test("#103 QA: a newer no-recommendation event preserves selection against a late startup reply", async () => {
  let finishStartup;
  const old = status({}, {}, { suggested: FOUR });
  const pending = new Promise((resolve) => { finishStartup = resolve; });
  const calls = await mount({ model_status: () => pending });
  await openSettings();
  assert.equal(el("model-note"), null, "an unknown status is not a weak computer");
  const next = status({}, {}, { chosen: TWO, picked: true, suggested: null });
  await act(async () => emit("queue://model", next));
  await act(async () => finishStartup(old));
  assert.equal(radio(TWO).checked, true);
  assert.ok(el("model-note"));
  assert.equal(el("model-suggested"), null);
  assert.deepEqual(mutations(calls), []);
});

for (const provider of ["api", "off"]) {
  test(`#103 QA: ${provider} hides local recommendations without changing the saved model`, async () => {
    const calls = await mount({
      model_status: () => status({}, {}, { chosen: TWO, picked: true, suggested: null }),
      settings_naming_provider: () => provider,
      naming_status: () => ({ provider, unavailable: provider === "api" ? "not_set_up" : null }),
    });
    await openSettings();
    assert.equal(container.querySelector(`input[name="provider"][value="${provider}"]`).checked, true);
    assert.equal(el("model-manager"), null);
    assert.equal(el("model-note"), null);
    assert.equal(el("model-quality"), null);
    assert.deepEqual(mutations(calls), []);
  });
}

test("#103 QA: a switch keeps the weak-machine note and downloads neither model", async () => {
  const calls = await mount({
    model_status: () => status({}, {}, { suggested: null }),
    settings_set_local_model: ({ model }) => status({}, {}, { chosen: model, picked: true, suggested: null }),
  });
  await openSettings();
  await act(async () => radio(TWO).click());
  assert.equal(radio(TWO).checked, true);
  assert.ok(el("model-note"));
  assert.equal(el("model-suggested"), null);
  assert.equal(container.querySelectorAll('input[name="local-model"]').length, 2, "only the pinned 4B and 2B are offered");
  assert.deepEqual(mutations(calls), [["settings_set_local_model", { model: TWO }]]);
});
