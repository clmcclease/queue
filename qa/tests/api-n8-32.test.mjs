// #32 / N8: the API service's setup under Settings → AI naming: base URL and model, a write-only
// key, Test connection, and what is sent. Real React DOM, mocked Tauri IPC. Then npm run build &&
// npm test.
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

const KEY = "sk-secret-32-never-shown";
const EMPTY = { kind: "openai_compatible", baseUrl: "", model: "", keySet: false, storeError: null };
const SAVED = { ...EMPTY, baseUrl: "https://api.example.test/v1", model: "small-model" };
const API_NOT_SET_UP = { provider: "api", unavailable: "not_set_up" };
const API_READY = { provider: "api", unavailable: null };

function deferred() {
  let resolve, reject;
  const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}

// The backend as the tests need it; `overrides` replace single commands. Every call is recorded.
async function mount(overrides = {}) {
  const calls = [];
  mockIPC((command, args) => {
    calls.push([command, args]);
    if (command in overrides) return overrides[command](args);
    switch (command) {
      case "queue_runner_state": return false;
      case "settings_download_folder": return { path: "/Users/me/Downloads", error: null };
      case "settings_format": return "default";
      case "queue_storage_problem": return null;
      case "queue_list": return [];
      case "settings_naming_template": return "title";
      case "naming_previews": return [];
      case "settings_naming_provider": return "api";
      case "naming_status": return API_NOT_SET_UP;
      case "settings_api": return EMPTY;
      default: return null;
    }
  }, { shouldMockEvents: true });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(App)));
  await act(async () => container.querySelector('button[aria-label="Settings"]').click());
  return calls;
}

const el = (role) => container.querySelector(`[data-role="${role}"]`);
async function type(role, value) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, "value").set.call(el(role), value);
    el(role).dispatchEvent(new dom.window.Event("input", { bubbles: true }));
  });
}
const click = (role) => act(async () => el(role).click());

test("N8: the API setup lists exactly what is sent, and nothing can be tested until it's saved", async () => {
  await mount();
  assert.ok(el("api-setup"), "shown under the chosen API provider");
  const sent = [...el("api-sent").querySelectorAll("li")].map((li) => li.textContent);
  assert.deepEqual(sent, [
    "Title", "Channel", "Upload date", "Artist", "Track", "Series", "Season number", "Episode number", "Description",
  ]);
  assert.equal(el("api-key-status").textContent, "No key");
  assert.equal(el("api-test").disabled, true);
  assert.equal(el("api-save").disabled, true, "nothing changed yet");
  assert.equal(el("api-key-remove").disabled, true);
});

test("N8: the API setup isn't shown for Local or Off", async () => {
  await mount({ settings_naming_provider: () => "local", naming_status: () => ({ provider: "local", unavailable: "model_missing" }) });
  assert.equal(el("api-setup"), null);
});

test("N8: saving the base URL and model sends them, refreshes the status, and enables Test", async () => {
  let ready = false;
  const calls = await mount({
    settings_set_api: ({ baseUrl, model }) => { ready = true; return { ...EMPTY, baseUrl, model }; },
    naming_status: () => (ready ? API_READY : API_NOT_SET_UP),
  });
  await type("api-base-url", "https://api.example.test/v1");
  await type("api-model", "small-model");
  await click("api-save");
  const saved = calls.find(([c]) => c === "settings_set_api");
  assert.deepEqual(saved[1], { baseUrl: "https://api.example.test/v1", model: "small-model" });
  assert.equal(el("api-test").disabled, false);
  assert.equal(el("provider-note"), null, "the not-set-up note goes once the status says it's ready");
});

test("N8: a refused base URL shows the backend's reason literally and keeps the typed text", async () => {
  await mount({
    settings_set_api: () => Promise.reject("The naming service must use https:// (or http:// on this computer). <b>x</b>"),
  });
  await type("api-base-url", "http://example.com/v1");
  await type("api-model", "m");
  await click("api-save");
  assert.equal(el("api-error").textContent, "The naming service must use https:// (or http:// on this computer). <b>x</b>");
  assert.equal(el("api-error").children.length, 0);
  assert.equal(el("api-base-url").value, "http://example.com/v1");
});

test("N8: the key is write-only: saved, the field is cleared, and it never appears again", async () => {
  let keySet = false;
  const calls = await mount({
    settings_api: () => ({ ...SAVED, keySet }),
    api_key_set: ({ key }) => { keySet = true; return { ...SAVED, keySet: key.length > 0 }; },
    api_key_remove: () => { keySet = false; return { ...SAVED, keySet: false }; },
  });
  await type("api-key", KEY);
  assert.equal(el("api-key").type, "password");
  await click("api-key-save");
  assert.deepEqual(calls.find(([c]) => c === "api_key_set")[1], { key: KEY });
  assert.equal(el("api-key").value, "", "cleared after saving");
  assert.equal(el("api-key-status").textContent, "Key saved");
  assert.ok(!container.innerHTML.includes(KEY), "the key isn't anywhere on the page");
  await click("api-key-remove");
  assert.equal(el("api-key-status").textContent, "No key");
});

test("N8: Test connection shows its result and can't be started twice", async () => {
  const pending = deferred();
  let tests = 0;
  await mount({
    settings_api: () => SAVED,
    naming_status: () => API_READY,
    api_test_connection: () => { tests++; return pending.promise; },
  });
  await click("api-test");
  assert.equal(el("api-test").disabled, true, "disabled while it runs");
  await click("api-test");
  assert.equal(tests, 1);
  await act(async () => pending.resolve({ ok: false, message: "The naming service answered with HTTP 401. The service refused the key." }));
  assert.equal(el("api-test-result").textContent, "The naming service answered with HTTP 401. The service refused the key.");
  assert.equal(el("api-test").disabled, false);
});

test("N8: an unreachable credential store is said plainly, not hidden", async () => {
  await mount({
    settings_api: () => ({ ...SAVED, storeError: "Couldn't reach your system's credential store." }),
    naming_status: () => ({ provider: "api", unavailable: "store_unreachable" }),
  });
  assert.equal(el("api-key-status").textContent, "Couldn't reach your system's credential store.");
  assert.match(el("provider-note").textContent, /credential store/);
});

test("N8 review: a late status reply from the API setup can't undo a switch to Local", async () => {
  const late = deferred();
  let statusCalls = 0;
  await mount({
    settings_api: () => ({ ...SAVED, keySet: true }),
    api_key_remove: () => ({ ...SAVED, keySet: false }),
    naming_status: () => (++statusCalls === 1 ? API_READY : late.promise),
    settings_set_naming_provider: ({ provider }) => {
      assert.equal(provider, "local");
      return { provider: "local", unavailable: "model_missing" };
    },
  });
  await click("api-key-remove"); // asks for a fresh status, which is slow to come back
  const local = [...container.querySelectorAll('[data-role="provider"] input[type="radio"]')].find((r) => r.value === "local");
  await act(async () => local.click());
  assert.equal(el("api-setup"), null, "the API setup is gone");
  assert.ok(el("naming-banner"), "Local's model isn't downloaded");
  await act(async () => late.resolve(API_READY));
  assert.ok(el("naming-banner"), "the older API reply doesn't hide the banner");
});
