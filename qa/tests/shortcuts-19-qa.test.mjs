// Independent QA for #19 / PRD §7, Q1/Q8, Q4, Q6 and N1 (D-035).
// Real React DOM with mocked IPC; all three OS branches are simulated, not native webviews.
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
const platforms = [
  { os: "macos", platform: "MacIntel", mod: { metaKey: true }, other: { ctrlKey: true }, download: "⌘↩", remove: "⌫" },
  { os: "windows", platform: "Win32", mod: { ctrlKey: true }, other: { metaKey: true }, download: "Ctrl+Enter", remove: "Delete" },
  { os: "linux", platform: "Linux x86_64", mod: { ctrlKey: true }, other: { metaKey: true }, download: "Ctrl+Enter", remove: "Delete" },
];
before(async () => {
  const original = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  try {
    for (const platform of platforms) {
      Object.defineProperty(globalThis, "navigator", { configurable: true, value: { platform: platform.platform } });
      const server = await createServer({
        root: fileURLToPath(new URL("../../", import.meta.url)),
        configFile: false, logLevel: "silent", appType: "custom",
        server: { middlewareMode: true, hmr: false, watch: null },
      });
      servers.push(server);
      const { default: App } = await server.ssrLoadModule("/src/App.tsx");
      assert.equal((await server.ssrLoadModule("/src/platform.ts")).os, platform.os);
      apps.set(platform.os, App);
    }
  } finally {
    if (original) Object.defineProperty(globalThis, "navigator", original);
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

const row = (id, state = "queued", extra = {}) => ({
  id, url: `https://example.test/${id}`, status: { state }, rev: 1,
  meta: { title: "  日本語 🎵 <b>CON</b> : café  " }, attempt: 0, name: "日本語 – café", ...extra,
});
function deferred() {
  let resolve;
  const promise = new Promise((ok) => { resolve = ok; });
  return { promise, resolve };
}
async function mount(os, list = [row(1)], overrides = {}) {
  const calls = [];
  mockIPC((command, args) => {
    calls.push([command, args]);
    if (command in overrides) return overrides[command](args);
    switch (command) {
      case "queue_list": return list;
      case "queue_runner_state": return false;
      case "settings_download_folder": return { path: "/qa/downloads", error: null };
      case "settings_naming_template": return "title";
      case "naming_previews": return [];
      case "naming_status": return { provider: "off", unavailable: "off" };
      case "settings_naming_provider": return "off";
      case "settings_format": return "default";
      case "queue_add": return row(100 + calls.length, "fetching", { url: args.url, name: null, meta: null });
      case "queue_rename": return row(args.id, "queued", { name: args.name, rev: 2 });
      default: return null;
    }
  }, { shouldMockEvents: true });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(apps.get(os))));
  return calls;
}
const field = () => container.querySelector('input[aria-label="Link to download"]');
const rows = () => [...container.querySelectorAll("li")];
const named = (calls, name) => calls.filter(([command]) => command === name);
const allButton = () => [...container.querySelectorAll("button")].find((button) => /Download all|Downloading all/.test(button.textContent));
async function key(target, init) {
  const event = new dom.window.KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
  await act(async () => target.dispatchEvent(event));
  return event;
}
async function paste(target, text) {
  const event = new dom.window.Event("paste", { bubbles: true, cancelable: true });
  event.clipboardData = { getData: (type) => type === "text/plain" ? text : "" };
  await act(async () => target.dispatchEvent(event));
  return event;
}
async function type(input, value) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, "value").set.call(input, value);
    input.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
  });
}
async function editName(value = "  日本語 – café <b>CON</b>  ") {
  await act(async () => container.querySelector('[data-role="name"]').click());
  const input = container.querySelector('input[aria-label="File name"]');
  await type(input, value);
  return input;
}

for (const p of platforms) {
  test(`#19 QA ${p.os}: runtime modifier and tooltips match the OS; typed URL is not added`, async () => {
    const calls = await mount(p.os);
    assert.equal(document.activeElement, field());
    assert.equal(allButton().title, `Download all (${p.download})`);
    assert.equal(container.querySelector('[aria-label="Remove"]').title, `Remove from queue (${p.remove})`);
    await type(field(), "https://example.test/unsubmitted");
    await key(field(), { key: "Enter", ...p.other });
    assert.equal(named(calls, "queue_run_all").length, 0);
    const event = await key(field(), { key: "Enter", ...p.mod });
    assert.equal(event.defaultPrevented, true);
    assert.equal(named(calls, "queue_run_all").length, 1);
    assert.equal(named(calls, "queue_add").length, 0);
    assert.equal(field().value, "https://example.test/unsubmitted");
  });

  test(`#19 QA ${p.os}: waits for the asynchronous Unicode rename reply before starting`, async () => {
    const reply = deferred();
    const calls = await mount(p.os, [row(1)], { queue_rename: () => reply.promise });
    const input = await editName();
    await key(input, { key: "Enter", ...p.mod });
    assert.equal(named(calls, "queue_rename").length, 1);
    assert.equal(named(calls, "queue_rename")[0][1].name, "日本語 – café <b>CON</b>");
    assert.equal(named(calls, "queue_run_all").length, 0, "issuing rename is not enough: wait for its reply");
    await act(async () => reply.resolve(row(1, "queued", { name: "日本語 – café CON", rev: 2 })));
    assert.equal(named(calls, "queue_run_all").length, 1);
    assert.equal(container.querySelector('[data-role="name"]').textContent, "日本語 – café CON");
    assert.equal(rows()[0].querySelector("b"), null, "titles and names remain literal text");
  });

  test(`#19 QA ${p.os}: runner events gate the shortcut and re-enable it`, async () => {
    const calls = await mount(p.os, [row(1)], { queue_runner_state: () => true });
    assert.equal(allButton().disabled, true);
    await key(field(), { key: "Enter", ...p.mod });
    assert.equal(named(calls, "queue_run_all").length, 0);
    await act(async () => emit("queue://runner", { running: false }));
    assert.equal(allButton().disabled, false);
    await key(rows()[0], { key: "Enter", ...p.mod });
    assert.equal(named(calls, "queue_run_all").length, 1);
    await act(async () => emit("queue://runner", { running: true }));
    await key(field(), { key: "Enter", ...p.mod });
    assert.equal(named(calls, "queue_run_all").length, 1);
  });

  test(`#19 QA ${p.os}: multiline paste preserves an existing draft and handles CRLF, blanks and bad lines`, async () => {
    const calls = await mount(p.os, [], {
      queue_add: ({ url }) => {
        if (url === "not a link 日本語 <b>x</b>") throw { kind: "invalid", message: "That isn't a link." };
        return row(url.endsWith("a") ? 10 : 11, "fetching", { url });
      },
    });
    await type(field(), "https://example.test/draft");
    const event = await paste(container.querySelector('[aria-label="Settings"]'), "\r\n https://example.test/a\r\n\r\nnot a link 日本語 <b>x</b>\rhttps://example.test/b\n");
    assert.equal(event.defaultPrevented, true);
    assert.deepEqual(named(calls, "queue_add").map(([, args]) => args.url), ["https://example.test/a", "not a link 日本語 <b>x</b>", "https://example.test/b"]);
    assert.equal(rows().length, 2);
    assert.equal(field().value, "https://example.test/draft");
    assert.match(container.querySelector('[role="alert"]').textContent, /not a link 日本語 <b>x<\/b>/);
    assert.equal(container.querySelector("b"), null);
  });

  test(`#19 QA ${p.os}: one-line paste replaces a URL draft; blank and non-text paste leave it alone`, async () => {
    const calls = await mount(p.os);
    await type(field(), "old draft");
    rows()[0].focus();
    assert.equal((await paste(rows()[0], " \r\n\t ")).defaultPrevented, false);
    assert.equal(field().value, "old draft");
    assert.equal(document.activeElement, rows()[0]);
    assert.equal((await paste(rows()[0], "")).defaultPrevented, false);
    assert.equal((await paste(rows()[0], "  https://example.test/日本語\n\n ")).defaultPrevented, true);
    assert.equal(field().value, "https://example.test/日本語");
    assert.equal(document.activeElement, field());
    assert.equal(named(calls, "queue_add").length, 0);
  });

  test(`#19 QA ${p.os}: Settings blocks outside paste and Download all, then closing restores shortcuts`, async () => {
    const calls = await mount(p.os);
    await act(async () => container.querySelector('[aria-label="Settings"]').click());
    const panel = container.querySelector('[role="dialog"]');
    assert.ok(panel);
    const event = await paste(panel, "https://example.test/a\nhttps://example.test/b");
    assert.equal(event.defaultPrevented, false);
    await key(document.activeElement, { key: "Enter", ...p.mod });
    assert.equal(named(calls, "queue_run_all").length, 0);
    assert.equal(named(calls, "queue_add").length, 0);
    assert.equal(field().value, "");
    await key(document.activeElement, { key: "Escape" });
    assert.equal(container.querySelector('[role="dialog"]'), null);
    await key(field(), { key: "Enter", ...p.mod });
    assert.equal(named(calls, "queue_run_all").length, 1);
  });

  test(`#19 QA ${p.os}: processing/downloading rows reject both removal keys, including after a state event`, async () => {
    const calls = await mount(p.os, [row(1), row(2, "processing"), row(3, "downloading")]);
    for (const r of rows().slice(1)) {
      r.focus();
      for (const k of ["Delete", "Backspace"]) await key(r, { key: k });
      assert.equal(document.activeElement, r);
    }
    await act(async () => emit("queue://item", row(1, "processing", { rev: 2 })));
    rows()[0].focus();
    await key(rows()[0], { key: "Delete" });
    assert.equal(named(calls, "queue_remove").length, 0);
    assert.equal(rows().length, 3);
    await key(rows()[0], { key: "ArrowUp" });
    assert.equal(document.activeElement, rows()[0]);
    await key(rows()[0], { key: "ArrowDown" });
    assert.equal(document.activeElement, rows()[1]);
  });

  test(`#19 QA ${p.os}: arrows/removal/Escape belong to row buttons and text fields, not the row`, async () => {
    const calls = await mount(p.os, [row(1), row(2)]);
    const button = rows()[0].querySelector('[aria-label="Remove"]');
    for (const target of [button, field()]) {
      target.focus();
      for (const k of ["ArrowUp", "ArrowDown", "Delete", "Backspace", "Escape"]) {
        assert.equal((await key(target, { key: k })).defaultPrevented, false);
        assert.equal(document.activeElement, target);
      }
    }
    const input = await editName("日本語");
    for (const k of ["ArrowUp", "ArrowDown", "Delete", "Backspace"]) {
      assert.equal((await key(input, { key: k })).defaultPrevented, false);
      assert.equal(document.activeElement, input);
    }
    await key(input, { key: "Escape" });
    assert.equal(named(calls, "queue_remove").length, 0);
    assert.equal(named(calls, "queue_rename").length, 0, "Escape cancels the name edit instead");
    assert.equal(container.querySelector('input[aria-label="File name"]'), null);
    assert.equal(named(calls, "queue_run_all").length, 0);
  });

  test(`#19 QA ${p.os}: IME composition Enter leaves the Unicode name editor open`, async () => {
    const calls = await mount(p.os);
    const input = await editName("日本語");
    await key(input, { key: "Enter", ...p.mod, isComposing: true, keyCode: 229 });
    assert.equal(document.activeElement, input);
    assert.equal(named(calls, "queue_rename").length, 0);
    assert.equal(named(calls, "queue_run_all").length, 0);
  });

  test(`#19 QA ${p.os}: IME keyCode 229 without isComposing also leaves the name editor open`, async () => {
    const calls = await mount(p.os);
    const input = await editName("日本語");
    await key(input, { key: "Enter", ...p.mod, isComposing: false, keyCode: 229 });
    assert.equal(named(calls, "queue_run_all").length, 0, "IME candidate confirmation must not start a download");
    assert.equal(named(calls, "queue_rename").length, 0);
    assert.equal(document.activeElement, input);
  });

  test(`#19 QA ${p.os}: rapid removal of both rows with pending IPC returns focus to the URL bar`, async () => {
    const first = deferred(), second = deferred();
    const calls = await mount(p.os, [row(1), row(2)], {
      queue_remove: ({ id }) => id === 1 ? first.promise : second.promise,
    });
    rows()[0].focus();
    await key(document.activeElement, { key: "Delete" });
    assert.equal(document.activeElement, rows()[1]);
    await key(document.activeElement, { key: "Backspace" });
    assert.deepEqual(named(calls, "queue_remove").map(([, args]) => args.id), [1, 2]);
    await act(async () => { first.resolve(); second.resolve(); });
    assert.equal(rows().length, 0);
    assert.equal(document.activeElement === field(), true, "no surviving row: focus belongs in the URL bar");
  });
}
