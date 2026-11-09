// #100 part 3 / N8, D-036: each service kind has its own key, labelled with where it's sent.
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

test("N8 #100: the key field says where the saved key is sent", async () => {
  await mount({ settings_api: () => ({ ...SAVED, keySet: true }) });
  assert.equal(label(), "Sent to api.example.test");
  assert.equal(el("api-key").disabled, false);
  assert.equal(el("api-key-remove").disabled, false);
});

test("N8 #100: with no base URL saved, the key isn't sent anywhere", async () => {
  await mount();
  assert.equal(label(), "Not sent anywhere until a base URL is saved.");
});

test("N8 #100: an unsaved service change holds the key field until Save", async () => {
  let saved = { ...SAVED, keySet: true };
  const calls = await mount({
    settings_api: () => saved,
    // Anthropic's slot is empty: the backend answers for the newly saved kind's own slot.
    settings_set_api: ({ kind, baseUrl, model }) => (saved = { ...EMPTY, kind, baseUrl, model, keySet: false, keyHost: "api.anthropic.com" }),
  });
  await choose("anthropic");
  assert.equal(label(), "Save the service first. Each service keeps its own key.");
  assert.equal(el("api-key").disabled, true);
  assert.equal(el("api-key-save").disabled, true);
  assert.equal(el("api-key-remove").disabled, true, "the other kind's key can't be removed from here");
  await type("api-model", "claude-haiku-5-5");
  await click("api-save");
  assert.equal(label(), "Sent to api.anthropic.com");
  assert.equal(el("api-key-status").textContent, "No key", "the OpenAI-compatible key isn't carried over");
  assert.equal(el("api-key").disabled, false);
  await type("api-key", KEY);
  assert.equal(el("api-key-save").disabled, false);
  assert.equal(calls.filter(([c]) => c === "api_key_set").length, 0);
});

test("N8 #100: editing the base URL holds the key until the new address is saved", async () => {
  await mount({ settings_api: () => ({ ...SAVED, keySet: true }) });
  await type("api-base-url", "https://other.example.test/v1");
  assert.equal(label(), "Save the service first. Each service keeps its own key.");
  assert.equal(el("api-key-save").disabled, true);
  await type("api-base-url", "https://api.example.test/v1");
  assert.equal(label(), "Sent to api.example.test", "back to the saved address");
});

test("N8 #100: the key never reaches the window", async () => {
  const calls = await mount({ settings_api: () => ({ ...SAVED, keySet: true }), api_key_set: () => ({ ...SAVED, keySet: true }) });
  await type("api-key", KEY);
  await click("api-key-save");
  assert.equal(el("api-key").value, "", "cleared after saving");
  assert.ok(!container.innerHTML.includes(KEY));
  assert.deepEqual(calls.find(([c]) => c === "api_key_set")[1], { key: KEY });
});

for (const [from, to, host] of [["openai_compatible", "anthropic", "api.anthropic.com"], ["anthropic", "openai_compatible", "api.openai.com"]]) {
  test(`N8 #100: a key typed for ${from} can't be saved into ${to}'s slot`, async () => {
    let saved = { ...SAVED, kind: from };
    const calls = await mount({
      settings_api: () => saved,
      settings_set_api: ({ kind, baseUrl, model }) => (saved = { ...EMPTY, kind, baseUrl, model, keyHost: host }),
    });
    await type("api-key", KEY);
    await choose(to);
    assert.equal(el("api-key").value, "", "the draft is dropped when the service changes");
    await type("api-model", "some-model");
    await click("api-save");
    assert.equal(el("api-key").value, "");
    assert.equal(el("api-key-save").disabled, true, "nothing to save");
    assert.equal(calls.filter(([c]) => c === "api_key_set").length, 0);
  });
}

test("N8 #100: saving the service drops a key typed before it", async () => {
  const calls = await mount({ settings_api: () => SAVED, settings_set_api: ({ kind, baseUrl, model }) => ({ ...SAVED, kind, baseUrl, model }) });
  await type("api-key", KEY);
  await type("api-model", "other-model");
  await click("api-save");
  assert.equal(el("api-key").value, "");
  assert.equal(calls.filter(([c]) => c === "api_key_set").length, 0);
});
