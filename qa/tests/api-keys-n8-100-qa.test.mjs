// Independent QA #100 part 3 / N8, D-036: each service kind has its own key, labelled with where it's sent.
// The label follows the saved base URL; while a service change is unsaved, the key field and
// its buttons wait, so a key always lands in the slot it's sent from. Real React DOM, mocked
// Tauri IPC. Then npm run build && npm test.
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
const EMPTY = { kind: "openai_compatible", baseUrl: "", model: "", keySet: false, keyHost: null, storeError: null };
const SAVED = { ...EMPTY, baseUrl: "https://api.example.test/v1", model: "small-model", keyHost: "api.example.test" };
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

async function choose(value) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(dom.window.HTMLSelectElement.prototype, "value").set.call(el("api-kind"), value);
    el("api-kind").dispatchEvent(new dom.window.Event("change", { bubbles: true }));
  });
}

const label = () => el("api-key-destination").textContent;

test("N8 #100 QA: a failed service save keeps key operations disabled and cannot test the unsaved host", async () => {
  const calls = await mount({
    settings_api: () => ({ ...SAVED, keySet: true }),
    settings_set_api: () => Promise.reject("Couldn't save your settings: permission denied"),
  });
  await type("api-base-url", "https://other.example.test/v1");
  await click("api-save");
  assert.match(el("api-error").textContent, /permission denied/);
  assert.equal(label(), "Save the service first. Each service keeps its own key.");
  for (const role of ["api-key", "api-key-save", "api-key-remove", "api-test"]) {
    assert.equal(el(role).disabled, true, role);
  }
  await click("api-key-remove");
  await click("api-test");
  assert.equal(calls.filter(([c]) => ["api_key_remove", "api_test_connection"].includes(c)).length, 0);
});

test("N8 #100 QA: model edits also lock the key and reverting restores the saved destination", async () => {
  await mount({ settings_api: () => ({ ...SAVED, keySet: true }) });
  await type("api-model", "different-model");
  assert.equal(el("api-key").disabled, true);
  assert.equal(el("api-key-remove").disabled, true);
  await type("api-model", `  ${SAVED.model}  `);
  assert.equal(label(), "Sent to api.example.test");
  assert.equal(el("api-key").disabled, false);
  assert.equal(el("api-key-remove").disabled, false);
});

test("N8 #100 QA: a pending service save cannot remove the previously saved kind's key", async () => {
  const pending = deferred();
  const calls = await mount({
    settings_api: () => ({ ...SAVED, keySet: true }),
    settings_set_api: () => pending.promise,
  });
  await choose("anthropic");
  await type("api-model", "qa-anthropic");
  await click("api-save");
  for (const role of ["api-kind", "api-key", "api-key-save", "api-key-remove", "api-test"]) {
    assert.equal(el(role).disabled, true, role);
  }
  await click("api-key-remove");
  assert.equal(calls.filter(([c]) => c === "api_key_remove").length, 0);
  await act(async () => pending.resolve({ ...SAVED, kind: "anthropic", model: "qa-anthropic", baseUrl: "https://api.anthropic.com", keyHost: "api.anthropic.com", keySet: false }));
  assert.equal(label(), "Sent to api.anthropic.com");
  assert.equal(el("api-key-status").textContent, "No key");
});

test("N8 #100 QA: saved host and port follow the backend's normalized result", async () => {
  await mount({
    settings_api: () => SAVED,
    settings_set_api: () => ({ ...SAVED, baseUrl: "http://[::1]:11434/v1", model: "qa-model", keyHost: "[::1]:11434" }),
  });
  await type("api-base-url", "http://[::1]:11434/v1/chat/completions/");
  await click("api-save");
  assert.equal(el("api-base-url").value, "http://[::1]:11434/v1");
  assert.equal(label(), "Sent to [::1]:11434");
  assert.equal(el("api-test").disabled, false);
});

test("N8 #100 QA: credential-store failure shows an error without claiming the new key was saved", async () => {
  const calls = await mount({
    settings_api: () => SAVED,
    api_key_set: () => Promise.reject("Couldn't reach your system's credential store."),
  });
  await type("api-key", KEY);
  await click("api-key-save");
  assert.match(el("api-error").textContent, /credential store/);
  assert.equal(el("api-key-status").textContent, "No key");
  assert.equal(el("api-key-remove").disabled, true);
  assert.equal(label(), "Sent to api.example.test");
  assert.equal(calls.filter(([c]) => c === "api_key_set").length, 1);
});


