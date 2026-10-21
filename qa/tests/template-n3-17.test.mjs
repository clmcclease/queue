// #17 / N3: the naming template picker in Settings, with live previews. Real React DOM, mocked
// Tauri IPC. Then npm run build && npm test (or node --test qa/tests/template-n3-17.test.mjs).
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

const PREVIEWS = [
  ["title", "Me at the zoo"],
  ["channel_title", "jawed - Me at the zoo"],
  ["date_title", "2005-04-23 Me at the zoo"],
];
const ROW = {
  id: 1,
  url: "https://example.test/v",
  status: { state: "queued" },
  rev: 1,
  meta: { title: "A", channel: "Chan", duration: null, thumbnail: null, site: null },
  name: "A",
  name_source: "rules",
};

async function openSettings() {
  await act(async () => container.querySelector('button[aria-label="Settings"]').click());
}
const radios = () => [...container.querySelectorAll('[data-role="template"] input[type="radio"]')];
const previews = () => [...container.querySelectorAll('[data-role="template"] [data-role="preview"]')].map((p) => p.textContent);

test("N3: Settings shows the three templates with their previews and the saved choice", async () => {
  await mount((command) => {
    if (command === "queue_list") return [];
    if (command === "settings_naming_template") return "date_title";
    if (command === "naming_previews") return PREVIEWS;
    throw new Error(`unexpected ${command}`);
  });
  await openSettings();
  assert.deepEqual(radios().map((r) => r.value), ["title", "channel_title", "date_title"]);
  assert.deepEqual(radios().map((r) => r.checked), [false, false, true]);
  assert.deepEqual(previews(), PREVIEWS.map(([, p]) => p));
});

test("N3: choosing a template saves it once; a rename event updates the queued row", async () => {
  const calls = [];
  await mount((command, args) => {
    calls.push({ command, args });
    if (command === "queue_list") return [ROW];
    if (command === "settings_naming_template") return "title";
    if (command === "naming_previews") return PREVIEWS;
    assert.equal(command, "settings_set_naming_template");
    return args.template;
  });
  await openSettings();
  await act(async () => radios()[1].click());
  assert.deepEqual(calls.filter((c) => c.command === "settings_set_naming_template"), [
    { command: "settings_set_naming_template", args: { template: "channel_title" } },
  ]);
  assert.equal(radios()[1].checked, true);
  await act(async () => emit("queue://item", { ...ROW, rev: 2, name: "Chan - A" }));
  assert.equal(container.querySelector('[data-role="name"]').textContent, "Chan - A");
});

test("N3: a failed save puts the previous template back and shows the reason literally", async () => {
  await mount((command) => {
    if (command === "queue_list") return [];
    if (command === "settings_naming_template") return "title";
    if (command === "naming_previews") return PREVIEWS;
    throw "Couldn't save your settings: <disk full>";
  });
  await openSettings();
  await act(async () => radios()[2].click());
  assert.equal(radios()[0].checked, true);
  const alert = container.querySelector('[data-role="template-error"]');
  assert.equal(alert.textContent, "Couldn't save your settings: <disk full>");
  assert.equal(alert.querySelector("*"), null, "literal text, no markup");
});

test("N3: without previews the choices still work", async () => {
  await mount((command, args) => {
    if (command === "queue_list") return [];
    if (command === "settings_naming_template") return "title";
    if (command === "naming_previews") throw "no previews";
    return args.template;
  });
  await openSettings();
  assert.deepEqual(previews(), []);
  await act(async () => radios()[1].click());
  assert.equal(radios()[1].checked, true);
});

// Independent QA additions for #17: delayed IPC, recovery, remounting, and literal Unicode.
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

test("N3 QA: controls wait for the saved template and clicking it never saves again", async () => {
  const read = deferred();
  const saves = [];
  await mount((command, args) => {
    if (command === "queue_list") return [];
    if (command === "settings_naming_template") return read.promise;
    if (command === "naming_previews") return PREVIEWS;
    if (command === "settings_set_naming_template") saves.push(args);
  });
  await openSettings();
  assert.ok(radios().every((r) => r.disabled && !r.checked));
  await act(async () => radios()[1].click());
  assert.deepEqual(saves, []);
  await act(async () => read.resolve("title"));
  assert.ok(radios().every((r) => !r.disabled));
  await act(async () => radios()[0].click());
  assert.deepEqual(saves, []);
});

test("N3 QA: a pending save blocks competing choices and re-enables them when complete", async () => {
  const write = deferred();
  const saves = [];
  await mount((command, args) => {
    if (command === "queue_list") return [];
    if (command === "settings_naming_template") return "title";
    if (command === "naming_previews") return PREVIEWS;
    if (command === "settings_set_naming_template") {
      saves.push(args);
      return write.promise;
    }
  });
  await openSettings();
  await act(async () => radios()[1].click());
  assert.equal(radios()[1].checked, true);
  assert.ok(radios().every((r) => r.disabled));
  await act(async () => radios()[2].click());
  assert.deepEqual(saves, [{ template: "channel_title" }]);
  await act(async () => write.resolve("channel_title"));
  assert.ok(radios().every((r) => !r.disabled));
  assert.equal(radios()[1].checked, true);
});

test("N3 QA: rejected save can be retried successfully and clears the literal error", async () => {
  let attempts = 0;
  await mount((command, args) => {
    if (command === "queue_list") return [];
    if (command === "settings_naming_template") return "title";
    if (command === "naming_previews") return PREVIEWS;
    if (command === "settings_set_naming_template") {
      if (++attempts === 1) throw "Timeout: <日本語> & disk full";
      return args.template;
    }
  });
  await openSettings();
  await act(async () => radios()[2].click());
  assert.equal(radios()[0].checked, true);
  assert.equal(container.querySelector('[data-role="template-error"]').textContent, "Timeout: <日本語> & disk full");
  assert.ok(radios().every((r) => !r.disabled));
  await act(async () => radios()[2].click());
  assert.equal(attempts, 2);
  assert.equal(radios()[2].checked, true);
  assert.equal(container.querySelector('[data-role="template-error"]'), null);
});

test("N3 QA: reopening Settings reads the saved choice and ignores an obsolete preview reply", async () => {
  let saved = "title", reads = 0;
  const obsolete = deferred();
  await mount((command, args) => {
    if (command === "queue_list") return [];
    if (command === "settings_naming_template") return saved;
    if (command === "naming_previews") return ++reads === 1 ? obsolete.promise : PREVIEWS;
    if (command === "settings_set_naming_template") return saved = args.template;
  });
  await openSettings();
  await act(async () => radios()[2].click());
  await act(async () => container.querySelector('button[aria-label="Close settings"]').click());
  await openSettings();
  assert.equal(radios()[2].checked, true);
  await act(async () => obsolete.resolve([["title", "obsolete"]]));
  assert.deepEqual(previews(), PREVIEWS.map(([, p]) => p));
});

test("N3 QA: previews preserve Unicode and render filename text without interpreting HTML", async () => {
  const unsafe = 'Café e\u0301 日本語 <img src=x onerror="boom"> & %(title)s';
  await mount((command) => {
    if (command === "queue_list") return [];
    if (command === "settings_naming_template") return "title";
    if (command === "naming_previews") return [["title", unsafe]];
  });
  await openSettings();
  const preview = container.querySelector('[data-role="preview"]');
  assert.equal(preview.textContent, unsafe);
  assert.equal(preview.querySelector("*"), null);
});

test("N3 QA: a failed initial read is visible and reopening allows recovery", async () => {
  let reads = 0;
  await mount((command) => {
    if (command === "queue_list") return [];
    if (command === "settings_naming_template") {
      if (++reads === 1) throw "Couldn't read settings: <unavailable>";
      return "channel_title";
    }
    if (command === "naming_previews") return PREVIEWS;
  });
  await openSettings();
  assert.ok(radios().every((r) => r.disabled));
  assert.equal(container.querySelector('[data-role="template-error"]').textContent, "Couldn't read settings: <unavailable>");
  await act(async () => container.querySelector('button[aria-label="Close settings"]').click());
  await openSettings();
  assert.ok(radios().every((r) => !r.disabled));
  assert.equal(radios()[1].checked, true);
  assert.equal(container.querySelector('[data-role="template-error"]'), null);
});
