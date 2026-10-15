// Independent QA: #11 / D2. React DOM with mocked Tauri IPC, including slow saves,
// failed reads, reopening Settings, StrictMode and out-of-order queue events.
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

function deferred() {
  let resolve, reject;
  const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}
async function mount(handler, { strict = false } = {}) {
  mockIPC((command, args) => {
    const answer = handler(command, args);
    if (answer !== undefined) return answer;
    if (command === "queue_list") return [];
    if (command === "queue_runner_state") return false;
    if (command === "settings_download_folder") return { path: "/Downloads 日本語", error: null };
    if (command === "settings_format") return "default";
    return null;
  }, { shouldMockEvents: true });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  const app = React.createElement(App);
  await act(async () => root.render(strict ? React.createElement(React.StrictMode, null, app) : app));
}
const gear = () => container.querySelector('button[aria-label="Settings"]');
const close = () => container.querySelector('button[aria-label="Close settings"]');
const radios = () => [...container.querySelectorAll('[data-role="format"] input[type="radio"]')];
const radio = (value) => radios().find((r) => r.value === value);
const checked = () => radios().find((r) => r.checked)?.value;
const click = async (el) => { await act(async () => el.click()); };

test("#11 QA: a pending save disables every choice, prevents a second write, and survives reopening", async () => {
  const pending = deferred();
  const saves = [];
  let saved = "default";
  await mount((command, args) => {
    if (command === "settings_format") return saved;
    if (command === "settings_set_format") {
      saves.push(args.format);
      return pending.promise.then(() => { saved = args.format; return saved; });
    }
  });
  await click(gear());
  await click(radio("compatibleVideo"));
  assert.equal(checked(), "compatibleVideo");
  assert.ok(radios().every((r) => r.disabled));
  await click(radio("audio"));
  assert.deepEqual(saves, ["compatibleVideo"]);
  await act(async () => pending.resolve());
  assert.ok(radios().every((r) => !r.disabled));
  await click(close());
  await click(gear());
  assert.equal(checked(), "compatibleVideo", "reopening reads the saved backend value");
});

test("#11 QA: each preset is sent to IPC once and remains selected after reopening", async () => {
  let saved = "default";
  const saves = [];
  await mount((command, args) => {
    if (command === "settings_format") return saved;
    if (command === "settings_set_format") { saves.push(args.format); saved = args.format; return saved; }
  });
  await click(gear());
  assert.equal(checked(), "default");
  for (const format of ["audio", "compatibleAudio", "compatibleVideo", "default"]) {
    await click(radio(format));
    await click(radio(format));
    await click(close());
    await click(gear());
    assert.equal(checked(), format);
  }
  assert.deepEqual(saves, ["audio", "compatibleAudio", "compatibleVideo", "default"]);
});

test("#11 QA: a failed initial settings read disables writes and reopening can recover", async () => {
  let fail = true;
  await mount((command) => {
    if (command === "settings_format") {
      if (fail) throw "read failed <b>日本語</b>";
      return "compatibleAudio";
    }
    if (command === "settings_set_format") assert.fail("unknown settings must not be overwritten");
  });
  await click(gear());
  assert.ok(radios().every((r) => r.disabled));
  assert.equal(checked(), undefined);
  assert.equal(container.querySelector('[data-role="format-error"]').textContent, "read failed <b>日本語</b>");
  assert.equal(container.querySelector('[data-role="format-error"] b'), null);
  await click(close());
  fail = false;
  await click(gear());
  assert.equal(checked(), "compatibleAudio");
  assert.equal(container.querySelector('[data-role="format-error"]'), null);
});

test("#11 QA: StrictMode ignores a stale read failure after a newer successful read", async () => {
  const stale = deferred();
  let reads = 0;
  await mount((command) => {
    if (command === "settings_format") return ++reads === 1 ? stale.promise : "audio";
  }, { strict: true });
  await click(gear());
  assert.equal(reads, 2);
  assert.equal(checked(), "audio");
  await act(async () => stale.reject("stale settings error"));
  assert.equal(checked(), "audio");
  assert.equal(container.querySelector('[data-role="format-error"]'), null);
});

test("#11 QA: a failed save restores the saved choice and a successful retry clears the error", async () => {
  let fail = true;
  await mount((command, args) => {
    if (command === "settings_format") return "compatibleVideo";
    if (command === "settings_set_format") {
      if (fail) throw "disk full 日本語";
      return args.format;
    }
  });
  await click(gear());
  await click(radio("compatibleAudio"));
  assert.equal(checked(), "compatibleVideo");
  assert.match(container.querySelector('[data-role="format-error"]').textContent, /disk full/);
  assert.ok(radios().every((r) => !r.disabled));
  fail = false;
  await click(radio("compatibleAudio"));
  assert.equal(checked(), "compatibleAudio");
  assert.equal(container.querySelector('[data-role="format-error"]'), null);
});

test("#11 QA: a newer finished-row notice survives an older snapshot and is rendered literally", async () => {
  const snapshot = deferred();
  await mount((command) => command === "queue_list" ? snapshot.promise : undefined);
  const done = { id: 1, url: "https://example.test/video", status: { state: "done", path: "/Downloads 日本語/video.webm" }, meta: null, progress: null, attempt: 0, rev: 4, notice: null };
  await act(async () => emit("queue://item", done));
  const notice = "No H.264 version was available <img src=x> 日本語";
  await act(async () => emit("queue://item", { ...done, rev: 5, notice }));
  await act(async () => snapshot.resolve([done]));
  assert.equal(container.querySelector('[data-role="notice"]').textContent, notice);
  assert.equal(container.querySelector('[data-role="notice"] img'), null);
});
