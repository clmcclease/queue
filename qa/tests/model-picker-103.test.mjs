// #103 / N7 (D-044): which machines get a recommended local model, the note pointing others to
// the API or Off, the test-based quality lines, and the banner's caveat. Real React DOM, mocked IPC.
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
const notes = () => [...container.querySelectorAll('[data-role="model-quality"]')].map((n) => n.textContent);

test("#103: a recommended computer tags the 4B and shows no note", async () => {
  await mount({ model_status: () => status({}, {}, { suggested: FOUR }) });
  await openSettings();
  const tags = [...container.querySelectorAll('[data-role="model-suggested"]')];
  assert.equal(tags.length, 1);
  assert.equal(tags[0].textContent, "Recommended for this computer.");
  assert.ok(tags[0].closest("label").querySelector(`input[value="${FOUR}"]`), "on the 4B");
  assert.equal(el("model-note"), null);
});

test("#103: with no recommendation there's no tag, and a plain note points to the API or Off", async () => {
  await mount({ model_status: () => status({}, {}, { suggested: null }) });
  await openSettings();
  assert.equal(container.querySelector('[data-role="model-suggested"]'), null);
  const note = el("model-note");
  assert.equal(note.textContent, "Local naming may be slow on this computer. The API or Off choices above may suit it better.");
  assert.equal(note.getAttribute("role"), null, "not an alert");
  const manager = el("model-manager");
  assert.ok(note.compareDocumentPosition(manager.querySelector('[role="radiogroup"]')) & 4, "above the list");
});

test("#103: each model has one line from Queue's tests, never 'on this computer'", async () => {
  await mount({ model_status: () => status({}, {}, { suggested: FOUR }) });
  await openSettings();
  const lines = notes();
  assert.equal(lines.length, 2);
  assert.match(lines[0], /about 2 in 3 titles.*about 0\.7 s each on one Apple-silicon Mac/);
  assert.match(lines[1], /much less accurate.*about 1 in 3 titles.*about 0\.45 s each/);
  for (const line of lines) {
    assert.match(line, /Queue's tests|same tests/);
    assert.doesNotMatch(line, /on this computer|%/);
  }
});

test("#103: the banner's offer adds the caveat only with no recommendation, after the consent sentence", async () => {
  await mount({ model_status: () => status({}, {}, { suggested: null }) });
  const offer = text();
  assert.match(offer, /^AI naming uses a local model that isn't downloaded yet \(Qwen3 4B Instruct, 2\.5 GB from huggingface\.co\)\. Names come from the rules for now\. It may be slow on this computer; API or Off in Settings may suit it better\.$/);
  assert.ok(el("model-download"), "still offers the download");
});

test("#103: no caveat with a recommendation, nor on paused, failed or unverified downloads", async () => {
  await mount({ model_status: () => status({}, {}, { suggested: FOUR }) });
  assert.doesNotMatch(text(), /may be slow/);
  for (const big of [{ state: "partial", received: 100 }, { error: "The download failed." }, { state: "unverified" }]) {
    await act(async () => root.unmount());
    root = undefined;
    container.remove();
    clearMocks();
    await mount({ model_status: () => status(big, {}, { suggested: null }) });
    assert.doesNotMatch(text() ?? "", /may be slow/, JSON.stringify(big));
  }
});
