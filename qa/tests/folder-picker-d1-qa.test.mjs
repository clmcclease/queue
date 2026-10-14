// Independent QA: #10 / D1. Real component, mocked IPC; no native dialog claims.
import assert from "node:assert/strict";
import { after, afterEach, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { JSDOM } from "jsdom";
import React, { act } from "react";
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks";
import { createServer } from "vite";

const dom = new JSDOM("<!doctype html><html><body></body></html>", { url: "http://localhost/" });
globalThis.window = dom.window;
globalThis.document = dom.window.document;
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const { createRoot } = await import("react-dom/client");
let server, FolderPicker, root, container;
before(async () => {
  server = await createServer({
    root: fileURLToPath(new URL("../../", import.meta.url)),
    configFile: false, logLevel: "silent", appType: "custom",
    server: { middlewareMode: true, hmr: false, watch: null },
  });
  ({ default: FolderPicker } = await server.ssrLoadModule("/src/components/FolderPicker.tsx"));
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
async function mount(handler) {
  mockIPC(handler);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(FolderPicker)));
}
const folder = () => container.querySelector('[data-role="folder"]');
const error = () => container.querySelector('[data-role="folder-error"]');
const change = async () => act(async () => container.querySelector("button").click());
const original = { path: "/old", error: null };
const picked = { path: "C:\\Vidéos 日本語 & <b>literal</b>", error: null };

test("D1 QA #10: late initial folder snapshot cannot undo a successful selection", async (t) => {
  const startup = deferred();
  t.after(() => startup.resolve(original));
  await mount((command) => command === "settings_download_folder" ? startup.promise : picked);
  await change();
  assert.equal(folder().textContent, picked.path);
  await act(async () => startup.resolve(original));
  assert.equal(folder().textContent, picked.path, "new selection must win over the stale startup snapshot");
  assert.equal(folder().title, picked.path);
  assert.equal(container.querySelector("b"), null);
});

test("D1 QA #10: late startup rejection cannot report an error after successful selection", async (t) => {
  const startup = deferred();
  t.after(() => startup.resolve(original));
  await mount((command) => command === "settings_download_folder" ? startup.promise : picked);
  await change();
  await act(async () => startup.reject("old settings read failed"));
  assert.equal(folder().textContent, picked.path);
  assert.equal(error()?.textContent ?? null, null, "a successful newer choice supersedes the old read error");
});

test("D1 QA #10: startup failure is literal and choosing another folder recovers", async () => {
  await mount((command) => {
    if (command === "settings_download_folder") throw "<img src=x> 日本語 unavailable";
    return picked;
  });
  assert.equal(error().textContent, "<img src=x> 日本語 unavailable");
  assert.equal(container.querySelector("img"), null);
  await change();
  assert.equal(folder().textContent, picked.path);
  assert.equal(error(), null);
});

test("D1 QA #10: cancelling preserves an unavailable folder and its error", async () => {
  const unavailable = { path: "/offline 日本語", error: "Choose another with Change…." };
  await mount((command) => command === "settings_download_folder" ? unavailable : null);
  await change();
  assert.equal(folder().textContent, unavailable.path);
  assert.equal(error().textContent, unavailable.error);
  assert.equal(container.querySelector("button").disabled, false);
});

test("D1 QA #10: pending picker blocks duplicate clicks; rejection restores retry", async (t) => {
  const pending = deferred();
  t.after(() => pending.resolve(null));
  let calls = 0;
  await mount((command) => {
    if (command === "settings_download_folder") return original;
    calls++;
    return pending.promise;
  });
  await change();
  await change();
  assert.equal(calls, 1);
  assert.equal(container.querySelector("button").disabled, true);
  await act(async () => pending.reject("Couldn't save: denied 日本語"));
  assert.equal(folder().textContent, original.path);
  assert.equal(error().textContent, "Couldn't save: denied 日本語");
  assert.equal(container.querySelector("button").disabled, false);
});

test("D1 QA #10: remount reads the remembered backend choice", async () => {
  let remembered = original;
  const handler = (command) => {
    if (command === "settings_download_folder") return remembered;
    remembered = picked;
    return remembered;
  };
  await mount(handler);
  await change();
  await act(async () => root.unmount());
  root = undefined;
  container.remove();
  await mount(handler);
  assert.equal(folder().textContent, picked.path);
});
