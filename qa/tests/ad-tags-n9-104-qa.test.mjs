// Independent #104 / N9 QA: real React events and deferred/error Tauri replies. Adapted (not
// weakened) to the per-phrase commands after review round 2: each test keeps its intent.
import assert from "node:assert/strict";
import { after, afterEach, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { JSDOM } from "jsdom";
import React, { act } from "react";
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks";
import { createServer } from "vite";
import { tagStore } from "./ad-tags-mock.mjs";

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
  ({ default: Editor } = await server.ssrLoadModule("/src/components/AdTagEditor.tsx"));
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
const input = (label) => container.querySelector(`input[aria-label="${label}"]`);
const values = () => [...container.querySelectorAll('input[aria-label^="Phrase "]')].map((i) => i.value);
const reset = () => container.querySelector('[data-role="ad-tags-reset"]');
async function type(el, value) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, "value").set.call(el, value);
    el.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
  });
}
async function enter(el) {
  await act(async () => el.dispatchEvent(new dom.window.KeyboardEvent("keydown", { key: "Enter", bubbles: true })));
}

test("#104 QA: slow initial loading disables every mutation and unmount ignores a late read", async () => {
  const pending = deferred();
  const calls = [];
  await mount((cmd) => { calls.push(cmd); return pending.promise; });
  assert.ok([...container.querySelectorAll("input,button")].every((el) => el.disabled));
  await act(async () => reset().click());
  assert.deepEqual(calls, ["settings_ad_tags"]);
  await act(async () => root.unmount());
  root = undefined;
  await act(async () => pending.resolve({ version: 0, tags: ["late"] }));
  assert.equal(container.childElementCount, 0);
});

test("#104 QA: a pending reset blocks competing changes and trusts the returned starter list", async () => {
  const pending = deferred();
  const calls = [];
  await mount((cmd) => {
    if (cmd === "settings_ad_tags") return { version: 0, tags: ["Custom"] };
    calls.push(cmd);
    return pending.promise;
  });
  await act(async () => reset().click());
  assert.ok([...container.querySelectorAll("input,button")].every((el) => el.disabled));
  await act(async () => reset().click());
  assert.deepEqual(calls, ["settings_reset_ad_tags"]);
  await act(async () => pending.resolve({ version: 1, tags: ["Official Video", "4K", "MV"] }));
  assert.deepEqual(values(), ["Official Video", "4K", "MV"]);
  assert.equal(input("New phrase").disabled, false);
});

test("#104 QA: failed additions keep the draft for retry and success clears the error", async () => {
  const store = tagStore(["MV"]);
  let attempt = 0;
  await mount((cmd, args) => {
    if (cmd !== "settings_ad_tags" && ++attempt === 1) throw "disk full <b>retry</b>";
    return store.apply(cmd, args);
  });
  await type(input("New phrase"), "日本語の広告");
  await act(async () => container.querySelector("form").requestSubmit());
  assert.equal(input("New phrase").value, "日本語の広告");
  assert.deepEqual(values(), ["MV"]);
  assert.equal(container.querySelector('[role="alert"]').textContent, "disk full <b>retry</b>");
  assert.equal(container.querySelector('[role="alert"] b'), null);
  await act(async () => container.querySelector("form").requestSubmit());
  assert.deepEqual(values(), ["MV", "日本語の広告"]);
  assert.equal(input("New phrase").value, "");
  assert.equal(container.querySelector('[role="alert"]'), null);
});

test("#104 QA: failed edits keep the draft and can be retried with Enter", async () => {
  const store = tagStore(["MV"]);
  let attempt = 0;
  const calls = [];
  await mount((cmd, args) => {
    if (cmd !== "settings_ad_tags") {
      calls.push(args);
      if (++attempt === 1) throw "read-only settings";
    }
    return store.apply(cmd, args);
  });
  await type(input("Phrase 1"), "  (Clip) ");
  await enter(input("Phrase 1"));
  assert.equal(input("Phrase 1").value, "  (Clip) ");
  await enter(input("Phrase 1"));
  assert.deepEqual(calls, [{ old: "MV", new: "(Clip)" }, { old: "MV", new: "(Clip)" }]);
  assert.deepEqual(values(), ["Clip"]);
  assert.equal(container.querySelector('[role="alert"]'), null);
});

test("#104 QA: removing all phrases survives a fresh editor mount", async () => {
  const store = tagStore(["MV"]);
  const handler = (cmd, args) => store.apply(cmd, args);
  await mount(handler);
  await act(async () => container.querySelector('button[aria-label="Remove MV"]').click());
  assert.deepEqual(store.tags(), []);
  await act(async () => root.unmount());
  root = undefined;
  container.remove();
  await mount(handler);
  assert.deepEqual(values(), []);
  assert.ok(container.querySelector('[data-role="ad-tags-empty"]'));
});

test("#104 QA: malformed save replies show an error and keep the previous phrases", async () => {
  await mount((cmd) =>
    cmd === "settings_ad_tags" ? { version: 0, tags: ["MV", "4K"] } : { version: 1, tags: ["MV", 42] },
  );
  await act(async () => container.querySelector('button[aria-label="Remove 4K"]').click());
  assert.deepEqual(values(), ["MV", "4K"]);
  assert.match(container.querySelector('[role="alert"]').textContent, /couldn't read the saved list/);
  assert.equal(reset().disabled, false);
});

test("#104 QA: clicking Remove while an edited field blurs must still remove the phrase", async () => {
  const store = tagStore(["MV", "4K"]);
  await mount((cmd, args) => store.apply(cmd, args));
  const field = input("Phrase 1");
  const remove = container.querySelector('button[aria-label="Remove MV"]');
  await act(async () => {
    container.querySelector("details").open = true;
    field.focus();
  });
  await type(field, "Clip");
  // Browser order: pointer down, blur the input (focus moving to the ✕), then click the button.
  await act(async () => remove.dispatchEvent(new dom.window.MouseEvent("mousedown", { bubbles: true })));
  await act(async () => remove.focus());
  await act(async () => remove.click());
  assert.deepEqual(store.tags(), ["4K"], "the Remove click must not silently become only an edit");
});
