// Independent #32(a) / N8 QA: the real API setup component, with mocked Tauri commands.
import assert from "node:assert/strict";
import { after, afterEach, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { JSDOM } from "jsdom";
import React, { act } from "react";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { createServer } from "vite";

const dom = new JSDOM("<!doctype html><html><body></body></html>", { url: "http://localhost/" });
globalThis.window = dom.window;
globalThis.document = dom.window.document;
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const { createRoot } = await import("react-dom/client");
let server, Component, root, container;
const SAVED = { kind: "openai_compatible", baseUrl: "https://api.example.test/v1", model: "model", keySet: false, storeError: null };

before(async () => {
  server = await createServer({
    root: fileURLToPath(new URL("../../", import.meta.url)),
    configFile: false, logLevel: "silent", appType: "custom",
    server: { middlewareMode: true, hmr: false, watch: null },
  });
  ({ default: Component } = await server.ssrLoadModule("/src/components/ApiSetup.tsx"));
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

async function mount(overrides = {}) {
  const calls = [];
  mockIPC((command, args) => {
    calls.push([command, args]);
    if (command in overrides) return overrides[command](args);
    if (command === "settings_api") return SAVED;
    if (command === "naming_status") return { provider: "api", unavailable: null };
    return null;
  });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(Component)));
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

test("N8 QA: saving a normalized endpoint updates the field and enables Test connection", async () => {
  const calls = await mount({ settings_set_api: () => SAVED });
  await type("api-base-url", "https://api.example.test/v1/chat/completions/");
  assert.equal(el("api-test").disabled, true);
  await click("api-save");
  assert.equal(calls.filter(([command]) => command === "settings_set_api").length, 1);
  assert.deepEqual({
    baseUrl: el("api-base-url").value,
    saveDisabled: el("api-save").disabled,
    testDisabled: el("api-test").disabled,
  }, {
    baseUrl: SAVED.baseUrl,
    saveDisabled: true,
    testDisabled: false,
  }, "show the canonical saved URL, clear the dirty state, and allow Test connection");
});

test("N8 QA: unsaved service edits block a test until they are reverted", async () => {
  const calls = await mount();
  await type("api-model", "another-model");
  assert.equal(el("api-test").disabled, true);
  await click("api-test");
  assert.equal(calls.filter(([command]) => command === "api_test_connection").length, 0);
  await type("api-model", SAVED.model);
  assert.equal(el("api-test").disabled, false);
  assert.equal(el("api-save").disabled, true);
});

test("N8 QA: failed key removal retains the saved-key state and allows retry", async () => {
  let tries = 0;
  await mount({
    settings_api: () => ({ ...SAVED, keySet: true }),
    api_key_remove: () => {
      if (++tries === 1) return Promise.reject("Couldn't reach your system's credential store.");
      return SAVED;
    },
  });
  await click("api-key-remove");
  assert.equal(el("api-key-status").textContent, "Key saved");
  assert.match(el("api-error").textContent, /credential store/);
  assert.equal(el("api-key-remove").disabled, false);
  await click("api-key-remove");
  assert.equal(el("api-key-status").textContent, "No key");
  assert.equal(el("api-error"), null);
  assert.equal(el("api-key-remove").disabled, true);
});

test("N8 QA: an IPC test failure is literal text and the test can be retried", async () => {
  let tries = 0;
  await mount({ api_test_connection: () => {
    if (++tries === 1) return Promise.reject("<b>Connection stopped</b>");
    return { ok: true, message: "Connected." };
  } });
  await click("api-test");
  assert.equal(el("api-test-result").textContent, "<b>Connection stopped</b>");
  assert.equal(el("api-test-result").children.length, 0);
  assert.equal(el("api-test").disabled, false);
  await click("api-test");
  assert.equal(el("api-test-result").textContent, "Connected.");
});

test("N8 QA: failed setup loading disables actions and displays the reason", async () => {
  await mount({ settings_api: () => Promise.reject("Settings couldn't be read.") });
  assert.equal(el("api-error").textContent, "Settings couldn't be read.");
  for (const role of ["api-base-url", "api-model", "api-save", "api-key", "api-key-save", "api-key-remove", "api-test"]) {
    assert.equal(el(role).disabled, true, role);
  }
});
