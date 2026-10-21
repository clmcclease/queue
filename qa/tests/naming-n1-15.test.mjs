// #15 / N1: the proposed file name in each row, edited inline. Real React DOM, mocked Tauri IPC.
// Then npm run build && npm test (or node --test qa/tests/naming-n1-15.test.mjs).
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

const ROW = {
  id: 1,
  url: "https://example.test/v",
  status: { state: "queued" },
  rev: 1,
  meta: { title: "Song (Official Video)", channel: null, duration: null, thumbnail: null, site: null },
  name: "Song",
  name_source: "rules",
};
const nameEl = () => container.querySelector('[data-role="name"]');
const nameInput = () => container.querySelector('input[aria-label="File name"]');

async function edit(value) {
  await act(async () => nameEl().click());
  await act(async () => {
    Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, "value").set.call(nameInput(), value);
    nameInput().dispatchEvent(new dom.window.Event("input", { bubbles: true }));
  });
}

async function key(k) {
  await act(async () => {
    nameInput().dispatchEvent(new dom.window.KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
  });
}

test("N1: a queued row shows its name; Enter saves an edit and shows the backend's sanitized name", async () => {
  const calls = [];
  await mount((command, args) => {
    calls.push({ command, args });
    if (command === "queue_list") return [ROW];
    assert.equal(command, "queue_rename");
    return { ...ROW, rev: 2, name: "My⧸song", name_source: "edited" };
  });
  assert.equal(nameEl().tagName, "BUTTON", "editable while queued");
  assert.equal(nameEl().textContent, "Song");
  await edit("  My/song  ");
  assert.equal(nameInput().value, "  My/song  ");
  await key("Enter");
  assert.deepEqual(calls.at(-1), { command: "queue_rename", args: { id: 1, name: "My/song" } });
  assert.equal(nameInput(), null, "the field closes");
  assert.equal(nameEl().textContent, "My⧸song", "the backend's sanitized name is shown");
});

test("N1: Escape, an unchanged name, or a blank one sends nothing", async () => {
  const calls = [];
  await mount((command, args) => {
    calls.push({ command, args });
    if (command === "queue_list") return [ROW];
    throw new Error(`unexpected ${command}`);
  });
  await edit("Something else");
  await key("Escape");
  await edit("Song");
  await key("Enter");
  await edit("   ");
  await key("Enter");
  assert.deepEqual(calls.map((c) => c.command), ["queue_list"]);
  assert.equal(nameEl().textContent, "Song");
});

test("N1: a refused rename shows the reason and keeps the old name", async () => {
  await mount((command) => {
    if (command === "queue_list") return [ROW];
    throw "Only a queued item can be renamed.";
  });
  await edit("New name");
  await key("Enter");
  assert.equal(nameEl().textContent, "Song");
  assert.match(container.textContent, /Only a queued item can be renamed\./);
});

test("N1: a downloading row shows its name but can't be edited; a fetching row has none yet", async () => {
  const fetching = { id: 2, url: "https://example.test/w", status: { state: "fetching" }, rev: 1, meta: null };
  await mount((command) => {
    if (command === "queue_list") return [ROW, fetching];
    throw new Error(`unexpected ${command}`);
  });
  await act(async () => emit("queue://item", { ...ROW, rev: 2, status: { state: "downloading" } }));
  const shown = container.querySelectorAll('[data-role="name"]');
  assert.equal(shown.length, 1, "only the row with info has a name");
  assert.equal(shown[0].tagName, "DIV");
  assert.equal(shown[0].textContent, "Song");
});

test("N1: a name with markup is literal text", async () => {
  await mount((command) => {
    if (command === "queue_list") return [{ ...ROW, name: "<img src=x onerror=alert(1)>" }];
    throw new Error(`unexpected ${command}`);
  });
  assert.equal(nameEl().textContent, "<img src=x onerror=alert(1)>");
  assert.equal(container.querySelector("li img"), null);
});

// Independent QA additions for #15 / N1.
function deferredRename() {
  let resolve, reject;
  const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}

test("N1 QA: leaving the field saves a Unicode edit exactly once", async () => {
  const calls = [];
  const name = "Café e\u0301 日本語 🎵 100% %(title)s";
  await mount((command, args) => {
    if (command === "queue_list") return [ROW];
    assert.equal(command, "queue_rename");
    calls.push(args);
    return { ...ROW, rev: 2, name: args.name, name_source: "edited" };
  });
  await edit(`  ${name}  `);
  await act(async () => nameInput().blur());
  assert.deepEqual(calls, [{ id: 1, name }]);
  assert.equal(nameEl().textContent, name);
  assert.equal(nameInput(), null);
});

test("N1 QA: a late rename reply cannot replace a newer downloading event", async () => {
  const pending = deferredRename();
  await mount((command) => {
    if (command === "queue_list") return [ROW];
    assert.equal(command, "queue_rename");
    return pending.promise;
  });
  await edit("Edited");
  await key("Enter");
  await act(async () => emit("queue://item", {
    ...ROW, rev: 3, name: "Edited", name_source: "edited", status: { state: "downloading" },
  }));
  await act(async () => pending.resolve({ ...ROW, rev: 2, name: "Edited", name_source: "edited" }));
  assert.equal(container.querySelector("li").dataset.state, "downloading");
  assert.equal(nameEl().tagName, "DIV");
  assert.equal(nameEl().textContent, "Edited");
});

test("N1 QA: a late rename reply cannot resurrect a removed row", async () => {
  const pending = deferredRename();
  await mount((command) => {
    if (command === "queue_list") return [ROW];
    if (command === "queue_remove") return null;
    assert.equal(command, "queue_rename");
    return pending.promise;
  });
  await edit("Edited");
  await key("Enter");
  await act(async () => container.querySelector('button[aria-label="Remove"]').click());
  await act(async () => pending.resolve({ ...ROW, rev: 2, name: "Edited", name_source: "edited" }));
  assert.equal(container.querySelector("li"), null);
});

test("N1 QA: a timed-out rename keeps the old name and permits a later successful edit", async () => {
  const pending = deferredRename();
  let calls = 0;
  await mount((command, args) => {
    if (command === "queue_list") return [ROW];
    assert.equal(command, "queue_rename");
    calls += 1;
    return calls === 1 ? pending.promise : { ...ROW, rev: 2, name: args.name, name_source: "edited" };
  });
  await edit("First");
  await key("Enter");
  await act(async () => pending.reject(new Error("IPC request timed out")));
  assert.equal(nameEl().textContent, "Song");
  assert.match(container.querySelector('[role="alert"]').textContent, /IPC request timed out/);
  await edit("Second 日本語");
  await key("Enter");
  assert.equal(nameEl().textContent, "Second 日本語");
  assert.equal(container.querySelector('[role="alert"]'), null);
});

test("N1 QA: IME Enter confirms composition without saving or closing the name editor", async () => {
  const calls = [];
  await mount((command, args) => {
    if (command === "queue_list") return [ROW];
    assert.equal(command, "queue_rename");
    calls.push(args);
    return { ...ROW, rev: 2, name: args.name, name_source: "edited" };
  });
  await edit("日本語");
  const input = nameInput();
  await act(async () => {
    input.dispatchEvent(new dom.window.CompositionEvent("compositionstart", { bubbles: true }));
    input.dispatchEvent(new dom.window.KeyboardEvent("keydown", {
      key: "Enter", isComposing: true, keyCode: 229, bubbles: true, cancelable: true,
    }));
  });
  assert.deepEqual(calls, [], "confirming an IME candidate must not submit a rename");
  assert.equal(nameInput(), input, "the user is still editing the composed name");
  await act(async () => input.dispatchEvent(new dom.window.CompositionEvent("compositionend", { bubbles: true })));
  await key("Enter");
  assert.deepEqual(calls, [{ id: 1, name: "日本語" }]);
});

test("N1 QA: an editor already open becomes read-only when a download starts", async () => {
  const calls = [];
  await mount((command, args) => {
    if (command === "queue_list") return [ROW];
    calls.push({ command, args });
    throw "Only a queued item can be renamed.";
  });
  await edit("Too late");
  await act(async () => emit("queue://item", { ...ROW, rev: 2, status: { state: "downloading" } }));
  assert.ok(!nameInput(), "the active download has captured its name; editing is no longer available");
  assert.equal(nameEl().tagName, "DIV");
  assert.equal(nameEl().textContent, "Song");
  assert.deepEqual(calls, []);
});

test("N1: a row that downloaded and came back Queued (cancelled) can be renamed again", async () => {
  const calls = [];
  await mount((command, args) => {
    calls.push({ command, args });
    if (command === "queue_list") return [ROW];
    assert.equal(command, "queue_rename");
    return { ...ROW, rev: 9, name: args.name, name_source: "edited" };
  });
  await act(async () => emit("queue://item", { ...ROW, rev: 2, status: { state: "downloading" } }));
  await act(async () => emit("queue://item", { ...ROW, rev: 3, status: { state: "queued" }, notice: "Cancelled." }));
  await edit("After cancel");
  await key("Enter");
  assert.deepEqual(calls.at(-1), { command: "queue_rename", args: { id: 1, name: "After cancel" } });
  assert.equal(nameEl().textContent, "After cancel");
});

test("N1: editing a name and clicking Download straight away downloads under the new name", async (t) => {
  const reply = { resolve: null };
  const renamed = new Promise((ok) => { reply.resolve = ok; });
  t.after(() => reply.resolve?.({ ...ROW, rev: 2, name: "New", name_source: "edited" }));
  const calls = [];
  await mount((command, args) => {
    calls.push(command);
    if (command === "queue_list") return [ROW];
    if (command === "queue_rename") return renamed;
    assert.equal(command, "queue_download");
    return { ...ROW, rev: 3, status: { state: "downloading" } };
  });
  await edit("New");
  // Clicking Download blurs the field (saving the name), then clicks.
  await act(async () => {
    nameInput().blur();
    [...container.querySelectorAll("li button")].find((b) => b.textContent === "Download").click();
  });
  assert.deepEqual(calls, ["queue_list", "queue_rename"], "the download waits for the rename");
  await act(async () => reply.resolve({ ...ROW, rev: 2, name: "New", name_source: "edited" }));
  assert.deepEqual(calls, ["queue_list", "queue_rename", "queue_download"]);
});
