// #91 / N8: the Service picker under Settings → AI naming → API: OpenAI-compatible or Anthropic.
// Switching fills in that service's address and clears the model; nothing is saved until Save,
// which sends the kind with the base URL and model. Real React DOM, mocked Tauri IPC. Then
// npm run build && npm test.
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

async function choose(value) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(dom.window.HTMLSelectElement.prototype, "value").set.call(el("api-kind"), value);
    el("api-kind").dispatchEvent(new dom.window.Event("change", { bubbles: true }));
  });
}

test("N8 #91: switching to Anthropic fills its address, clears the model and saves nothing yet", async () => {
  const calls = await mount({ settings_api: () => SAVED });
  assert.equal(el("api-kind").value, "openai_compatible");
  assert.equal(el("api-model").value, "small-model");
  await choose("anthropic");
  assert.equal(el("api-base-url").value, "https://api.anthropic.com");
  assert.equal(el("api-model").value, "");
  assert.equal(el("api-model").placeholder, "claude-haiku-5-5");
  assert.equal(el("api-key").placeholder, "Required for Anthropic");
  assert.equal(el("api-test").disabled, true, "unsaved changes can't be tested");
  assert.equal(calls.filter(([c]) => c === "settings_set_api").length, 0, "nothing saved yet");
  await choose("openai_compatible");
  assert.equal(el("api-base-url").value, "https://api.openai.com/v1");
  assert.equal(el("api-key").placeholder, "Optional for Ollama and LM Studio");
});

test("N8 #91: Save sends the kind with the base URL and model", async () => {
  const calls = await mount({
    settings_set_api: ({ kind, baseUrl, model }) => ({ ...EMPTY, kind, baseUrl, model }),
  });
  await choose("anthropic");
  await type("api-model", "claude-haiku-5-5");
  await click("api-save");
  const saved = calls.find(([c]) => c === "settings_set_api");
  assert.deepEqual(saved[1], { kind: "anthropic", baseUrl: "https://api.anthropic.com", model: "claude-haiku-5-5" });
  assert.equal(el("api-kind").value, "anthropic");
  assert.equal(el("api-save").disabled, true, "saved");
});

test("N8 #91: a saved Anthropic setup opens as Anthropic", async () => {
  await mount({
    settings_api: () => ({ ...EMPTY, kind: "anthropic", baseUrl: "https://api.anthropic.com", model: "claude-haiku-5-5", keySet: true }),
  });
  assert.equal(el("api-kind").value, "anthropic");
  assert.equal(el("api-key").placeholder, "Key saved. Paste a new one to replace it.");
  assert.equal(el("api-save").disabled, true);
});

test("N8 #91 QA: a failed service Save leaves the draft retryable and Test disabled", async () => {
  let fail = true;
  const calls = await mount({
    settings_api: () => SAVED,
    settings_set_api: ({ kind, baseUrl, model }) => {
      if (fail) throw new Error("Couldn't save settings.");
      return { ...EMPTY, kind, baseUrl, model };
    },
  });
  await choose("anthropic");
  await type("api-model", "custom-claude-model");
  await click("api-save");
  assert.match(el("api-error").textContent, /Couldn't save settings/);
  assert.equal(el("api-kind").value, "anthropic");
  assert.equal(el("api-model").value, "custom-claude-model");
  assert.equal(el("api-save").disabled, false);
  assert.equal(el("api-test").disabled, true);
  fail = false;
  await click("api-save");
  assert.equal(el("api-error"), null);
  assert.equal(el("api-test").disabled, false);
  assert.equal(calls.filter(([command]) => command === "settings_set_api").length, 2);
});

test("N8 #91 QA: an in-flight service Save locks the kind and connection controls", async () => {
  const pending = deferred();
  await mount({ settings_set_api: () => pending.promise });
  await choose("anthropic");
  await type("api-model", "custom-claude-model");
  await click("api-save");
  for (const role of ["api-kind", "api-base-url", "api-model", "api-save", "api-key", "api-test"]) {
    assert.equal(el(role).disabled, true, role);
  }
  await act(async () => pending.resolve({
    ...EMPTY, kind: "anthropic", baseUrl: "https://api.anthropic.com", model: "custom-claude-model",
  }));
  assert.equal(el("api-kind").disabled, false);
  assert.equal(el("api-test").disabled, false);
});

test("N8 #91 QA: Anthropic key Save, Test connection and Remove use the shared IPC safely", async () => {
  const setup = { ...EMPTY, kind: "anthropic", baseUrl: "https://api.anthropic.com", model: "custom-claude-model" };
  const calls = await mount({
    settings_api: () => setup,
    api_key_set: () => ({ ...setup, keySet: true }),
    api_key_remove: () => setup,
    naming_status: () => API_READY,
    api_test_connection: () => ({ ok: true, message: "Connected. It would name the sample “Me at the zoo”." }),
  });
  assert.equal(el("api-key").type, "password");
  await type("api-key", KEY);
  await click("api-key-save");
  assert.deepEqual(calls.find(([command]) => command === "api_key_set")[1], { key: KEY });
  assert.equal(el("api-key").value, "");
  assert.equal(container.innerHTML.includes(KEY), false);
  assert.equal(el("api-key-remove").disabled, false);
  await click("api-test");
  assert.match(el("api-test-result").textContent, /Connected/);
  assert.equal(calls.filter(([command]) => command === "api_test_connection").length, 1);
  await click("api-key-remove");
  assert.equal(el("api-key-remove").disabled, true);
  assert.equal(el("api-key").placeholder, "Required for Anthropic");
  assert.equal(el("api-test-result"), null);
});
