// Independent #109 / N8 QA: real API setup, mocked Tauri IPC, no real keys or network.
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
const EMPTY = { kind: "openai_compatible", baseUrl: "", model: "", keySet: false, storeError: null };
const DEFAULTS = [
  ["openai_compatible", "https://api.openai.com/v1"],
  ["anthropic", "https://api.anthropic.com"],
];

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
    if (command === "settings_api") return { ...EMPTY };
    if (command === "naming_status") return { provider: "api", unavailable: null };
    throw new Error(`Unexpected IPC: ${command}`);
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
async function choose(kind) {
  await act(async () => {
    el("api-kind").value = kind;
    el("api-kind").dispatchEvent(new dom.window.Event("change", { bubbles: true }));
  });
}
const click = (role) => act(async () => el(role).click());

for (const [kind, baseUrl] of DEFAULTS) {
  test(`#109 QA: ${kind} fills its default without saving or sending a request`, async () => {
    const calls = await mount({ settings_api: () => ({ ...EMPTY, kind }) });
    assert.equal(el("api-kind").value, kind);
    assert.equal(el("api-base-url").value, baseUrl);
    assert.equal(el("api-base-url").getAttribute("placeholder"), null);
    assert.equal(el("api-save").disabled, true);
    assert.equal(el("api-test").disabled, true);
    assert.deepEqual(calls.map(([command]) => command), ["settings_api"]);
    await type("api-model", " \t ");
    assert.equal(el("api-save").disabled, true, "whitespace isn't a model change");
    await type("api-model", "custom-model");
    assert.equal(el("api-save").disabled, false);
    await type("api-model", "");
    assert.equal(el("api-save").disabled, true, "reverting the model restores the clean state");
  });

  test(`#109 QA: ${kind} saves the filled default with a model and survives reopening`, async () => {
    let stored = { ...EMPTY, kind };
    const calls = await mount({
      settings_api: () => ({ ...stored }),
      settings_set_api: (args) => {
        stored = { ...stored, ...args, model: args.model.trim() };
        return { ...stored };
      },
      api_test_connection: () => ({ ok: true, message: "Connected." }),
    });
    await type("api-model", "  modèle-日本語  ");
    assert.equal(el("api-test").disabled, true, "the visible default hasn't been saved yet");
    await click("api-save");
    assert.deepEqual(calls.find(([command]) => command === "settings_set_api")[1], {
      kind, baseUrl, model: "  modèle-日本語  ",
    });
    assert.equal(el("api-model").value, "modèle-日本語");
    assert.equal(el("api-save").disabled, true);
    assert.equal(el("api-test").disabled, false);
    await click("api-test");
    assert.equal(el("api-test-result").textContent, "Connected.");
    await act(async () => root.unmount());
    root = createRoot(container);
    await act(async () => root.render(React.createElement(Component)));
    assert.equal(el("api-base-url").value, baseUrl);
    assert.equal(el("api-model").value, "modèle-日本語");
    assert.equal(el("api-save").disabled, true);
    assert.equal(el("api-test").disabled, false);
    assert.equal(calls.filter(([command]) => command === "settings_set_api").length, 1);
  });
}

for (const [kind, baseUrl] of [
  ["openai_compatible", "http://localhost:11434/v1"],
  ["openai_compatible", "http://127.0.0.1:1234/v1"],
  ["openai_compatible", "http://[::1]:11434/v1"],
  ["anthropic", "https://gateway.example.test/日本語/proxy/v1"],
]) {
  test(`#109 QA: preserve the saved ${kind} URL ${baseUrl} through a model-only save`, async () => {
    const stored = { ...EMPTY, kind, baseUrl, model: "old-model" };
    const calls = await mount({
      settings_api: () => stored,
      settings_set_api: (args) => ({ ...stored, ...args }),
    });
    assert.equal(el("api-base-url").value, baseUrl);
    assert.equal(el("api-save").disabled, true);
    await type("api-model", "new-model");
    await click("api-save");
    assert.deepEqual(calls.find(([command]) => command === "settings_set_api")[1], {
      kind, baseUrl, model: "new-model",
    });
    assert.equal(el("api-base-url").value, baseUrl);
    assert.equal(el("api-test").disabled, false);
  });
}

test("#109 QA: saving a key first neither persists the default nor loses it for the model save", async () => {
  let stored = { ...EMPTY };
  const calls = await mount({
    api_key_set: () => (stored = { ...stored, keySet: true }),
    settings_set_api: (args) => (stored = { ...stored, ...args }),
  });
  await type("api-key", "sk-ant-qa-fake-key");
  await click("api-key-save");
  assert.equal(el("api-base-url").value, "https://api.openai.com/v1");
  assert.equal(el("api-kind").value, "openai_compatible");
  assert.equal(el("api-key").value, "");
  assert.equal(el("api-save").disabled, true);
  assert.equal(el("api-test").disabled, true);
  assert.equal(calls.filter(([command]) => command === "settings_set_api").length, 0);
  await type("api-model", "model");
  await click("api-save");
  assert.equal(stored.baseUrl, "https://api.openai.com/v1");
  assert.equal(stored.model, "model");
  assert.equal(stored.keySet, true);
  assert.equal(el("api-test").disabled, false);
});

test("#109 QA: a typed custom URL survives a key save and reaches the service save", async () => {
  let stored = { ...EMPTY };
  const calls = await mount({
    api_key_set: () => (stored = { ...stored, keySet: true }),
    settings_set_api: (args) => (stored = { ...stored, ...args }),
  });
  await type("api-base-url", "https://openrouter.ai/api/v1");
  await type("api-key", "sk-qa-fake-key");
  await click("api-key-save");
  assert.equal(el("api-base-url").value, "https://openrouter.ai/api/v1");
  assert.equal(el("api-test").disabled, true);
  await type("api-model", "provider/model");
  await click("api-save");
  assert.deepEqual(calls.find(([command]) => command === "settings_set_api")[1], {
    kind: "openai_compatible", baseUrl: "https://openrouter.ai/api/v1", model: "provider/model",
  });
});

test("#109 QA: a failed default-URL save retains the draft and permits retry", async () => {
  let attempts = 0;
  await mount({ settings_set_api: (args) => {
    if (++attempts === 1) return Promise.reject("Couldn't save your settings: permission denied.");
    return { ...EMPTY, ...args };
  } });
  await type("api-model", "model");
  await click("api-save");
  assert.equal(el("api-error").textContent, "Couldn't save your settings: permission denied.");
  assert.equal(el("api-base-url").value, "https://api.openai.com/v1");
  assert.equal(el("api-model").value, "model");
  assert.equal(el("api-save").disabled, false);
  assert.equal(el("api-test").disabled, true);
  await click("api-save");
  assert.equal(attempts, 2);
  assert.equal(el("api-error"), null);
  assert.equal(el("api-test").disabled, false);
});

test("#109 QA: switching service from an empty setup saves the selected service's real default", async () => {
  const calls = await mount({ settings_set_api: (args) => ({ ...EMPTY, ...args }) });
  await type("api-model", "openai-model");
  await choose("anthropic");
  assert.equal(el("api-base-url").value, "https://api.anthropic.com");
  assert.equal(el("api-base-url").getAttribute("placeholder"), null);
  assert.equal(el("api-model").value, "");
  assert.equal(el("api-test").disabled, true);
  await type("api-model", "claude-model");
  await click("api-save");
  assert.deepEqual(calls.find(([command]) => command === "settings_set_api")[1], {
    kind: "anthropic", baseUrl: "https://api.anthropic.com", model: "claude-model",
  });
});
