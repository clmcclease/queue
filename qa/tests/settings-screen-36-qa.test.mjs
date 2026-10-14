// Independent QA: #36 / PRD §6.4 and its D1 control. Real React components,
// mocked Tauri IPC; jsdom does not exercise native focus traversal or dialogs.
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
const apps = new Map();
const servers = [];
let root, container;
const original = { path: "/original", error: null };
const selected = { path: "C:\\Vidéos 日本語 e\u0301 🎵 & <b>literal</b>", error: null };

before(async () => {
  // App chooses its shortcut at module evaluation. Load an independent module
  // instance for each platform rather than inferring the answer from its tooltip.
  const previous = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  try {
    for (const platform of ["MacIntel", "Win32", "Linux x86_64"]) {
      Object.defineProperty(globalThis, "navigator", { configurable: true, value: { platform } });
      const server = await createServer({
        root: fileURLToPath(new URL("../../", import.meta.url)),
        configFile: false, logLevel: "silent", appType: "custom",
        server: { middlewareMode: true, hmr: false, watch: null },
      });
      servers.push(server);
      apps.set(platform, (await server.ssrLoadModule("/src/App.tsx")).default);
    }
  } finally {
    if (previous) Object.defineProperty(globalThis, "navigator", previous);
    else delete globalThis.navigator;
  }
});
afterEach(async () => {
  if (root) await act(async () => root.unmount());
  root = undefined;
  container?.remove();
  clearMocks();
});
after(async () => {
  await Promise.all(servers.map((server) => server.close()));
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
async function mount(handler = () => undefined, { platform = "Win32", strict = false } = {}) {
  mockIPC((command, args) => {
    const reply = handler(command, args);
    if (reply !== undefined) return reply;
    if (command === "queue_list") return [];
    if (command === "queue_runner_state") return false;
    if (command === "settings_download_folder") return original;
    return null;
  }, { shouldMockEvents: true });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  const app = React.createElement(apps.get(platform));
  await act(async () => root.render(strict ? React.createElement(React.StrictMode, null, app) : app));
}
const panel = () => container.querySelector('[role="dialog"][data-role="settings"]');
const gear = () => container.querySelector('button[aria-label="Settings"]');
const close = () => panel().querySelector('button[aria-label="Close settings"]');
const changes = () => [...container.querySelectorAll("button")].filter((b) => b.textContent === "Change…");
const paths = () => [...container.querySelectorAll('[data-role="folder"]')].map((el) => el.textContent);
const errors = () => [...container.querySelectorAll('[data-role="folder-error"]')].map((el) => el.textContent);
async function click(element) { await act(async () => element.click()); }
async function open() { gear().focus(); await click(gear()); }
async function change() { await click([...panel().querySelectorAll("button")].find((b) => b.textContent === "Change…")); }
async function key(target, options) {
  const event = new window.KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...options });
  await act(async () => target.dispatchEvent(event));
  return event;
}

for (const platform of ["MacIntel", "Win32", "Linux x86_64"]) {
  test(`#36 QA: ${platform} uses its own shortcut, labels the dialog, and restores the keyboard opener`, async () => {
    await mount(undefined, { platform });
    const input = container.querySelector("input");
    input.focus();
    const mac = platform === "MacIntel";
    await key(input, { key: ",", ...(mac ? { ctrlKey: true } : { metaKey: true }) });
    assert.equal(panel(), null);
    const opened = await key(input, { key: ",", ...(mac ? { metaKey: true } : { ctrlKey: true }) });
    assert.equal(opened.defaultPrevented, true);
    assert.equal(gear().title, mac ? "Settings (⌘,)" : "Settings (Ctrl+,)");
    assert.equal(panel().getAttribute("aria-modal"), "true");
    assert.equal(document.getElementById(panel().getAttribute("aria-labelledby")).textContent, "Settings");
    assert.equal(document.activeElement, close());
    const pickerButton = changes().at(-1);
    pickerButton.focus();
    await key(pickerButton, { key: ",", ...(mac ? { metaKey: true } : { ctrlKey: true }) });
    assert.equal(container.querySelectorAll('[role="dialog"]').length, 1);
    assert.equal(document.activeElement, pickerButton, "repeated shortcut must not remount the panel");
    await key(pickerButton, { key: "Escape" });
    assert.equal(panel(), null);
    assert.equal(document.activeElement, input);
    assert.equal(gear().closest("[inert]"), null);
  });
}

test("#36 QA: pending picker survives closing/reopening Settings and disables both copies", async (t) => {
  const pending = deferred();
  t.after(() => pending.resolve(null));
  let calls = 0;
  await mount((command) => {
    if (command === "settings_choose_download_folder") { calls++; return pending.promise; }
  });
  await open();
  await change();
  assert.ok(changes().every((button) => button.disabled));
  await change();
  await click(close());
  assert.equal(changes()[0].disabled, true);
  await open();
  await change();
  assert.equal(calls, 1, "a remounted panel must not launch a second native picker");
  await act(async () => pending.resolve(selected));
  assert.deepEqual(paths(), [selected.path, selected.path]);
  assert.ok(changes().every((button) => !button.disabled));
  assert.equal(container.querySelector("b"), null, "paths are literal text");
});

test("#36 QA: picker rejection after panel closes remains visible on reopen and can recover", async (t) => {
  const pending = deferred();
  t.after(() => pending.resolve(null));
  let calls = 0;
  await mount((command) => {
    if (command === "settings_choose_download_folder") return ++calls === 1 ? pending.promise : selected;
  });
  await open();
  await change();
  await click(close());
  const message = "Couldn't save <img src=x> 日本語 settings: permission denied";
  await act(async () => pending.reject(message));
  assert.deepEqual(errors(), [message]);
  await open();
  assert.deepEqual(errors(), [message, message]);
  assert.deepEqual(paths(), [original.path, original.path]);
  assert.equal(container.querySelector("img"), null);
  await change();
  assert.deepEqual(errors(), []);
  assert.deepEqual(paths(), [selected.path, selected.path]);
});

test("#36 QA: cancelling preserves an unavailable folder and its error across panel remounts", async () => {
  const unavailable = { path: "Z:\\offline 日本語", error: "Volume isn't available. Choose another with Change…." };
  await mount((command) => command === "settings_download_folder" ? unavailable : undefined);
  await open();
  await change();
  assert.deepEqual(paths(), [unavailable.path, unavailable.path]);
  assert.deepEqual(errors(), [unavailable.error, unavailable.error]);
  await click(close());
  await open();
  assert.deepEqual(errors(), [unavailable.error, unavailable.error]);
});

test("#36 QA: a late startup snapshot cannot undo a folder selected inside Settings", async (t) => {
  const startup = deferred();
  t.after(() => startup.resolve(original));
  await mount((command) => {
    if (command === "settings_download_folder") return startup.promise;
    if (command === "settings_choose_download_folder") return selected;
  });
  await open();
  assert.deepEqual(paths(), ["…", "…"]);
  await change();
  await act(async () => startup.resolve(original));
  assert.deepEqual(paths(), [selected.path, selected.path]);
});

test("#36 QA: a stale startup rejection cannot surface after a successful Settings selection", async (t) => {
  const startup = deferred();
  t.after(() => startup.resolve(original));
  await mount((command) => {
    if (command === "settings_download_folder") return startup.promise;
    if (command === "settings_choose_download_folder") return selected;
  });
  await open();
  await change();
  await act(async () => startup.reject("stale startup read failure"));
  assert.deepEqual(paths(), [selected.path, selected.path]);
  assert.deepEqual(errors(), []);
});

test("#36 QA: cancel does not clear a startup read error; a successful choice clears both copies", async () => {
  let calls = 0;
  await mount((command) => {
    if (command === "settings_download_folder") throw "settings read failed 日本語";
    if (command === "settings_choose_download_folder") return ++calls === 1 ? null : selected;
  });
  await open();
  await change();
  assert.deepEqual(errors(), ["settings read failed 日本語", "settings read failed 日本語"]);
  await change();
  assert.deepEqual(errors(), []);
  assert.deepEqual(paths(), [selected.path, selected.path]);
});

test("#36 QA: a new App instance reads the backend's remembered Settings choice", async () => {
  let remembered = original;
  const handler = (command) => {
    if (command === "settings_download_folder") return remembered;
    if (command === "settings_choose_download_folder") { remembered = selected; return remembered; }
  };
  await mount(handler);
  await open();
  await change();
  await act(async () => root.unmount());
  root = undefined;
  container.remove();
  await mount(handler);
  await open();
  assert.deepEqual(paths(), [selected.path, selected.path]);
});

test("#36 QA: background queue events update rows while Settings keeps its focus and folder", async () => {
  const item = { id: 1, url: "https://example.test/video", rev: 1, status: { state: "queued" } };
  await mount((command) => command === "queue_list" ? [item] : undefined);
  await open();
  await act(async () => emit("queue://item", { ...item, rev: 2, status: { state: "failed", reason: "Unavailable 日本語" } }));
  assert.ok(panel());
  assert.equal(document.activeElement, close());
  assert.deepEqual(paths(), [original.path, original.path]);
  await click(close());
  assert.match(container.querySelector("li").textContent, /Failed: Unavailable 日本語/);
});

test("#36 QA: StrictMode preserves a selected folder and returns focus after Escape", async () => {
  await mount((command) => command === "settings_choose_download_folder" ? selected : undefined, { strict: true });
  await open();
  await change();
  await key(close(), { key: "Escape" });
  assert.equal(document.activeElement, gear());
  await open();
  assert.deepEqual(paths(), [selected.path, selected.path]);
});

test("#36 QA: StrictMode restores the opener when inert elements cannot receive focus", async () => {
  // jsdom exposes inert as an attribute but still lets inert descendants receive
  // focus. Supply only that missing browser constraint. This matters because
  // src/main.tsx wraps App in StrictMode, which replays the panel's mount effect.
  const prototype = window.HTMLElement.prototype;
  const nativeFocus = prototype.focus;
  prototype.focus = function (...args) {
    if (!this.closest("[inert]")) return nativeFocus.apply(this, args);
  };
  try {
    await mount(undefined, { strict: true });
    await open();
    assert.equal(document.activeElement, close(), "initial focus enters Settings");
    await click(close());
    assert.equal(panel(), null);
    assert.equal(gear().closest("[inert]"), null);
    assert.ok(document.activeElement === gear(), `closing must restore the original opener; focused ${document.activeElement.tagName}`);
  } finally {
    prototype.focus = nativeFocus;
  }
});

test("#36 QA: unmount removes the Settings keyboard listener", async () => {
  await mount();
  await open();
  await act(async () => root.unmount());
  root = undefined;
  const event = await key(window, { key: ",", ctrlKey: true });
  assert.equal(event.defaultPrevented, false, "an unmounted App must not consume keyboard shortcuts");
});
