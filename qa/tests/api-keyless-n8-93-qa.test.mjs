// Independent #93 / N8 / D-035: the API service's setup under Settings → AI naming: base URL and model, a write-only
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


const UNREACHABLE = "Couldn't reach your system's credential store.";
const KEYLESS = { provider: "api", unavailable: null, store_unreachable: true };
const BROKEN = { ...SAVED, storeError: UNREACHABLE };

test("#93 QA: a keyless setup still enables Test and preserves the store note after a refused request", async () => {
  const message = `The naming service answered with HTTP 401. The service refused the key. ${UNREACHABLE} No key was sent.`;
  const calls = await mount({
    settings_api: () => BROKEN,
    naming_status: () => KEYLESS,
    api_test_connection: () => ({ ok: false, message }),
  });
  assert.equal(el("api-test").disabled, false);
  assert.equal(el("api-key-status").textContent, UNREACHABLE);
  assert.match(el("provider-note").textContent, /asked without a key/);
  await click("api-test");
  assert.equal(el("api-test-result").textContent, message);
  assert.match(el("provider-note").textContent, /credential store/);
  assert.equal(calls.filter(([c]) => c === "api_test_connection").length, 1);
});

test("#93 QA: the keyless success message and warning are both visible", async () => {
  const message = `Connected. It would name the sample “Me at the zoo”. ${UNREACHABLE} No key was sent.`;
  await mount({
    settings_api: () => BROKEN,
    naming_status: () => KEYLESS,
    api_test_connection: () => ({ ok: true, message }),
  });
  await click("api-test");
  assert.equal(el("api-test-result").textContent, message);
  assert.match(el("provider-note").textContent, /asked without a key/);
});

test("#93 QA: Anthropic's store failure says rules rather than claiming a keyless request", async () => {
  await mount({
    settings_api: () => ({ ...BROKEN, kind: "anthropic" }),
    naming_status: () => ({ provider: "api", unavailable: "store_unreachable", store_unreachable: true }),
    api_test_connection: () => ({ ok: false, message: UNREACHABLE }),
  });
  assert.match(el("provider-note").textContent, /Names come from the rules/);
  assert.doesNotMatch(el("provider-note").textContent, /asked without a key/);
  await click("api-test");
  assert.equal(el("api-test-result").textContent, UNREACHABLE);
});

test("#93 QA: a recovered store clears the provider warning after a settings refresh", async () => {
  let recovered = false;
  await mount({
    settings_api: () => BROKEN,
    naming_status: () => recovered ? { ...KEYLESS, store_unreachable: false } : KEYLESS,
    api_key_set: () => { recovered = true; return { ...SAVED, keySet: true }; },
  });
  assert.match(el("provider-note").textContent, /credential store/);
  await type("api-key", KEY);
  await click("api-key-save");
  assert.equal(el("provider-note"), null);
  assert.equal(el("api-key-status").textContent, "Key saved");
  assert.equal(el("api-key").value, "");
});
