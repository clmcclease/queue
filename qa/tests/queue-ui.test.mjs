// Independent QA: #3 / Q1. Real React DOM, mocked Tauri IPC; no native webview coverage.
// jsdom is a pinned devDependency (test-only).
// Then npm run build && npm test (or node --test qa/tests/queue-ui.test.mjs).
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
// Import after installing DOM globals so React's input-event support is detected correctly.
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

function deferred() {
  let resolve, reject;
  const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}

async function mount(handler) {
  mockIPC(handler, { shouldMockEvents: true });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(App)));
}

const field = () => container.querySelector("input");
const button = () => container.querySelector("button");
const rows = () => [...container.querySelectorAll("li")].map((row) => row.querySelector('[data-role="url"]').textContent);
// Every backend item carries a status and a revision (#4).
const QUEUED = { status: { state: "queued" }, rev: 1 };

async function type(value) {
  await act(async () => {
    // Bypass React's instance value tracker, as an actual user edit does.
    Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, "value").set.call(field(), value);
    field().dispatchEvent(new dom.window.Event("input", { bubbles: true }));
  });
}

async function submit(method = "form") {
  await act(async () => {
    if (method === "button") button().click();
    else container.querySelector("form").dispatchEvent(new dom.window.Event("submit", { bubbles: true, cancelable: true }));
  });
}

function immediateQueue(initial = []) {
  const stored = [...initial];
  const calls = [];
  return {
    stored, calls,
    handler(command, args) {
      calls.push({ command, args });
      if (command === "queue_list") return [...stored];
      assert.equal(command, "queue_add");
      const item = { id: stored.length + 1, url: args.url.trim(), ...QUEUED };
      stored.push(item);
      return item;
    },
  };
}

test("Q1: initial input is single-line and focused; blank submissions do nothing", async () => {
  const queue = immediateQueue();
  await mount(queue.handler);
  assert.equal(field().type, "text");
  assert.equal(field().inputMode, "url");
  assert.equal(document.activeElement, field());
  assert.equal(button().textContent, "+");
  for (const blank of ["", " \t ", "\u{a0}\u{2003}\u{3000}"]) {
    await type(blank);
    assert.equal(button().disabled, true);
    await submit();
  }
  assert.equal(queue.calls.filter((call) => call.command === "queue_add").length, 0);
  assert.deepEqual(rows(), []);
});

test("Q1: form submit and + each add one raw Unicode URL, clear the field, and restore focus", async () => {
  const queue = immediateQueue();
  await mount(queue.handler);
  const urls = ["https://例え.jp/🎵?a=e\u{301}&b=2#片段", "https://example.test/CON?title=OFFICIAL%20VIDEO"];
  for (const [index, method] of ["form", "button"].entries()) {
    await type(`  ${urls[index]}  `);
    assert.equal(button().disabled, false);
    button().focus();
    await submit(method);
    assert.equal(field().value, "");
    assert.equal(document.activeElement, field());
    assert.deepEqual(rows(), urls.slice(0, index + 1));
  }
  assert.equal(queue.calls.filter((call) => call.command === "queue_add").length, 2);
});

test("Q1: raw HTML-like text is shown literally and never creates elements", async () => {
  const queue = immediateQueue();
  await mount(queue.handler);
  const url = 'https://example.test/?title=<img src=x onerror="alert(1)">&q=<script>oops</script>';
  await type(url);
  await submit("button");
  assert.deepEqual(rows(), [url]);
  assert.equal(container.querySelectorAll("img,script").length, 0);
  assert.equal(container.querySelector('li [data-role="url"]').title, url);
});

test("Q1: initial backend rows appear in order and subsequent adds append", async () => {
  const initial = [{ id: 1, url: "https://example.test/old", ...QUEUED }];
  const queue = immediateQueue(initial);
  await mount(queue.handler);
  assert.deepEqual(rows(), [initial[0].url]);
  await type("https://example.test/new");
  await submit();
  assert.deepEqual(rows(), [initial[0].url, "https://example.test/new"]);
});

test("Q1: failed add keeps the input for retry; successful retry clears its alert", async () => {
  let fail = true;
  await mount((command, args) => {
    if (command === "queue_list") return [];
    if (fail) throw new Error("queue lock poisoned");
    return { id: 1, url: args.url, ...QUEUED };
  });
  await type("https://example.test/retry");
  await submit("button");
  assert.equal(field().value, "https://example.test/retry");
  assert.match(container.querySelector('[role="alert"]').textContent, /queue lock poisoned/);
  assert.deepEqual(rows(), []);
  fail = false;
  await submit();
  assert.deepEqual(rows(), ["https://example.test/retry"]);
  assert.equal(container.querySelector('[role="alert"]'), null);
});

test("Q1 regression: completing an earlier add must preserve text typed while it was pending", async () => {
  const pending = deferred();
  const calls = [];
  await mount((command, args) => {
    if (command === "queue_list") return [];
    calls.push(args.url);
    return pending.promise;
  });
  await type("https://example.test/first");
  await submit();
  assert.deepEqual(calls, ["https://example.test/first"]);
  await type("https://example.test/next");
  await act(async () => pending.resolve({ id: 1, url: calls[0], ...QUEUED }));
  assert.deepEqual(rows(), ["https://example.test/first"]);
  assert.equal(field().value, "https://example.test/next", "a completed add must not erase the next link");
});

test("Q1 regression: a delayed initial list must not erase a successfully added row", async () => {
  const initialList = deferred();
  await mount((command, args) => {
    if (command === "queue_list") return initialList.promise;
    return { id: 1, url: args.url, ...QUEUED };
  });
  await type("https://example.test/new");
  await submit();
  assert.deepEqual(rows(), ["https://example.test/new"]);
  await act(async () => initialList.resolve([]));
  assert.deepEqual(rows(), ["https://example.test/new"], "a stale startup snapshot must not hide a queued item");
});

// Review round 2 (qa/reports/20261010-174447-review-cdVo.md): every reply merges by id in backend order.
test("Q1: a snapshot that already contains a pending add leaves exactly one row", async (t) => {
  const add = deferred();
  const list = deferred();
  t.after(() => { add.resolve({ id: 1, url: "https://example.test/a", ...QUEUED }); list.resolve([]); }); // never hang on failure
  await mount((command) => (command === "queue_list" ? list.promise : add.promise));
  await type("https://example.test/a");
  await submit();
  await act(async () => list.resolve([{ id: 1, url: "https://example.test/a", ...QUEUED }]));
  await act(async () => add.resolve({ id: 1, url: "https://example.test/a", ...QUEUED }));
  assert.deepEqual(rows(), ["https://example.test/a"]);
});

test("Q1: add replies arriving in reverse order still render in backend order", async (t) => {
  const replies = [deferred(), deferred()];
  t.after(() => replies.forEach((r, i) => r.resolve({ id: i + 1, url: `https://example.test/${i + 1}` })));
  let n = 0;
  await mount((command) => (command === "queue_list" ? [] : replies[n++].promise));
  await type("https://example.test/1");
  await submit();
  await type("https://example.test/2");
  await submit();
  await act(async () => replies[1].resolve({ id: 2, url: "https://example.test/2", ...QUEUED }));
  await act(async () => replies[0].resolve({ id: 1, url: "https://example.test/1", ...QUEUED }));
  assert.deepEqual(rows(), ["https://example.test/1", "https://example.test/2"]);
});

test("Q1: clicking + returns focus to the field before the add finishes", async (t) => {
  const add = deferred();
  t.after(() => add.resolve({ id: 1, url: "https://example.test/slow", ...QUEUED }));
  await mount((command) => (command === "queue_list" ? [] : add.promise));
  await type("https://example.test/slow");
  button().focus();
  await submit("button");
  assert.equal(document.activeElement, field(), "focus must not wait for IPC");
  await act(async () => add.resolve({ id: 1, url: "https://example.test/slow", ...QUEUED }));
});

test("Q1: one add rejecting while another succeeds keeps the good row, shows the error, and allows more adds", async (t) => {
  const replies = [deferred(), deferred()];
  t.after(() => replies.forEach((r) => r.resolve({ id: 99, url: "cleanup", ...QUEUED })));
  let n = 0;
  let nextId = 1;
  await mount((command, args) => {
    if (command === "queue_list") return [];
    if (n < 2) return replies[n++].promise;
    return { id: nextId++, url: args.url.trim(), ...QUEUED };
  });
  await type("https://example.test/bad");
  await submit();
  await type("https://example.test/good");
  await submit();
  await type("https://example.test/typing");
  await act(async () => replies[1].resolve({ id: 1, url: "https://example.test/good", ...QUEUED }));
  nextId = 2;
  await act(async () => replies[0].reject("Enter a link to add."));
  assert.deepEqual(rows(), ["https://example.test/good"]);
  assert.equal(container.querySelector('[role="alert"]')?.textContent, "Enter a link to add.");
  assert.equal(field().value, "https://example.test/typing", "neither reply may clear newer text");
  await type("https://example.test/after");
  await submit();
  assert.deepEqual(rows(), ["https://example.test/good", "https://example.test/after"]);
  assert.equal(container.querySelector('[role="alert"]'), null, "a successful add clears the error");
});

// #4 (D2): download one item; status arrives via the command reply and the queue://item event.
const at = (item, status, rev) => ({ ...item, status, rev });
const downloadButtons = () => [...container.querySelectorAll("li button")].filter((b) => b.textContent === "Download");
const statusText = () => [...container.querySelectorAll("li")].map((row) => row.querySelector("[data-role=url] + div").textContent);

test("D2: Download shows progress, then the saved file name from the event", async (t) => {
  const reply = deferred();
  t.after(() => reply.resolve(null));
  const item = { id: 1, url: "https://example.test/v", ...QUEUED };
  const calls = [];
  await mount((command, args) => {
    calls.push({ command, args });
    if (command === "queue_list") return [item];
    assert.equal(command, "queue_download");
    return reply.promise;
  });
  assert.equal(downloadButtons().length, 1);
  await act(async () => downloadButtons()[0].click());
  assert.deepEqual(calls.at(-1), { command: "queue_download", args: { id: 1 } });
  await act(async () => reply.resolve(at(item, { state: "downloading" }, 2)));
  assert.deepEqual(statusText(), ["Downloading…"]);
  assert.equal(downloadButtons().length, 0, "no second Download while running");
  await act(async () => emit("queue://item", at(item, { state: "done", path: "/Users/me/Downloads/Me at the zoo.webm" }, 3)));
  assert.deepEqual(statusText(), ["✓ Me at the zoo.webm"]);
  assert.equal(container.querySelector("li [title]:not([data-role])").title, "/Users/me/Downloads/Me at the zoo.webm");
});

test("D2: a stale reply arriving after the finish event does not revert the row", async (t) => {
  const reply = deferred();
  t.after(() => reply.resolve(null));
  const item = { id: 1, url: "https://example.test/v", ...QUEUED };
  await mount((command) => (command === "queue_list" ? [item] : reply.promise));
  await act(async () => downloadButtons()[0].click());
  await act(async () => emit("queue://item", at(item, { state: "failed", reason: "Video unavailable" }, 3)));
  await act(async () => reply.resolve(at(item, { state: "downloading" }, 2)));
  assert.deepEqual(statusText(), ["Failed: Video unavailable"]);
});

test("D2: Windows paths show just the file name; a rejected download shows the error", async () => {
  const items = [{ id: 1, url: "https://example.test/a", ...QUEUED }, { id: 2, url: "https://example.test/b", ...QUEUED }];
  await mount((command) => {
    if (command === "queue_list") return items;
    throw "Only queued items can be downloaded.";
  });
  await act(async () => emit("queue://item", at(items[0], { state: "done", path: "C:\\Users\\me\\Downloads\\Song.m4a" }, 3)));
  assert.equal(statusText()[0], "✓ Song.m4a");
  assert.equal(downloadButtons().length, 1, "only the queued row offers Download");
  await act(async () => downloadButtons()[0].click());
  assert.equal(container.querySelector('[role="alert"]')?.textContent, "Only queued items can be downloaded.");
});

// Independent QA additions for #4 / D2.
test("D2: a finish event survives a delayed startup snapshot and older events", async (t) => {
  const list = deferred();
  t.after(() => list.resolve([]));
  const item = { id: 1, url: "https://example.test/slow", ...QUEUED };
  await mount((command) => {
    assert.equal(command, "queue_list");
    return list.promise;
  });
  const done = at(item, { state: "done", path: "/Downloads/日本語 🎵 e\u{301}.webm" }, 3);
  await act(async () => emit("queue://item", done));
  await act(async () => list.resolve([item]));
  await act(async () => emit("queue://item", at(item, { state: "downloading" }, 2)));
  assert.deepEqual(rows(), [item.url]);
  assert.deepEqual(statusText(), ["✓ 日本語 🎵 e\u{301}.webm"]);
  assert.equal(downloadButtons().length, 0);
});

test("D2: title and error markup are literal text; another queued row remains downloadable", async () => {
  const items = [1, 2, 3].map((id) => ({ id, url: `https://example.test/${id}`, ...QUEUED }));
  await mount((command) => {
    assert.equal(command, "queue_list");
    return items;
  });
  const path = '/Downloads/<img onerror="bad"> 日本語.webm';
  const reason = '<script>bad()</script> & "unavailable"';
  await act(async () => {
    await emit("queue://item", at(items[0], { state: "done", path }, 3));
    await emit("queue://item", at(items[1], { state: "failed", reason }, 3));
  });
  assert.deepEqual(statusText(), ['✓ <img onerror="bad"> 日本語.webm', `Failed: ${reason}`, ""]);
  assert.equal(container.querySelectorAll("img,script").length, 0);
  assert.equal(container.querySelector('[role="status"]').title, reason);
  assert.equal(downloadButtons().length, 1);
  assert.deepEqual(rows(), items.map((item) => item.url));
});

// #4 review round (qa/reports/20261010-184319-review-pWmI.md).
async function mountRaw(handler) {
  mockIPC(handler); // no event mocking: the handler sees plugin:event|listen itself
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(App)));
}

test("D2: the queue snapshot is requested only after the event listener is registered", async (t) => {
  const registered = deferred();
  t.after(() => registered.resolve(1));
  const calls = [];
  await mountRaw((command) => {
    calls.push(command);
    if (command === "plugin:event|listen") return registered.promise;
    if (command === "plugin:event|unlisten") return null;
    if (command === "queue_list") return [];
    throw new Error(`unexpected ${command}`);
  });
  assert.ok(calls.includes("plugin:event|listen"));
  assert.ok(!calls.includes("queue_list"), "snapshot must wait for the listener");
  await act(async () => registered.resolve(1));
  assert.ok(calls.indexOf("queue_list") > calls.indexOf("plugin:event|listen"));
});

test("D2: a failed listener registration shows an error instead of failing silently", async () => {
  await mountRaw((command) => {
    if (command === "plugin:event|listen") throw "event system unavailable";
    return [];
  });
  assert.match(container.querySelector('[data-role="disconnected"]')?.textContent ?? "", /event system unavailable.*Restart Queue/);
});

test("D2: after a failed listener registration, downloads stay off and the warning survives a successful add", async () => {
  let n = 0;
  await mountRaw((command, args) => {
    if (command === "plugin:event|listen") throw "event system unavailable";
    if (command === "queue_add") return { id: ++n, url: args.url, ...QUEUED };
    if (command === "queue_download") throw new Error("must not start an unobservable download");
    return [];
  });
  await type("https://example.test/a");
  await submit();
  assert.deepEqual(rows(), ["https://example.test/a"]);
  assert.ok(container.querySelector('[data-role="disconnected"]'), "warning still shown after a good add");
  assert.equal(downloadButtons()[0].disabled, true);
});

test("D2: Download stays disabled until the listener is registered", async (t) => {
  const registered = deferred();
  t.after(() => registered.resolve(1));
  await mountRaw((command) => {
    if (command === "plugin:event|listen") return registered.promise;
    if (command === "plugin:event|unlisten") return null;
    if (command === "queue_list") return [{ id: 1, url: "https://example.test/a", ...QUEUED }];
    return null;
  });
  await type("x"); // no rows yet: the snapshot waits for the listener
  assert.equal(downloadButtons().length, 0);
  await act(async () => registered.resolve(1));
  assert.equal(downloadButtons()[0].disabled, false);
});

test("D2: while one item downloads, the other Download buttons are disabled", async () => {
  const items = [
    { id: 1, url: "https://example.test/a", status: { state: "downloading" }, rev: 2 },
    { id: 2, url: "https://example.test/b", ...QUEUED },
  ];
  await mount((command) => (command === "queue_list" ? items : null));
  assert.equal(downloadButtons().length, 1);
  assert.equal(downloadButtons()[0].disabled, true);
  await act(async () => emit("queue://item", at(items[0], { state: "done", path: "/d/a.webm" }, 3)));
  assert.equal(downloadButtons()[0].disabled, false, "enabled again once the download finishes");
});
