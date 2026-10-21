// Independent QA: #59 / Q2. Actual React UI with mocked IPC; Rust tests cover deduplication.
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

const url = "https://example.test/music";
const finished = {
  id: 1, url, rev: 8, status: { state: "done", path: "/Downloads 日本語/_CON.opus" },
  meta: { title: "CON 🎵 (OFFICIAL VIDEO)", channel: "Artist", duration: 90, thumbnail: null, site: "test" },
  progress: null, attempt: 0, notice: "Original format notice",
};
const fresh = { id: 2, url, rev: 1, status: { state: "fetching" }, meta: null, progress: null, attempt: 0, notice: null };
const rows = () => [...container.querySelectorAll("li")];
const field = () => container.querySelector('input[aria-label="Link to download"]');
const click = async (element) => { await act(async () => element.click()); };

async function mount(initial, handler) {
  mockIPC((command, args) => {
    if (command === "queue_list") return initial;
    if (command === "queue_runner_state") return false;
    if (command === "settings_download_folder") return { path: "/Downloads 日本語", error: null };
    if (command === "settings_naming_template") return "title"; // #17
    if (command === "naming_previews") return [];
    if (command === "naming_status") return { provider: "off", unavailable: "off" }; // #25
    if (command === "model_status") return null; // #31
    if (command === "settings_naming_provider") return "off";
    if (command === "settings_format") return "audio";
    return handler(command, args);
  }, { shouldMockEvents: true });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(App)));
}
async function add() {
  await act(async () => {
    Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, "value").set.call(field(), url);
    field().dispatchEvent(new dom.window.Event("input", { bubbles: true }));
  });
  await click(container.querySelector('button[aria-label="Add to queue"]'));
}

test("#59: same-URL rows retain separate metadata, files and reveal actions after a format change", async () => {
  let format = "audio";
  const calls = [];
  await mount([finished], (command, args) => {
    calls.push({ command, args });
    if (command === "settings_set_format") { format = args.format; return format; }
    if (command === "queue_add") { assert.equal(args.url, url); return fresh; }
    if (command === "reveal_download") return null;
    assert.fail(`unexpected command: ${command}`);
  });
  await click(container.querySelector('button[aria-label="Settings"]'));
  await click(container.querySelector('input[value="compatibleAudio"]'));
  assert.equal(format, "compatibleAudio");
  await click(container.querySelector('button[aria-label="Close settings"]'));
  await add();
  assert.equal(field().value, "");
  assert.deepEqual(rows().map((r) => r.dataset.state), ["done", "fetching"]);
  assert.equal(rows()[1].querySelector('[data-role="title"]'), null, "no inherited metadata");
  assert.equal(rows()[1].querySelector('[data-role="notice"]'), null);
  const newTitle = 'NUL.txt 日本語 <img src=x> & "quoted"';
  const newDone = {
    ...fresh, rev: 5, status: { state: "done", path: "/Downloads 日本語/_CON.m4a" },
    meta: { ...finished.meta, title: newTitle },
  };
  await act(async () => emit("queue://item", newDone));
  // Late events for either id cannot regress either completed row.
  await act(async () => emit("queue://item", { ...finished, rev: 3, status: { state: "downloading" } }));
  await act(async () => emit("queue://item", fresh));
  assert.deepEqual(rows().map((r) => r.dataset.state), ["done", "done"]);
  assert.equal(rows()[0].querySelector('[data-role="title"]').textContent, finished.meta.title);
  assert.equal(rows()[0].querySelector('[data-role="notice"]').textContent, finished.notice);
  assert.equal(rows()[1].querySelector('[data-role="title"]').textContent, newTitle);
  assert.equal(rows()[1].querySelector('[data-role="title"] img'), null);
  assert.ok(rows()[0].textContent.includes("_CON.opus"));
  assert.ok(rows()[1].textContent.includes("_CON.m4a"));
  for (const row of rows()) await click([...row.querySelectorAll("button")].find((b) => b.textContent.startsWith("Show in")));
  assert.deepEqual(calls.filter((c) => c.command === "reveal_download").map((c) => c.args), [{ id: 1 }, { id: 2 }]);
  assert.equal(container.querySelector("[data-highlight]"), null);
});

test("#59: a duplicate reply highlights the active copy among same-URL finished history", async () => {
  await mount([finished, fresh], (command, args) => {
    assert.equal(command, "queue_add");
    assert.equal(args.url, url);
    throw { kind: "duplicate", id: fresh.id, message: "That link is already in the queue." };
  });
  await add();
  assert.equal(rows().length, 2);
  assert.equal(field().value, "");
  assert.equal(rows()[0].hasAttribute("data-highlight"), false);
  assert.equal(rows()[1].hasAttribute("data-highlight"), true);
  assert.equal(rows()[0].dataset.state, "done");
  assert.equal(rows()[0].querySelector('[data-role="notice"]').textContent, finished.notice);
});
