// #21 (D-042): the yt-dlp update setting in Settings. Real React DOM, mocked Tauri IPC.
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
let server, Editor, root, container;
before(async () => {
  server = await createServer({
    root: fileURLToPath(new URL("../../", import.meta.url)),
    configFile: false, logLevel: "silent", appType: "custom",
    server: { middlewareMode: true, hmr: false, watch: null },
  });
  ({ default: Editor } = await server.ssrLoadModule("/src/components/YtdlpUpdates.tsx"));
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
async function mount(handler) {
  mockIPC(handler);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(Editor)));
}
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const box = () => container.querySelector('[data-role="ytdlp-updates"] input[type="checkbox"]');
const line = () => container.querySelector('[data-role="ytdlp-status"]')?.textContent;
const BUNDLED = { enabled: true, version: "2026.08.19", updated: false, lastCheck: null, lastResult: null };

test("#21: Settings shows the setting and the copy in use, and saves nothing on opening", async () => {
  const calls = [];
  await mount((cmd, args) => {
    calls.push(cmd);
    return BUNDLED;
  });
  assert.equal(box().checked, true);
  assert.equal(line(), "yt-dlp 2026.08.19 (came with Queue)");
  assert.deepEqual(calls, ["settings_ytdlp"]);
});

test("#21: an updated copy and the last check are shown", async () => {
  await mount(() => ({
    enabled: true,
    version: "2026.10.26",
    updated: true,
    lastCheck: 1794338509,
    lastResult: "Updated to 2026.10.26.",
  }));
  assert.match(line(), /^yt-dlp 2026\.10\.26 \(updated\) · Last check .+: Updated to 2026\.10\.26\.$/);
});

test("#21: turning it off saves false and shows the bundled copy", async () => {
  const calls = [];
  await mount((cmd, args) => {
    calls.push({ cmd, args });
    if (cmd === "settings_set_ytdlp") return { ...BUNDLED, enabled: args.enabled };
    return { ...BUNDLED, version: "2026.10.26", updated: true };
  });
  await act(async () => box().click());
  assert.deepEqual(calls.at(-1), { cmd: "settings_set_ytdlp", args: { enabled: false } });
  assert.equal(box().checked, false);
  assert.equal(line(), "yt-dlp 2026.08.19 (came with Queue)");
});

test("#21: a failed save keeps the box as it was and shows the reason literally", async () => {
  await mount((cmd) => {
    if (cmd === "settings_set_ytdlp") throw "Couldn't save your settings: <disk full>";
    return BUNDLED;
  });
  await act(async () => box().click());
  assert.equal(box().checked, true);
  const alert = container.querySelector('[data-role="ytdlp-error"]');
  assert.equal(alert.textContent, "Couldn't save your settings: <disk full>");
  assert.equal(alert.querySelector("*"), null);
});

test("#21: a slow first read keeps the box disabled; a malformed reply shows nothing wrong", async () => {
  const pending = deferred();
  await mount(() => pending.promise);
  assert.equal(box().disabled, true);
  await act(async () => pending.resolve({ enabled: "yes" }));
  assert.equal(box().disabled, true);
  assert.equal(line(), undefined);
});
