// #31 / N7: the local model's banner and its manager in Settings → AI naming → Local. Real React
// DOM, mocked Tauri IPC and events. Then npm run build && npm test.
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

test("N7: the banner offers the download with its size and source, and nothing downloads without a click", async () => {
  const calls = await mount();
  assert.equal(
    text(),
    "AI naming uses a local model that isn't downloaded yet (Qwen3 4B Instruct, 2.5 GB from huggingface.co). Names come from the rules for now.",
  );
  assert.ok(el("model-download"));
  assert.ok(el("model-settings"));
  assert.ok(!calls.some(([c]) => c === "model_download"), "consent is a click");
});

test("N7: Download starts the chosen model; progress, Cancel, pause and Resume follow the events", async () => {
  const calls = await mount({
    model_download: ({ model }) => status({ downloading: true, state: "partial", received: 0 }, {}, { chosen: model }),
    model_cancel: () => status({ state: "partial", received: BIG * 0.43 }),
  });
  await click("model-download");
  assert.deepEqual(calls.find(([c]) => c === "model_download")[1], { model: "qwen3_4b_instruct_2507" });
  await act(async () => emit("queue://model", status({ downloading: true, state: "partial", received: 1_100_000_000 })));
  assert.equal(text(), "Downloading the local model… 44% (1.1 GB of 2.5 GB)");
  await click("model-cancel");
  assert.ok(calls.some(([c]) => c === "model_cancel"));
  assert.equal(text(), "The model download is paused at 43%.");
  assert.ok(el("model-download"));
  assert.equal(el("model-download").textContent, "Resume");
});

test("N7: checking, a failure, a downloaded model and a stray file each say what's going on", async () => {
  await mount();
  await act(async () => emit("queue://model", status({ downloading: true, verifying: true, state: "partial", received: BIG })));
  assert.equal(text(), "Checking the download…");
  await act(async () => emit("queue://model", status({ error: "The downloaded file didn't match its checksum, so it was deleted. Try again." })));
  assert.match(text(), /checksum, so it was deleted/);
  assert.equal(el("model-download").textContent, "Try again");
  await act(async () => emit("queue://model", status({ state: "unverified" })));
  assert.match(text(), /Queue didn't download it/);
  assert.equal(el("model-download"), null, "never offered over someone's file");
  await act(async () => emit("queue://model", status({ state: "ready" })));
  assert.equal(text(), "The local model is downloaded, but this copy of Queue is missing the part that runs it, so names come from the rules. Reinstalling Queue fixes this.");
});

test("N7: a model event refreshes the naming status, and a finished download keeps the honest banner", async () => {
  let ready = false;
  await mount({
    naming_status: () => (ready ? { provider: "local", unavailable: "runtime_missing" } : LOCAL_MISSING),
  });
  ready = true;
  await act(async () => emit("queue://model", status({ state: "ready" })));
  assert.match(text(), /downloaded, but this copy of Queue is missing the part that runs it/);
});

test("N7: Settings shows the models, the suggestion, the path and the source, and switches on a click", async () => {
  const calls = await mount({
    model_status: () => status({}, {}, { suggested: "qwen3_5_2b" }),
    settings_set_local_model: ({ model }) => status({}, {}, { chosen: model, picked: true, suggested: "qwen3_5_2b" }),
  });
  await openSettings();
  assert.ok(el("model-manager"));
  assert.match(el("model-suggested").textContent, /no GPU, or under 8 GB/);
  assert.equal(
    el("model-path").textContent,
    "/Users/me/Library/Application Support/dev.mcclease.queue/models/Qwen_Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
  );
  assert.equal(el("model-source").textContent, "huggingface.co/bartowski/x");
  const small = container.querySelector('input[name="local-model"][value="qwen3_5_2b"]');
  await act(async () => small.click());
  assert.deepEqual(calls.find(([c]) => c === "settings_set_local_model")[1], { model: "qwen3_5_2b" });
  assert.match(el("model-path").textContent, /Qwen_Qwen3\.5-2B-Q4_K_M\.gguf$/);
});

test("N7: Delete asks once more, and Keep leaves the model alone", async () => {
  const calls = await mount({
    model_status: () => status({ state: "ready" }),
    naming_status: () => ({ provider: "local", unavailable: "runtime_missing" }),
    model_delete: () => status(),
  });
  await openSettings();
  await click("model-delete");
  assert.ok(el("model-delete-confirm"));
  await click("model-delete-keep");
  assert.ok(!calls.some(([c]) => c === "model_delete"));
  await click("model-delete");
  await click("model-delete-confirm");
  assert.deepEqual(calls.find(([c]) => c === "model_delete")[1], { model: "qwen3_4b_instruct_2507" });
  assert.equal(el("model-state").textContent, "Not downloaded.");
});

test("N7: a refused download shows the backend's reason literally", async () => {
  await mount({ model_download: () => Promise.reject("One model downloads at a time. <b>x</b>") });
  await click("model-download");
  assert.equal(el("model-error").textContent, "One model downloads at a time. <b>x</b>");
  assert.equal(el("model-error").children.length, 0);
});

test("N7 QA #31: a late startup model snapshot cannot replace a newer event", async () => {
  let resolve;
  const initial = new Promise((r) => { resolve = r; });
  // The backend takes the startup snapshot first, so it has the older revision.
  const startup = status();
  await mount({ model_status: () => initial });
  await act(async () => emit("queue://model", status({ state: "partial", received: BIG / 2 })));
  await act(async () => resolve(startup));
  assert.equal(text(), "The model download is paused at 50%.");
});

test("N7 QA #31: Windows locations use Windows separators and unverified files require deletion", async () => {
  const calls = await mount({
    model_status: () => status({ state: "unverified" }, {}, { dir: "C:\\Users\\Zoë 日本語\\AppData\\Local\\Queue\\models" }),
  });
  await openSettings();
  assert.equal(el("model-path").textContent, "C:\\Users\\Zoë 日本語\\AppData\\Local\\Queue\\models\\Qwen_Qwen3-4B-Instruct-2507-Q4_K_M.gguf");
  assert.equal(el("model-download"), null);
  assert.ok(el("model-delete"));
  assert.ok(!calls.some(([command]) => command === "model_download" || command === "model_delete"));
});

test("N7 QA #31: a failed model switch keeps the saved choice and permits another attempt", async () => {
  let fail = true;
  await mount({
    settings_set_local_model: ({ model }) => fail
      ? Promise.reject("Couldn't save your settings: disk full <b>literal</b>")
      : status({}, {}, { chosen: model, picked: true }),
  });
  await openSettings();
  const small = () => container.querySelector('input[name="local-model"][value="qwen3_5_2b"]');
  await act(async () => small().click());
  assert.equal(small().checked, false);
  assert.equal(el("model-error").children.length, 0);
  assert.match(el("model-error").textContent, /disk full <b>literal<\/b>/);
  fail = false;
  await act(async () => small().click());
  assert.equal(small().checked, true);
  assert.equal(el("model-error"), null);
});

test("N7 QA #31: a delayed Cancel reply must not undo the finished model event", async () => {
  let resolve;
  const cancelReply = new Promise((r) => { resolve = r; });
  await mount({
    model_status: () => status({ downloading: true, state: "partial", received: BIG - 1 }),
    model_cancel: () => cancelReply,
  });
  await click("model-cancel");
  // model_cancel took this snapshot while its flag was set, before the worker finished.
  const cancelled = status({ downloading: true, state: "partial", received: BIG - 1 });
  await act(async () => emit("queue://model", status({ state: "ready" })));
  assert.match(text(), /local model is downloaded/);
  await act(async () => resolve(cancelled));
  assert.match(text(), /local model is downloaded/, "the worker's newer event must win over the command snapshot");
  assert.equal(el("model-cancel"), null);
});

test("N7 review: a switch survives an older download event that arrives while it's saving", async () => {
  const downloading = { downloading: true, state: "partial", received: 1000 };
  await mount({
    model_status: () => status(downloading),
    settings_set_local_model: async ({ model }) => {
      // A progress event with the old choice lands while the save is on its way...
      await emit("queue://model", status(downloading));
      // ...then the backend's own event after saving (it always sends one).
      await emit("queue://model", status(downloading, {}, { chosen: model, picked: true }));
      return status(downloading, {}, { chosen: model, picked: true });
    },
  });
  await openSettings();
  const small = () => container.querySelector('input[name="local-model"][value="qwen3_5_2b"]');
  await act(async () => small().click());
  assert.equal(small().checked, true, "the new choice stays");
  assert.match(el("model-path").textContent, /Qwen_Qwen3\.5-2B-Q4_K_M\.gguf$/);
});

// The real event plugin is out of the way here: the handler sees each listen call itself.
async function mountRaw(handler) {
  const calls = [];
  mockIPC((command, args) => {
    calls.push([command, args]);
    switch (command) {
      case "queue_runner_state": return false;
      case "settings_download_folder": return { path: "/Users/me/Downloads", error: null };
      case "settings_format": return "default";
      case "queue_storage_problem": return null;
      case "queue_list": return [];
      case "naming_status": return LOCAL_MISSING;
      case "plugin:event|unlisten": return null;
      default: return handler(command, args);
    }
  });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(App)));
  return calls;
}
const modelListen = ([c, a]) => c === "plugin:event|listen" && a?.event === "queue://model";

test("N7 review: the model status is asked for only after its listener is registered", async (t) => {
  let release;
  const registered = new Promise((r) => { release = r; });
  t.after(() => release(1));
  const calls = await mountRaw((command, args) => {
    if (command === "plugin:event|listen") return args.event === "queue://model" ? registered : 1;
    if (command === "model_status") return status({ state: "ready" });
    return null;
  });
  assert.ok(calls.some(modelListen));
  assert.ok(!calls.some(([c]) => c === "model_status"), "a completion before listening would be lost");
  await act(async () => release(1));
  assert.ok(calls.findIndex(([c]) => c === "model_status") > calls.findIndex(modelListen));
  assert.match(text(), /local model is downloaded/);
});

test("N7 review: if the model listener can't be registered, the status is polled instead", async () => {
  let asked = 0;
  await mountRaw((command, args) => {
    if (command === "plugin:event|listen") {
      if (args.event === "queue://model") throw "no events";
      return 1;
    }
    if (command === "model_status") {
      asked++;
      return asked < 2 ? status() : status({ state: "ready" });
    }
    return null;
  });
  assert.equal(asked, 1);
  await act(async () => new Promise((r) => setTimeout(r, 1100)));
  assert.ok(asked >= 2, `asked ${asked} times`);
  assert.match(text(), /local model is downloaded/, "a finished download still shows up");
});

test("N7 review: an older event never replaces a newer reply", async () => {
  const older = status({ downloading: true, state: "partial", received: 250_000_000 });
  await mount({
    model_download: () => status({ downloading: true, state: "partial", received: 1_250_000_000 }),
  });
  await click("model-download");
  assert.match(text(), /50%/);
  await act(async () => emit("queue://model", older));
  assert.match(text(), /50%/, "the event was taken earlier, so it's ignored");
});
