// Independent QA: #12 / Q7 startup behavior in React; native persistence is tested in Rust.
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

const interrupted = "Interrupted when Queue closed. Download it again when you're ready.";
const base = {
  url: "https://e.test/video", rev: 1, progress: null, attempt: 0, notice: null,
  meta: { title: 'CON 日本語 🎵 <img src=x> & "quotes"', channel: "Artist", duration: 75, thumbnail: null, site: "test" },
};
const queued = { ...base, id: 8, status: { state: "queued" }, notice: interrupted };
const done = { ...base, id: 9, status: { state: "done", path: "C:\\Downloads 日本語\\_CON (2).opus" }, notice: "Original codec retained <script>literal</script>" };
const failed = { ...base, id: 10, meta: null, status: { state: "failed", reason: 'Timed out 日本語 <img src=x> & "quotes"' } };
const fetching = { ...base, id: 11, meta: null, status: { state: "fetching" } };
const rows = () => [...container.querySelectorAll("li")];
const button = (scope, label) => [...scope.querySelectorAll("button")].find((b) => b.textContent === label);
const click = async (element) => { await act(async () => element.click()); };
async function mount(list, handler) {
  const calls = [];
  mockIPC((command, args) => {
    calls.push({ command, args });
    if (command === "queue_list") return typeof list === "function" ? list() : list;
    if (command === "queue_runner_state") return false;
    if (command === "settings_download_folder") return { path: "/Downloads 日本語", error: null };
    if (command === "settings_naming_template") return "title"; // #17
    if (command === "naming_previews") return [];
    if (command === "settings_format") return "compatibleAudio";
    return handler(command, args);
  }, { shouldMockEvents: true });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(App)));
  return calls;
}

test("#12 Q7: restored rows wait for user action, retain literal history, and load remembered settings", async () => {
  const calls = await mount([queued, done, failed, fetching], (command, args) => {
    if (command === "queue_download") return { ...queued, rev: 2, status: { state: "downloading" }, notice: null, attempt: 1 };
    if (command === "reveal_download") return null;
    if (command === "queue_retry") return { ...failed, rev: 2, status: { state: "fetching" } };
    assert.fail(`unexpected command: ${command} ${JSON.stringify(args)}`);
  });
  assert.deepEqual(rows().map((r) => r.dataset.state), ["queued", "done", "failed", "fetching"]);
  assert.equal(calls.filter((c) => ["queue_download", "queue_run_all", "queue_retry"].includes(c.command)).length, 0);
  assert.equal(rows()[0].querySelector('[data-role="notice"]').textContent, interrupted);
  assert.equal(rows()[0].querySelector('[data-role="title"]').textContent, base.meta.title);
  assert.equal(rows()[0].querySelector('[data-role="title"] img'), null);
  assert.ok(rows()[1].textContent.includes("_CON (2).opus"));
  assert.equal(rows()[1].querySelector('[data-role="notice"]').textContent, done.notice);
  assert.ok(rows()[2].textContent.includes(failed.status.reason));
  assert.equal(container.querySelector("script"), null);
  assert.equal(container.querySelector("[role=progressbar]"), null);
  await click(container.querySelector('button[aria-label="Settings"]'));
  assert.equal(container.querySelector('input[value="compatibleAudio"]').checked, true);
  assert.ok(container.textContent.includes("/Downloads 日本語"));
  await click(container.querySelector('button[aria-label="Close settings"]'));
  await click([...rows()[1].querySelectorAll("button")].find((b) => b.textContent.startsWith("Show in")));
  assert.deepEqual(calls.filter((c) => c.command === "reveal_download").map((c) => c.args), [{ id: done.id }]);
  await click(button(rows()[2], "Retry"));
  assert.equal(rows()[2].dataset.state, "fetching");
  await click(button(rows()[0], "Download"));
  assert.equal(rows()[0].dataset.state, "downloading");
  assert.equal(rows()[0].querySelector('[data-role="notice"]'), null);
  assert.deepEqual(calls.filter((c) => c.command === "queue_download").map((c) => c.args), [{ id: queued.id }]);
});

test("#12 Q7: refetched metadata arriving before the restored snapshot wins over revision one", async () => {
  let resolveList;
  const listed = new Promise((resolve) => { resolveList = resolve; });
  const calls = await mount(() => listed, (command) => assert.fail(`unexpected command: ${command}`));
  const refreshed = {
    ...fetching, rev: 2, meta: { ...base.meta, title: "New fetched title e\u0301 日本語" },
    status: { state: "queued" },
  };
  await act(async () => emit("queue://item", refreshed));
  await act(async () => resolveList([fetching, queued]));
  assert.deepEqual(rows().map((r) => r.dataset.state), ["queued", "queued"]);
  assert.equal(rows()[1].querySelector('[data-role="title"]').textContent, refreshed.meta.title);
  assert.equal(rows()[0].querySelector('[data-role="notice"]').textContent, interrupted);
  assert.equal(calls.filter((c) => ["queue_download", "queue_run_all"].includes(c.command)).length, 0);
});
