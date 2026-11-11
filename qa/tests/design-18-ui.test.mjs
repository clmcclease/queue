// #18 (D-045): the design pass's behaviour in the real React DOM: the thumbnail fallback, the
// empty-state hint, the spinner rows, and Test connection's check mark.
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
let server, Editor, ApiSetup, root, container;
before(async () => {
  server = await createServer({
    root: fileURLToPath(new URL("../../", import.meta.url)),
    configFile: false, logLevel: "silent", appType: "custom",
    server: { middlewareMode: true, hmr: false, watch: null },
  });
  ({ default: Editor } = await server.ssrLoadModule("/src/components/QueueList.tsx"));
  ({ default: ApiSetup } = await server.ssrLoadModule("/src/components/ApiSetup.tsx"));
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
}
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

const props = (items) => ({
  items,
  onDownload() {},
  onRetry() {},
  onRename() {},
  onRemove() {},
  onCancel() {},
  onReveal() {},
  cancelling: new Set(),
  connected: true,
  busy: false,
  highlight: null,
});
async function render(items) {
  await act(async () => root.render(React.createElement(Editor, props(items))));
}
const row = (state, extra = {}) => ({
  id: 1,
  url: "https://example.test/v",
  status: { state, ...(state === "done" ? { path: "/d/A.mp4" } : state === "failed" ? { reason: "Unsupported URL" } : {}) },
  rev: 1,
  meta: { title: "A", channel: null, duration: null, thumbnail: "https://cdn.example.test/expired.jpg", site: null },
  name: "A",
  name_source: "rules",
  ...extra,
});

test("#18: an empty queue keeps its line and adds a hint with the platform's shortcut", async () => {
  await mount(() => null);
  await render([]);
  const ps = [...container.querySelectorAll("p")];
  assert.equal(ps[0].outerHTML, "<p>Queue is empty.</p>", "the line itself is unchanged");
  const hint = container.querySelector('[data-role="empty-hint"]');
  assert.match(hint.textContent, /^Add links above, then press Download all \((⌘↩|Ctrl\+Enter)\)\.$/);
  assert.doesNotMatch(hint.textContent, /⌘\/Ctrl/, "never a literal ⌘/Ctrl");
});

test("#18: a thumbnail that fails to load falls back to the neutral box", async () => {
  await mount(() => null);
  await render([row("queued")]);
  const img = container.querySelector("img");
  assert.equal(img.getAttribute("src"), "https://cdn.example.test/expired.jpg");
  const box = img.parentElement;
  await act(async () => img.dispatchEvent(new dom.window.Event("error")));
  assert.equal(container.querySelector("img"), null, "no broken image");
  assert.ok(box.isConnected && box.className.includes("bg-neutral-200"), "the neutral box stays");
  // A new link for the same row is tried again.
  await render([row("queued", { meta: { title: "A", channel: null, duration: null, thumbnail: "https://cdn.example.test/fresh.jpg", site: null } })]);
  assert.equal(container.querySelector("img")?.getAttribute("src"), "https://cdn.example.test/fresh.jpg");
});

test("#18: Fetching and Processing rows carry a hidden spinner beside their text; others don't", async () => {
  await mount(() => null);
  for (const [state, spins, text] of [
    ["fetching", true, "Fetching info…"],
    ["processing", true, "Processing…"],
    ["queued", false, null],
    ["downloading", false, null],
    ["done", false, "✓ A.mp4"],
    ["failed", false, "Failed: Unsupported URL"],
  ]) {
    await render([row(state)]);
    const spinner = container.querySelector('[data-role="spinner"]');
    assert.equal(!!spinner, spins, state);
    if (spinner) {
      assert.equal(spinner.getAttribute("aria-hidden"), "true");
      assert.match(spinner.className, /motion-reduce:animate-none/);
    }
    if (text) assert.ok(container.textContent.includes(text), `${state}: ${text}`);
  }
});

test("#18: Done is neutral with a check mark, not green", async () => {
  await mount(() => null);
  await render([row("done")]);
  const done = [...container.querySelectorAll("span")].find((s) => s.textContent === "✓ A.mp4");
  assert.ok(done);
  assert.doesNotMatch(done.className, /green/);
  assert.match(done.className, /text-neutral-700/);
});

test("#18: a passed Test connection differs from a failure by more than colour", async () => {
  const saved = { kind: "openai_compatible", baseUrl: "https://api.openai.com/v1", model: "m", keySet: true, keyHost: "api.openai.com", storeError: null };
  for (const [ok, message] of [[true, "Connected."], [false, "The naming service answered with HTTP 401."]]) {
    await mount((command) => {
      if (command === "settings_api") return { ...saved };
      if (command === "api_test_connection") return { ok, message };
      return null;
    });
    await act(async () => root.render(React.createElement(ApiSetup, {})));
    await act(async () => container.querySelector('[data-role="api-test"]').click());
    assert.equal(container.querySelector('[data-role="api-test-result"]').textContent, message, "the message is unchanged");
    const mark = container.querySelector('[data-role="api-test-ok"]');
    if (ok) {
      assert.equal(mark?.textContent.trim(), "✓", "a pass shows a check mark");
      assert.equal(mark.getAttribute("aria-hidden"), "true", "screen readers already hear the message");
    } else {
      assert.equal(mark, null, "a failure has none");
    }
    await act(async () => root.unmount());
    root = undefined;
    container.remove();
    clearMocks();
  }
});
