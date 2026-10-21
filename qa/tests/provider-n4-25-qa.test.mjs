// Independent #25 / N4 QA: persisted unavailable providers, delayed snapshots and save errors.
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

const LOCAL = { provider: "local", unavailable: "model_missing" };
const API = { provider: "api", unavailable: "not_set_up" };
const OFF = { provider: "off", unavailable: "off" };
function deferred() {
  let resolve, reject;
  const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}
async function mount(overrides) {
  mockIPC((command, args) => {
    if (command in overrides) return overrides[command](args);
    switch (command) {
      case "queue_list": return [];
      case "queue_runner_state": return false;
      case "queue_storage_problem": return null;
      case "settings_download_folder": return { path: "/downloads", error: null };
      case "settings_format": return "default";
      case "settings_naming_template": return "title";
      case "naming_previews": return [["title", "Sample"]];
      case "naming_status": return LOCAL;
      case "settings_naming_provider": return "local";
      default: throw new Error(`Unexpected IPC: ${command}`);
    }
  }, { shouldMockEvents: true });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(App)));
}
const radios = () => [...container.querySelectorAll('[data-role="provider"] input')];
const banner = () => container.querySelector('[data-role="naming-banner"]');
const note = () => container.querySelector('[data-role="provider-note"]');
async function openSettings() {
  await act(async () => container.querySelector('button[aria-label="Settings"]').click());
}

test("N4 QA: reopening Settings for a saved API choice still explains its unavailable status", async () => {
  await mount({
    naming_status: () => API,
    settings_naming_provider: () => "api",
  });
  await openSettings();
  assert.equal(radios().find((r) => r.value === "api").checked, true);
  assert.ok(note(), "a persisted API choice needs the same not-set-up notice as a newly chosen API");
  assert.match(note().textContent, /Not set up yet.*rules/);
  assert.equal(banner()?.textContent ?? null, null);
});

test("N4 QA: a late startup Local snapshot cannot undo a successful switch to Off", async () => {
  const startup = deferred();
  await mount({
    naming_status: () => startup.promise,
    settings_set_naming_provider: ({ provider }) => {
      assert.equal(provider, "off");
      return OFF;
    },
  });
  await openSettings();
  await act(async () => radios().find((r) => r.value === "off").click());
  assert.equal(radios().find((r) => r.value === "off").checked, true);
  assert.equal(banner()?.textContent ?? null, null);
  await act(async () => startup.resolve(LOCAL));
  assert.equal(banner()?.textContent ?? null, null, "Off is saved: the older Local response must not revive its banner");
});

test("N4 QA: a pending save blocks duplicate changes; failure permits retry and clears its error", async () => {
  const save = deferred();
  const calls = [];
  await mount({
    settings_set_naming_provider: ({ provider }) => {
      calls.push(provider);
      return calls.length === 1 ? save.promise : API;
    },
  });
  await openSettings();
  await act(async () => radios().find((r) => r.value === "api").click());
  assert.ok(radios().every((r) => r.disabled));
  await act(async () => radios().find((r) => r.value === "off").click());
  assert.deepEqual(calls, ["api"]);
  await act(async () => save.reject("Disk full <unsafe> 日本語"));
  assert.ok(radios().every((r) => !r.disabled));
  assert.equal(radios().find((r) => r.value === "local").checked, true);
  const error = container.querySelector('[data-role="provider-error"]');
  assert.equal(error.textContent, "Disk full <unsafe> 日本語");
  assert.equal(error.children.length, 0);
  assert.ok(banner());
  await act(async () => radios().find((r) => r.value === "api").click());
  assert.deepEqual(calls, ["api", "api"]);
  assert.equal(container.querySelector('[data-role="provider-error"]'), null);
  assert.equal(banner()?.textContent ?? null, null);
  assert.ok(note());
});

test("N4 QA: a failed provider read is visible and cannot silently save a guessed preference", async () => {
  let writes = 0;
  await mount({
    settings_naming_provider: () => Promise.reject("Read failed <locked>"),
    settings_set_naming_provider: () => { writes++; return OFF; },
  });
  await openSettings();
  assert.ok(radios().every((r) => r.disabled && !r.checked));
  const error = container.querySelector('[data-role="provider-error"]');
  assert.equal(error.textContent, "Read failed <locked>");
  assert.equal(error.children.length, 0);
  await act(async () => radios().find((r) => r.value === "off").click());
  assert.equal(writes, 0);
});
