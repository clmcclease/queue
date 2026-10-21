// D-030: while a download runs, Settings says it finishes in the folder it started with.
// Also checks the panel scrolls itself, so a long Settings never spills onto the page.
import assert from "node:assert/strict";
import { after, afterEach, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { JSDOM } from "jsdom";
import React, { act } from "react";
import { createServer } from "vite";

const dom = new JSDOM("<!doctype html><html><body></body></html>", { url: "http://localhost/" });
globalThis.window = dom.window;
globalThis.document = dom.window.document;
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const { createRoot } = await import("react-dom/client");
let server, SettingsPanel, root, container;

before(async () => {
  server = await createServer({
    root: fileURLToPath(new URL("../../", import.meta.url)),
    configFile: false, logLevel: "silent", appType: "custom",
    server: { middlewareMode: true, hmr: false, watch: null },
  });
  SettingsPanel = (await server.ssrLoadModule("/src/components/SettingsPanel.tsx")).default;
});
afterEach(async () => {
  if (root) await act(async () => root.unmount());
  root = undefined;
  container?.remove();
});
after(async () => {
  await server.close();
  dom.window.close();
  delete globalThis.window;
  delete globalThis.document;
  delete globalThis.IS_REACT_ACT_ENVIRONMENT;
});

const folder = { folder: { path: "/Downloads", error: null }, error: null, choosing: false, change: async () => {} };

async function render(props) {
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(SettingsPanel, { folder, onClose: () => {}, ...props })));
}

test("the folder note shows only while a download is running", async () => {
  await render({ downloading: true });
  assert.equal(
    container.querySelector('[data-role="folder-note"]')?.textContent,
    "Downloads in progress keep the folder they started in.",
  );
  await act(async () => root.render(React.createElement(SettingsPanel, { folder, onClose: () => {}, downloading: false })));
  assert.equal(container.querySelector('[data-role="folder-note"]'), null);
});

test("without the prop there is no note", async () => {
  await render({});
  assert.equal(container.querySelector('[data-role="folder-note"]'), null);
});

test("the panel scrolls itself", async () => {
  await render({});
  assert.ok(container.querySelector('[data-role="settings"]').classList.contains("overflow-y-auto"));
});
