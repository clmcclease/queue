// #19: keyboard shortcuts (D-035). Real React DOM, mocked Tauri IPC. The modifier is ⌘ or Ctrl
// as the platform module decides for the machine running the tests. Then npm run build && npm test.
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
let server, App, root, container, MOD, SHORTCUT;

before(async () => {
  server = await createServer({
    root: fileURLToPath(new URL("../../", import.meta.url)),
    configFile: false, logLevel: "silent", appType: "custom",
    server: { middlewareMode: true, hmr: false, watch: null },
  });
  ({ default: App } = await server.ssrLoadModule("/src/App.tsx"));
  const platform = await server.ssrLoadModule("/src/platform.ts");
  MOD = platform.os === "macos" ? { metaKey: true } : { ctrlKey: true };
  SHORTCUT = platform.DOWNLOAD_ALL_SHORTCUT;
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

const row = (id, state, extra = {}) => ({
  id, url: `https://example.test/${id}`, status: { state }, rev: 1, meta: { title: `T${id}` }, attempt: 0, name: `T${id}`, ...extra,
});

// Mounts the app with `list` as the saved queue; every IPC call is recorded.
async function mount(list = [], overrides = {}) {
  const calls = [];
  mockIPC((command, args) => {
    calls.push([command, args]);
    if (command in overrides) return overrides[command](args);
    switch (command) {
      case "queue_runner_state": return false;
      case "settings_download_folder": return { path: "/Users/me/Downloads", error: null };
      case "settings_naming_template": return "title";
      case "naming_previews": return [];
      case "naming_status": return { provider: "off", unavailable: "off" };
      case "model_status": return null;
      case "settings_naming_provider": return "off";
      case "settings_format": return "default";
      case "queue_storage_problem": return null;
      case "queue_list": return list;
      case "queue_add": return row(100 + calls.length, "fetching", { url: args.url, meta: null, name: null });
      case "queue_run_all": return true;
      case "queue_remove": return null;
      case "queue_rename": return row(args.id, "queued", { name: args.name });
      default: return null;
    }
  }, { shouldMockEvents: true });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(App)));
  await act(async () => {});
  return calls;
}

const field = () => container.querySelector('input[aria-label="Link to download"]');
const rows = () => [...container.querySelectorAll("li")];
const named = (calls, command) => calls.filter(([c]) => c === command);

function paste(target, text) {
  const event = new dom.window.Event("paste", { bubbles: true, cancelable: true });
  event.clipboardData = { getData: (type) => (type === "text/plain" ? text : "") };
  return act(async () => target.dispatchEvent(event));
}

function key(target, init) {
  return act(async () =>
    target.dispatchEvent(new dom.window.KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init })),
  );
}

test("#19: focus starts in the URL bar", async () => {
  await mount();
  assert.equal(document.activeElement, field());
});

test("#19: a one-line paste outside a field fills the URL bar and focuses it; nothing is added yet", async () => {
  const calls = await mount([row(1, "queued")]);
  rows()[0].focus();
  await paste(rows()[0], "  https://example.test/new  ");
  assert.equal(field().value, "https://example.test/new");
  assert.equal(document.activeElement, field());
  assert.equal(named(calls, "queue_add").length, 0, "Enter adds it, as for a typed link");
});

test("#19: a several-line paste outside a field adds one row per line", async () => {
  const calls = await mount();
  await paste(document.body, "https://example.test/a\nhttps://example.test/b");
  assert.deepEqual(named(calls, "queue_add").map(([, a]) => a.url), ["https://example.test/a", "https://example.test/b"]);
  assert.equal(field().value, "", "the field keeps what it had");
});

test("#19: a paste into a text field is the field's own", async () => {
  const calls = await mount([row(1, "queued")]);
  await act(async () => container.querySelector('[data-role="name"]').click());
  const editor = container.querySelector('input[aria-label="File name"]');
  await paste(editor, "https://example.test/not-a-link-paste");
  assert.equal(field().value, "", "the URL bar isn't filled");
  assert.equal(named(calls, "queue_add").length, 0);
});

test("#19: ⌘/Ctrl+Enter is Download all, only when the button is enabled", async () => {
  let calls = await mount([row(1, "queued")]);
  const all = [...container.querySelectorAll("button")].find((b) => b.textContent === "Download all");
  assert.equal(all.disabled, false);
  assert.equal(all.getAttribute("title"), `Download all (${SHORTCUT})`);
  await key(field(), { key: "Enter", ...MOD });
  assert.equal(named(calls, "queue_run_all").length, 1);
  assert.equal(named(calls, "queue_add").length, 0, "the URL bar's Enter doesn't also add");
  await act(async () => root.unmount());
  root = undefined;
  container.remove();
  clearMocks();

  calls = await mount([row(1, "done", { status: { state: "done", path: "/d/x.webm" } })]);
  await key(document.body, { key: "Enter", ...MOD });
  assert.equal(named(calls, "queue_run_all").length, 0, "nothing queued: the button is disabled");
});

test("#19: ⌘/Ctrl+Enter does nothing while Settings is open", async () => {
  const calls = await mount([row(1, "queued")]);
  await act(async () => container.querySelector('button[aria-label="Settings"]').click());
  await key(document.body, { key: "Enter", ...MOD });
  assert.equal(named(calls, "queue_run_all").length, 0);
});

test("#19: ⌘/Ctrl+Enter saves a name being edited before Download all", async () => {
  const calls = await mount([row(1, "queued")]);
  await act(async () => container.querySelector('[data-role="name"]').click());
  const editor = container.querySelector('input[aria-label="File name"]');
  await act(async () => {
    Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, "value").set.call(editor, "Renamed");
    editor.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
  });
  await key(editor, { key: "Enter", ...MOD });
  await act(async () => {});
  const order = calls.map(([c]) => c).filter((c) => c === "queue_rename" || c === "queue_run_all");
  assert.deepEqual(order, ["queue_rename", "queue_run_all"]);
  assert.equal(named(calls, "queue_rename")[0][1].name, "Renamed");
});

test("#19: rows are focusable; arrows move between them and Escape returns to the URL bar", async () => {
  await mount([row(1, "queued"), row(2, "queued"), row(3, "done", { status: { state: "done", path: "/d/x.webm" } })]);
  const [a, b, c] = rows();
  for (const r of [a, b, c]) assert.equal(r.tabIndex, 0);
  a.focus();
  await key(a, { key: "ArrowDown" });
  assert.equal(document.activeElement, b);
  await key(b, { key: "ArrowDown" });
  assert.equal(document.activeElement, c);
  await key(c, { key: "ArrowDown" });
  assert.equal(document.activeElement, c, "stays on the last row");
  await key(c, { key: "ArrowUp" });
  assert.equal(document.activeElement, b);
  await key(b, { key: "Escape" });
  assert.equal(document.activeElement, field());
});

test("#19: Delete or Backspace removes the focused row, never a downloading one, and focus moves on", async () => {
  const calls = await mount([
    row(1, "queued"),
    row(2, "downloading", { progress: null, attempt: 1 }),
    row(3, "failed", { status: { state: "failed", reason: "no" } }),
  ]);
  let [a, b, c] = rows();
  b.focus();
  await key(b, { key: "Delete" });
  assert.equal(named(calls, "queue_remove").length, 0, "a downloading row stays");
  assert.equal(document.activeElement, b);
  a.focus();
  await key(a, { key: "Delete" });
  assert.deepEqual(named(calls, "queue_remove").map(([, x]) => x.id), [1]);
  assert.equal(document.activeElement.querySelector('[data-role="url"]').textContent, "https://example.test/2", "the next row");
  [b, c] = rows();
  c.focus();
  await key(c, { key: "Backspace" });
  assert.deepEqual(named(calls, "queue_remove").map(([, x]) => x.id), [1, 3]);
  assert.equal(document.activeElement, b, "no next row: the previous one");
});

test("#19: removing the only row returns focus to the URL bar; keys inside a row's controls are left alone", async () => {
  const calls = await mount([row(1, "queued")]);
  const download = [...rows()[0].querySelectorAll("button")].find((x) => x.textContent === "Download");
  download.focus();
  await key(download, { key: "Delete" });
  assert.equal(named(calls, "queue_remove").length, 0, "Delete on a button isn't the row's");
  rows()[0].focus();
  await key(rows()[0], { key: "Backspace" });
  assert.equal(named(calls, "queue_remove").length, 1);
  assert.equal(document.activeElement, field());
});

function deferred() {
  let resolve, reject;
  const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}

test("#19 review: arrows skip a row still being removed", async () => {
  const pending = deferred();
  await mount([row(1, "queued"), row(2, "queued")], { queue_remove: () => pending.promise });
  const [a, b] = rows();
  a.focus();
  await key(a, { key: "Delete" });
  assert.equal(document.activeElement, b);
  await key(b, { key: "ArrowUp" });
  assert.equal(document.activeElement, b, "not back onto the leaving row");
  await act(async () => pending.resolve(null));
  assert.equal(rows().length, 1);
  assert.equal(document.activeElement, rows()[0]);
});

test("#19 review: a row whose removal failed can be focused again", async () => {
  let first = true;
  await mount([row(1, "queued"), row(2, "queued")], {
    queue_remove: () => {
      if (first) {
        first = false;
        throw "It started downloading.";
      }
      return null;
    },
  });
  const [a, b] = rows();
  a.focus();
  await key(a, { key: "Delete" });
  await act(async () => {});
  assert.equal(rows().length, 2, "the failed removal kept the row");
  b.focus();
  await key(b, { key: "Delete" });
  await act(async () => {});
  assert.equal(document.activeElement, rows()[0], "focus goes to the surviving row, not the URL bar");
});
