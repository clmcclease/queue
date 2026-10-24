// Q7 / #83 / #85: React behavior with mocked IPC; native loading is covered in Rust.
import assert from "node:assert/strict";
import { after, afterEach, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { readFile } from "node:fs/promises";
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
async function mount(list, problem = null) {
  const calls = [];
  mockIPC((command, args) => {
    calls.push({ command, args });
    if (command === "queue_list") return list;
    if (command === "queue_storage_problem") return problem;
    if (command === "queue_runner_state") return false;
    if (command === "settings_download_folder") return { path: "/Downloads", error: null };
    if (command === "settings_naming_template") return "title";
    if (command === "naming_previews") return [];
    if (command === "naming_status") return { provider: "off", unavailable: "off" };
    if (command === "model_status") return null;
    if (command === "settings_naming_provider") return "off";
    if (command === "settings_format") return "default";
    if (command === "queue_remove") return null;
    assert.fail(`unexpected command ${command}`);
  }, { shouldMockEvents: true });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(App)));
  return calls;
}

test("#83: the startup version warning is an alert and an empty queue does not auto-start", async () => {
  // Use the backend's actual warning text, delivered through the real startup IPC read.
  const backend = await readFile(new URL("../../src-tauri/src/lib.rs", import.meta.url), "utf8");
  const problem = backend.match(/queue::Saving::Newer\) => Some\("([^"]+)"/)[1];
  const calls = await mount([], problem);
  const alert = container.querySelector('[data-role="unsaved"]');
  assert.equal(alert.getAttribute("role"), "alert");
  assert.equal(alert.textContent, problem);
  assert.match(problem, /newer version/);
  assert.match(problem, /won't be saved/);
  assert.equal(container.querySelectorAll("li").length, 0);
  assert.equal(calls.some(c => ["queue_download", "queue_run_all", "queue_retry"].includes(c.command)), false);
});

test("#85: adjacent largest safe ids stay distinct through JSON, events, and removal IPC", async () => {
  const list = JSON.parse(`[
    {"id":9007199254740990,"url":"https://e.test/previous","rev":1,"status":{"state":"queued"},"meta":{"title":"previous 日本語"},"name":"previous","progress":null,"attempt":0},
    {"id":9007199254740991,"url":"https://e.test/last","rev":1,"status":{"state":"queued"},"meta":{"title":"last 日本語"},"name":"last","progress":null,"attempt":0}
  ]`);
  const calls = await mount(list);
  assert.equal(list[1].id, Number.MAX_SAFE_INTEGER);
  assert.notEqual(list[0].id, list[1].id);
  await act(async () => emit("queue://item", { ...list[1], rev: 2, meta: { title: "updated last 日本語" } }));
  let rows = [...container.querySelectorAll("li")];
  assert.equal(rows.length, 2);
  assert.equal(rows[0].querySelector('[data-role="title"]').textContent, "previous 日本語");
  assert.equal(rows[1].querySelector('[data-role="title"]').textContent, "updated last 日本語");
  await act(async () => rows[1].querySelector('button[aria-label="Remove"]').click());
  assert.deepEqual(calls.filter(c => c.command === "queue_remove").map(c => c.args), [{ id: Number.MAX_SAFE_INTEGER }]);
  rows = [...container.querySelectorAll("li")];
  assert.equal(rows.length, 1);
  assert.equal(rows[0].querySelector('[data-role="title"]').textContent, "previous 日本語");
});
