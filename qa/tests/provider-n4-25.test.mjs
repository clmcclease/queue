// #25 / N4: the AI naming provider in Settings, the "model not downloaded" banner, and the row
// tags. Real React DOM, mocked Tauri IPC. Then npm run build && npm test.
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

async function mount(handler) {
  mockIPC((command, args) => {
    if (command === "queue_runner_state") return false;
    if (command === "settings_download_folder") return { path: "/Users/me/Downloads", error: null };
    if (command === "settings_format") return "default";
    if (command === "queue_storage_problem") return null;
    return handler(command, args);
  }, { shouldMockEvents: true });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(App)));
}

const LOCAL_MISSING = { provider: "local", unavailable: "model_missing" };
const row = (id, name_source) => ({
  id,
  url: `https://example.test/${id}`,
  status: { state: "queued" },
  rev: 1,
  meta: { title: "T", channel: null, duration: null, thumbnail: null, site: null },
  name: `Name ${id}`,
  name_source,
});
const banner = () => container.querySelector('[data-role="naming-banner"]');
const radios = () => [...container.querySelectorAll('[data-role="provider"] input[type="radio"]')];
async function openSettings() {
  await act(async () => container.querySelector('button[aria-label="Settings"]').click());
}

test("N4: a fresh install shows the not-downloaded banner, and rows carry no tag", async () => {
  await mount((command) => {
    if (command === "queue_list") return [row(1, "rules")];
    if (command === "naming_status") return LOCAL_MISSING;
    throw new Error(`unexpected ${command}`);
  });
  assert.match(banner().textContent, /local model that isn't downloaded yet/);
  assert.equal(banner().getAttribute("role"), "status", "not an error");
  assert.equal(container.querySelector('[data-role="name-tag"]'), null, "nothing was asked, so no 'rules' tag");
});

test("N4: rows named by the AI say AI; rows whose ask failed say rules, with why", async () => {
  await mount((command) => {
    if (command === "queue_list") return [row(1, "ai"), row(2, "fallback"), row(3, "edited")];
    if (command === "naming_status") return { provider: "api", unavailable: null };
    throw new Error(`unexpected ${command}`);
  });
  const tags = [...container.querySelectorAll('[data-role="name-tag"]')];
  assert.deepEqual(tags.map((t) => t.textContent), ["AI", "rules"]);
  assert.match(tags[1].title, /didn't give a usable name/);
  assert.equal(banner(), null);
});

test("N4: Settings offers Local (default), API and Off; switching to Off hides the banner", async () => {
  const calls = [];
  await mount((command, args) => {
    calls.push({ command, args });
    if (command === "queue_list") return [];
    if (command === "naming_status") return LOCAL_MISSING;
    if (command === "settings_naming_provider") return "local";
    assert.equal(command, "settings_set_naming_provider");
    return { provider: args.provider, unavailable: args.provider === "off" ? "off" : "not_set_up" };
  });
  await openSettings();
  assert.deepEqual(radios().map((r) => r.value), ["local", "api", "off"]);
  assert.match(container.querySelector('[data-role="provider"]').textContent, /Local \(default\)/);
  assert.equal(radios()[0].checked, true);
  await act(async () => radios()[2].click());
  assert.deepEqual(calls.at(-1), { command: "settings_set_naming_provider", args: { provider: "off" } });
  assert.equal(banner(), null);
});

test("N4: choosing API says what's sent and that it isn't set up yet", async () => {
  await mount((command, args) => {
    if (command === "queue_list") return [];
    if (command === "naming_status") return LOCAL_MISSING;
    if (command === "settings_naming_provider") return "local";
    return { provider: args.provider, unavailable: "not_set_up" };
  });
  await openSettings();
  const group = container.querySelector('[data-role="provider"]');
  assert.match(group.textContent, /title, channel and description are sent/);
  await act(async () => radios()[1].click());
  assert.equal(radios()[1].checked, true, "the radio isn't disabled");
  assert.match(container.querySelector('[data-role="provider-note"]').textContent, /Not set up yet/);
});

test("N4: a failed save puts the previous provider back and shows the reason literally", async () => {
  await mount((command) => {
    if (command === "queue_list") return [];
    if (command === "naming_status") return LOCAL_MISSING;
    if (command === "settings_naming_provider") return "local";
    throw "Couldn't save your settings: <disk full>";
  });
  await openSettings();
  await act(async () => radios()[2].click());
  assert.equal(radios()[0].checked, true);
  const alert = container.querySelector('[data-role="provider-error"]');
  assert.equal(alert.textContent, "Couldn't save your settings: <disk full>");
  assert.equal(alert.querySelector("*"), null);
  assert.ok(banner(), "the banner stays: the provider didn't change");
});
