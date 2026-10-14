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

// Ambient answers so per-test handlers needn't: "Download all" state (#7) and the folder (D1).
const DEFAULT_FOLDER = { path: "/Users/me/Downloads", error: null };
function ambient(handler) {
  return (command, args) => {
    if (command === "queue_runner_state") return false;
    if (command === "settings_download_folder") return DEFAULT_FOLDER;
    return handler(command, args);
  };
}

async function mount(handler) {
  mockIPC(ambient(handler), { shouldMockEvents: true });
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
  mockIPC(ambient(handler)); // no event mocking: the handler sees plugin:event|listen itself
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

// #5 (Q2): invalid links are inline errors; duplicates are a toast plus a row highlight.
test("Q2: an invalid link shows the reason inline and keeps the text for fixing", async () => {
  await mount((command) => {
    if (command === "queue_list") return [];
    throw { kind: "invalid", message: "That isn't a link. Paste a full address like https://…" };
  });
  await type("not a url");
  await submit();
  assert.equal(container.querySelector('[role="alert"]')?.textContent, "That isn't a link. Paste a full address like https://…");
  assert.equal(field().value, "not a url");
  assert.deepEqual(rows(), []);
});

test("Q2: a duplicate shows a toast, highlights the existing row, adds nothing, and clears the field", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const existing = { id: 7, url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ", ...QUEUED };
  await mount((command) => {
    if (command === "queue_list") return [existing];
    throw { kind: "duplicate", message: "That link is already in the queue.", id: 7 };
  });
  await type("https://youtu.be/dQw4w9WgXcQ");
  await submit();
  assert.deepEqual(rows(), [existing.url], "no second row");
  assert.equal(field().value, "");
  assert.equal(container.querySelector('[data-role="toast"]')?.textContent, "That link is already in the queue.");
  assert.equal(container.querySelector('[role="alert"]'), null, "not an error");
  assert.ok(container.querySelector("li[data-highlight]"), "existing row highlighted");
  await act(async () => t.mock.timers.tick(3000));
  assert.equal(container.querySelector('[data-role="toast"]'), null, "toast goes away");
  assert.equal(container.querySelector("li[data-highlight]"), null);
});

// Independent QA additions for #5 / Q2: asynchronous replies and toast lifecycle.
test("Q2: a delayed duplicate reply preserves the next link and shows no second row", async (t) => {
  const reply = deferred();
  t.after(() => reply.resolve(null));
  const existing = { id: 1, url: "https://example.test/first", ...QUEUED };
  await mount((command) => command === "queue_list" ? [existing] : reply.promise);
  await type(existing.url);
  await submit("button");
  await type("https://例え.jp/次のリンク");
  await act(async () => reply.reject({ kind: "duplicate", message: "Already queued", id: 1 }));
  assert.equal(field().value, "https://例え.jp/次のリンク");
  assert.deepEqual(rows(), [existing.url]);
  assert.equal(document.activeElement, field());
  assert.equal(container.querySelector('[data-role="toast"]')?.textContent, "Already queued");
});

test("Q2: another duplicate replaces the highlight and restarts the full toast duration", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const items = [1, 2].map((id) => ({ id, url: `https://example.test/${id}`, ...QUEUED }));
  await mount((command, args) => {
    if (command === "queue_list") return items;
    const item = items.find((item) => item.url === args.url);
    throw { kind: "duplicate", message: `Already queued ${item.id}`, id: item.id };
  });
  await type(items[0].url);
  await submit();
  await act(async () => t.mock.timers.tick(2000));
  await type(items[1].url);
  await submit();
  assert.equal(container.querySelectorAll("li[data-highlight]").length, 1);
  assert.equal(container.querySelector("li[data-highlight] [data-role=url]").textContent, items[1].url);
  await act(async () => t.mock.timers.tick(1000));
  assert.equal(container.querySelector('[data-role="toast"]')?.textContent, "Already queued 2", "the first toast's timer cannot clear the second");
  await act(async () => t.mock.timers.tick(1999));
  assert.ok(container.querySelector('[data-role="toast"]'));
  await act(async () => t.mock.timers.tick(1));
  assert.equal(container.querySelector('[data-role="toast"]'), null);
  assert.equal(container.querySelector("li[data-highlight]"), null);
  assert.deepEqual(rows(), items.map((item) => item.url));
});

test("Q2: invalid input through + can be corrected; Unicode errors remain literal text", async () => {
  const message = '<img src=x onerror="bad()"> 無効なリンク 🎵';
  await mount((command, args) => {
    if (command === "queue_list") return [];
    if (args.url === "bad") throw { kind: "invalid", message };
    return { id: 1, url: args.url, ...QUEUED };
  });
  await type("bad");
  await submit("button");
  assert.equal(container.querySelector('[role="alert"]')?.textContent, message);
  assert.equal(field().value, "bad");
  assert.equal(document.activeElement, field());
  assert.equal(container.querySelectorAll("img,script").length, 0);
  assert.equal(container.querySelector('[data-role="toast"]'), null);
  await type("https://例え.jp/🎵?title=CON");
  await submit("button");
  assert.deepEqual(rows(), ["https://例え.jp/🎵?title=CON"]);
  assert.equal(field().value, "");
  assert.equal(container.querySelector('[role="alert"]'), null);
});

test("Q2: a duplicate clears an earlier inline error and safely renders the toast message", async () => {
  const existing = { id: 3, url: "https://example.test/CON", ...QUEUED };
  const message = "<script>bad()</script> & 日本語 already queued";
  await mount((command, args) => {
    if (command === "queue_list") return [existing];
    if (args.url === "bad") throw { kind: "invalid", message: "Invalid link" };
    throw { kind: "duplicate", message, id: existing.id };
  });
  await type("bad");
  await submit();
  assert.ok(container.querySelector('[role="alert"]'));
  await type(existing.url);
  await submit();
  assert.equal(container.querySelector('[role="alert"]'), null);
  assert.equal(container.querySelector('[data-role="toast"]')?.textContent, message);
  assert.equal(container.querySelectorAll("img,script").length, 0);
  assert.deepEqual(rows(), [existing.url]);
});

test("Q2: a duplicate rejected before the startup snapshot highlights the row when it arrives", async (t) => {
  const list = deferred();
  t.after(() => list.resolve([]));
  const existing = { id: 4, url: "https://example.test/video", ...QUEUED };
  await mount((command) => {
    if (command === "queue_list") return list.promise;
    throw { kind: "duplicate", message: "Already queued", id: existing.id };
  });
  await type(existing.url);
  await submit();
  assert.deepEqual(rows(), []);
  assert.equal(field().value, "");
  await act(async () => list.resolve([existing]));
  assert.deepEqual(rows(), [existing.url]);
  assert.equal(container.querySelector("li[data-highlight] [data-role=url]").textContent, existing.url);
});

// #6 (Q3): rows fetch metadata first; the queue://item event fills in title, channel, duration, thumbnail.
const FETCHING = { status: { state: "fetching" }, rev: 1, meta: null };
const text = (sel) => container.querySelector(sel)?.textContent ?? null;

test("Q3: a new row says Fetching info without a Download button, then fills in from the event", async () => {
  const row = { id: 1, url: "https://www.youtube.com/watch?v=jNQXAC9IVRw", ...FETCHING };
  await mount((command) => (command === "queue_list" ? [row] : null));
  assert.deepEqual(statusText(), ["Fetching info…"]);
  assert.equal(downloadButtons().length, 0, "no download before the info is in");
  const meta = { title: "Me at the zoo", channel: "jawed", duration: 19, thumbnail: "https://i.ytimg.com/vi/x/hq.jpg", site: "Youtube" };
  await act(async () => emit("queue://item", { ...row, status: { state: "queued" }, meta, rev: 2 }));
  assert.equal(text('[data-role="title"]'), "Me at the zoo");
  assert.equal(text('[data-role="byline"]'), "jawed · 0:19");
  const img = container.querySelector("li img");
  assert.equal(img.getAttribute("src"), meta.thumbnail);
  assert.equal(img.getAttribute("referrerpolicy"), "no-referrer");
  assert.equal(img.getAttribute("alt"), "");
  assert.equal(downloadButtons().length, 1);
  assert.deepEqual(rows(), [row.url], "the URL stays visible under the title");
});

test("Q3: a failed fetch shows yt-dlp's reason and offers no download", async () => {
  const row = { id: 1, url: "https://example.test/private", ...FETCHING };
  await mount((command) => (command === "queue_list" ? [row] : null));
  await act(async () => emit("queue://item", { ...row, status: { state: "failed", reason: "Private video" }, rev: 2 }));
  assert.deepEqual(statusText(), ["Failed: Private video"]);
  assert.equal(downloadButtons().length, 0);
});

test("Q3: untrusted metadata renders as text; long durations use h:mm:ss; missing fields are omitted", async () => {
  const meta = { title: '<img src=x onerror="alert(1)">', channel: null, duration: 3723, thumbnail: null, site: null };
  const rowsIn = [
    { id: 1, url: "https://example.test/a", status: { state: "queued" }, rev: 2, meta },
    { id: 2, url: "https://example.test/b", status: { state: "queued" }, rev: 2, meta: { title: null, channel: "C", duration: null, thumbnail: null, site: null } },
  ];
  await mount((command) => (command === "queue_list" ? rowsIn : null));
  assert.equal(container.querySelectorAll("li img").length, 0, "no element created from the title");
  assert.equal(container.querySelector('[data-role="title"]').textContent, meta.title);
  const bylines = [...container.querySelectorAll('[data-role="byline"]')].map((b) => b.textContent);
  assert.deepEqual(bylines, ["1:02:03", "C"]);
  assert.equal(container.querySelectorAll('[data-role="title"]').length, 1, "no title row when there is no title");
});

// Independent QA for #6 / Q3: metadata races and display boundaries.
test("Q3 QA: metadata event survives the later Fetching add reply", async (t) => {
  const reply = deferred();
  t.after(() => reply.resolve(null));
  const row = { id: 1, url: "https://example.test/a", ...FETCHING };
  await mount((command) => command === "queue_list" ? [] : reply.promise);
  await type(row.url);
  await submit();
  const meta = { title: "日本語 🎵 e\u{301} – CON", channel: "作者", duration: 0, thumbnail: null, site: null };
  await act(async () => emit("queue://item", { ...row, status: { state: "queued" }, meta, rev: 2 }));
  await act(async () => reply.resolve(row));
  assert.deepEqual(rows(), [row.url]);
  assert.equal(text('[data-role="title"]'), meta.title);
  assert.equal(text('[data-role="byline"]'), "作者 · 0:00");
  assert.equal(downloadButtons().length, 1);
});

test("Q3 QA: stale startup snapshot and old events cannot erase fetched metadata", async (t) => {
  const list = deferred();
  t.after(() => list.resolve([]));
  const row = { id: 1, url: "https://example.test/a", ...FETCHING };
  await mount((command) => command === "queue_list" ? list.promise : null);
  const meta = { title: "Ready", channel: null, duration: 59.6, thumbnail: "https://cdn.example.test/a.jpg", site: null };
  await act(async () => emit("queue://item", { ...row, status: { state: "queued" }, meta, rev: 2 }));
  await act(async () => list.resolve([row]));
  await act(async () => emit("queue://item", row));
  assert.equal(text('[data-role="title"]'), "Ready");
  assert.equal(text('[data-role="byline"]'), "1:00");
  assert.equal(container.querySelector("li img").getAttribute("loading"), "lazy");
  assert.equal(downloadButtons().length, 1);
});

test("Q3 QA: fetch failure stays literal and another metadata row can complete", async () => {
  const pending = [1, 2].map((id) => ({ id, url: `https://example.test/${id}`, ...FETCHING }));
  await mount((command) => command === "queue_list" ? pending : null);
  const reason = '<script>bad()</script> 日本語 unavailable';
  await act(async () => emit("queue://item", { ...pending[0], status: { state: "failed", reason }, rev: 2 }));
  const meta = { title: "CON", channel: '<img onerror="bad()"> 作者', duration: null, thumbnail: null, site: null };
  await act(async () => emit("queue://item", { ...pending[1], status: { state: "queued" }, meta, rev: 2 }));
  assert.deepEqual(statusText(), [`Failed: ${reason}`, ""]);
  assert.equal(text('[data-role="byline"]'), meta.channel);
  assert.equal(container.querySelectorAll("script,img").length, 0);
  assert.equal(downloadButtons().length, 1);
});

test("Q3 QA: duration formatting covers rounding boundaries and invalid inputs", async () => {
  const { formatDuration } = await server.ssrLoadModule("/src/queue.ts");
  for (const [value, expected] of [
    [null, ""], [undefined, ""], [NaN, ""], [Infinity, ""], [-1, ""],
    [0, "0:00"], [0.49, "0:00"], [0.5, "0:01"], [59.5, "1:00"],
    [3599.5, "1:00:00"], [3600, "1:00:00"], [3723, "1:02:03"], [86400, "24:00:00"],
  ]) assert.equal(formatDuration(value), expected, `duration ${value}`);
});

// #48 (D-017): a 403 retry shows its attempt; the row returns to the normal flow afterwards.
test("D-017: a retrying download says which attempt it is on", async () => {
  const base = { id: 1, url: "https://example.test/v", meta: null };
  await mount((command) => (command === "queue_list" ? [{ ...base, status: { state: "downloading" }, rev: 3, attempt: 1 }] : null));
  assert.deepEqual(statusText(), ["Downloading…"]);
  await act(async () => emit("queue://item", { ...base, status: { state: "downloading" }, rev: 4, attempt: 2 }));
  assert.deepEqual(statusText(), ["Retrying (2/3)…"]);
  await act(async () => emit("queue://item", { ...base, status: { state: "done", path: "/d/V.webm" }, rev: 5, attempt: 0 }));
  assert.deepEqual(statusText(), ["✓ V.webm"]);
});

test("D-017 QA #48: both backoffs keep the next item disabled; final failure unlocks it", async () => {
  const base = { id: 1, url: "https://example.test/v", meta: null };
  const next = { id: 2, url: "https://example.test/next", meta: null, attempt: 0, ...QUEUED };
  await mount((command) => command === "queue_list" ? [
    { ...base, status: { state: "downloading" }, rev: 3, attempt: 1 }, next,
  ] : null);
  for (const attempt of [2, 3]) {
    await act(async () => emit("queue://item", {
      ...base, status: { state: "downloading" }, rev: attempt + 2, attempt,
    }));
    assert.equal(statusText()[0], `Retrying (${attempt}/3)…`);
    assert.equal(downloadButtons().length, 1);
    assert.equal(downloadButtons()[0].disabled, true);
  }
  await act(async () => emit("queue://item", {
    ...base, status: { state: "failed", reason: "HTTP Error 403: Forbidden" }, rev: 6, attempt: 0,
  }));
  assert.equal(statusText()[0], "Failed: HTTP Error 403: Forbidden");
  assert.equal(downloadButtons()[0].disabled, false);
});

test("D-017 QA #48: late replies and retry events cannot regress the attempt or terminal state", async (t) => {
  const reply = deferred();
  t.after(() => reply.resolve(null));
  const base = { id: 1, url: "https://example.test/v", meta: null };
  await mount((command) => command === "queue_list"
    ? [{ ...base, status: { state: "queued" }, rev: 2, attempt: 0 }]
    : reply.promise);
  await act(async () => downloadButtons()[0].click());
  await act(async () => emit("queue://item", {
    ...base, status: { state: "downloading" }, rev: 5, attempt: 3,
  }));
  await act(async () => reply.resolve({
    ...base, status: { state: "downloading" }, rev: 3, attempt: 1,
  }));
  await act(async () => emit("queue://item", {
    ...base, status: { state: "downloading" }, rev: 4, attempt: 2,
  }));
  assert.equal(statusText()[0], "Retrying (3/3)…");
  await act(async () => emit("queue://item", {
    ...base, status: { state: "done", path: "/d/日本語 é.webm" }, rev: 6, attempt: 0,
  }));
  await act(async () => emit("queue://item", {
    ...base, status: { state: "downloading" }, rev: 5, attempt: 3,
  }));
  assert.deepEqual(statusText(), ["✓ 日本語 é.webm"]);
});

// #7 (Q4): "Download all" starts the backend runner; queue://runner mirrors it.
const allButton = () => [...container.querySelectorAll("button")].find((b) => /Download all|Downloading all/.test(b.textContent));

test("Q4: Download all is enabled only with queued rows, starts the runner, and tracks its state", async () => {
  const calls = [];
  const rowsIn = [{ id: 1, url: "https://example.test/a", ...QUEUED, meta: null, attempt: 0 }];
  await mount((command) => {
    calls.push(command);
    if (command === "queue_list") return rowsIn;
    if (command === "queue_run_all") return true;
    return null;
  });
  assert.equal(allButton().textContent, "Download all");
  assert.equal(allButton().disabled, false);
  await act(async () => allButton().click());
  assert.ok(calls.includes("queue_run_all"));
  // The backend emits queue://runner {running: true} before it replies (lib.rs queue_run_all).
  await act(async () => emit("queue://runner", { running: true }));
  assert.equal(allButton().textContent, "Downloading all…");
  assert.equal(allButton().disabled, true, "no second run while one is active");
  assert.equal(downloadButtons()[0].disabled, true, "per-row Download waits for the run");
  await act(async () => emit("queue://runner", { running: false }));
  assert.equal(allButton().textContent, "Download all");
});

test("Q4: Download all is disabled with nothing queued", async () => {
  const fetching = [{ id: 1, url: "https://example.test/a", status: { state: "fetching" }, rev: 1, meta: null, attempt: 0 }];
  await mount((command) => (command === "queue_list" ? fetching : null));
  assert.equal(allButton().disabled, true);
});

test("Q4: a run already in progress at startup shows as running", async () => {
  mockIPC((command) => {
    if (command === "queue_list") return [];
    if (command === "queue_runner_state") return true;
    return null;
  }, { shouldMockEvents: true });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(App)));
  assert.equal(allButton().textContent, "Downloading all…");
});

// Independent QA for #7 / Q4,Q5: runner replies and events have no item-style revision.
async function mountRunnerQA(handler) {
  mockIPC(handler, { shouldMockEvents: true });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(App)));
}

const runnerRows = () => [{ id: 1, url: "https://example.test/ready", ...QUEUED, meta: null, attempt: 0 }];

test("Q4 QA #7: a finish event before the start reply must leave the runner stopped", async (t) => {
  const reply = deferred();
  t.after(() => reply.resolve(false));
  await mount((command) => command === "queue_list" ? runnerRows() : reply.promise);
  await act(async () => allButton().click());
  await act(async () => emit("queue://runner", { running: true }));
  await act(async () => emit("queue://runner", { running: false }));
  await act(async () => reply.resolve(true));
  assert.equal(allButton().textContent, "Download all", "a late acknowledgement cannot revive a finished run");
  assert.equal(allButton().disabled, false);
  assert.equal(downloadButtons()[0].disabled, false);
});

test("Q4 QA #7: a stale false startup snapshot cannot override a newer running event", async (t) => {
  const snapshot = deferred();
  t.after(() => snapshot.resolve(false));
  await mountRunnerQA((command) => command === "queue_list" ? runnerRows() : snapshot.promise);
  await act(async () => emit("queue://runner", { running: true }));
  await act(async () => snapshot.resolve(false));
  assert.equal(allButton().textContent, "Downloading all…");
  assert.equal(allButton().disabled, true);
  assert.equal(downloadButtons()[0].disabled, true);
});

test("Q4 QA #7: a stale true startup snapshot cannot override a newer finish event", async (t) => {
  const snapshot = deferred();
  t.after(() => snapshot.resolve(false));
  await mountRunnerQA((command) => command === "queue_list" ? runnerRows() : snapshot.promise);
  await act(async () => emit("queue://runner", { running: false }));
  await act(async () => snapshot.resolve(true));
  assert.equal(allButton().textContent, "Download all");
  assert.equal(allButton().disabled, false);
  assert.equal(downloadButtons()[0].disabled, false);
});

test("Q4 QA #7: runner listener failure remains visible after a successful add", async () => {
  await mountRaw((command, args) => {
    if (command === "plugin:event|listen") {
      if (args.event === "queue://runner") throw "runner listener unavailable";
      return 1;
    }
    if (command === "queue_add") return runnerRows()[0];
    return null;
  });
  assert.match(container.querySelector('[role="alert"]')?.textContent ?? "", /runner listener unavailable/);
  await type("https://example.test/ready");
  await submit();
  assert.equal(allButton().disabled, true);
  assert.equal(downloadButtons()[0].disabled, true);
  assert.match(container.querySelector('[role="alert"]')?.textContent ?? "", /runner listener unavailable/,
    "a successful add must not erase the reason downloads are permanently disabled");
});

test("Q4 QA #7: rejected Download all shows literal Unicode error and permits another attempt", async () => {
  const reason = '<script>bad()</script> 日本語 unavailable';
  await mount((command) => {
    if (command === "queue_list") return runnerRows();
    throw reason;
  });
  await act(async () => allButton().click());
  assert.equal(container.querySelector('[role="alert"]').textContent, reason);
  assert.equal(container.querySelectorAll("script,img").length, 0);
  assert.equal(allButton().disabled, false);
  assert.equal(downloadButtons()[0].disabled, false);
});

test("Q4 QA #7: duplicate start acknowledgement does not stop a running event", async (t) => {
  const reply = deferred();
  t.after(() => reply.resolve(false));
  await mount((command) => command === "queue_list" ? runnerRows() : reply.promise);
  await act(async () => allButton().click());
  await act(async () => emit("queue://runner", { running: true }));
  await act(async () => reply.resolve(false));
  assert.equal(allButton().textContent, "Downloading all…", "false acknowledges an existing run; it must not stop it");
  assert.equal(allButton().disabled, true);
});

test("Q4 QA #7: a row added while running stays disabled until the runner stops", async () => {
  await mount((command) => command === "queue_list" ? [] : runnerRows()[0]);
  await act(async () => emit("queue://runner", { running: true }));
  await type("https://example.test/ready");
  await submit();
  assert.equal(allButton().disabled, true);
  assert.equal(downloadButtons()[0].disabled, true);
  await act(async () => emit("queue://runner", { running: false }));
  assert.equal(allButton().disabled, false);
  assert.equal(downloadButtons()[0].disabled, false);
});

// #8 (D4): live progress from queue://item events; then Processing; then Done.
test("D4: a downloading row shows percent, speed, ETA and a progress bar, then Processing, then Done", async () => {
  const base = { id: 1, url: "https://example.test/v", meta: null, attempt: 1 };
  await mount((command) => (command === "queue_list" ? [{ ...base, status: { state: "downloading" }, rev: 3, progress: null }] : null));
  assert.deepEqual(statusText(), ["Downloading…"]);
  assert.equal(container.querySelector('[role="progressbar"]'), null, "no bar until a percent is known");
  const progress = { percent: 42.7, downloaded: 427, total: 1000, speed: 3_200_000, eta: 35, part: 1 };
  await act(async () => emit("queue://item", { ...base, status: { state: "downloading" }, rev: 4, progress }));
  assert.deepEqual(statusText(), ["Downloading… 42% · 3.2 MB/s · 0:35 left"]);
  assert.equal(container.querySelector('[role="progressbar"]').getAttribute("aria-valuenow"), "42");
  await act(async () => emit("queue://item", { ...base, status: { state: "downloading" }, rev: 5, progress: { ...progress, percent: 10, speed: 950, eta: 0, part: 2 } }));
  assert.deepEqual(statusText(), ["Downloading… 10% · 950 B/s (part 2)"]);
  await act(async () => emit("queue://item", { ...base, status: { state: "processing" }, rev: 6, progress: null }));
  assert.deepEqual(statusText(), ["Processing…"]);
  assert.equal(container.querySelector('[role="progressbar"]'), null);
  assert.equal(allButton().disabled, true, "processing still holds the slot");
  await act(async () => emit("queue://item", { ...base, status: { state: "done", path: "/d/V.webm" }, rev: 7, attempt: 0, progress: null }));
  assert.deepEqual(statusText(), ["✓ V.webm"]);
});

test("D4 QA #8: retry downloads keep live percent, speed and ETA visible", async () => {
  const item = { id: 1, url: "https://example.test/retry", meta: null, attempt: 2, rev: 5,
    status: { state: "downloading" },
    progress: { percent: 25, downloaded: 25, total: 100, speed: 1000, eta: 75, part: 1 } };
  await mount((command) => command === "queue_list" ? [item] : null);
  const text = statusText()[0];
  assert.match(text, /Retrying \(2\/3\)/);
  assert.match(text, /25%/, "D4 percent remains visible during an active retry");
  assert.match(text, /1\.0 KB\/s/);
  assert.match(text, /1:15 left/);
});

test("D4 QA #8: unknown total shows available speed and ETA with no numeric bar", async () => {
  const item = { id: 1, url: "https://example.test/live", meta: null, attempt: 1, rev: 4,
    status: { state: "downloading" },
    progress: { percent: null, downloaded: 500, total: null, speed: 0, eta: 3661, part: 1 } };
  await mount((command) => command === "queue_list" ? [item] : null);
  assert.deepEqual(statusText(), ["Downloading… · 0 B/s · 1:01:01 left"]);
  assert.equal(container.querySelector('[role="progressbar"]'), null);
});

test("D4 QA #8: stale progress cannot undo Processing or terminal failure", async () => {
  const base = { id: 1, url: "https://example.test/v", meta: { title: '日本語 🎵 CON <script>bad()</script>', channel: null, duration: null, thumbnail: null }, attempt: 1 };
  const active = { ...base, rev: 4, status: { state: "downloading" },
    progress: { percent: 50, downloaded: 50, total: 100, speed: 20, eta: 2, part: 2 } };
  const queued = { id: 2, url: "https://example.test/b", meta: null, attempt: 0, rev: 2, status: { state: "queued" } };
  await mount((command) => command === "queue_list" ? [active, queued] : null);
  await act(async () => emit("queue://item", { ...base, rev: 5, status: { state: "processing" }, progress: null }));
  await act(async () => emit("queue://item", active));
  assert.equal(statusText()[0], "Processing…");
  assert.equal(downloadButtons()[0].disabled, true);
  assert.equal(container.querySelector('[role="progressbar"]'), null);
  assert.equal(container.querySelector('[data-role="title"]').textContent, base.meta.title);
  assert.equal(container.querySelector("script"), null);
  await act(async () => emit("queue://item", { ...base, attempt: 0, rev: 6,
    status: { state: "failed", reason: "ffmpeg failed" }, progress: null }));
  await act(async () => emit("queue://item", active));
  assert.equal(statusText()[0], "Failed: ffmpeg failed");
  assert.equal(downloadButtons()[0].disabled, false);
});

// #9 / Q6: remove and retry.
const buttonsIn = (index) => [...container.querySelectorAll("li")[index].querySelectorAll("button")].map((b) => b.textContent);
const clickIn = async (index, label) => {
  const b = [...container.querySelectorAll("li")[index].querySelectorAll("button")].find((x) => x.textContent === label);
  assert.ok(b, `row ${index} has ${label}`);
  await act(async () => b.click());
};

test("Q6: a failed row offers Retry and ✕; Retry queues it again for Download", async () => {
  const row = { id: 1, url: "https://example.test/a", status: { state: "failed", reason: "Video unavailable" }, rev: 3, meta: { title: "A", channel: null, duration: null, thumbnail: null, site: null }, attempt: 0 };
  const calls = [];
  await mount((command, args) => {
    calls.push([command, args]);
    if (command === "queue_list") return [row];
    if (command === "queue_retry") return { ...row, status: { state: "queued" }, rev: 4 };
  });
  assert.deepEqual(statusText(), ["Failed: Video unavailable"]);
  assert.deepEqual(buttonsIn(0), ["Retry", "✕"]);
  await clickIn(0, "Retry");
  assert.deepEqual(calls.at(-1), ["queue_retry", { id: 1 }]);
  assert.deepEqual(buttonsIn(0), ["Download", "✕"]);
});

test("Q6: a retried fetch failure shows Fetching info again", async () => {
  const row = { id: 1, url: "https://example.test/a", status: { state: "failed", reason: "timed out" }, rev: 2, meta: null, attempt: 0 };
  await mount((command) => {
    if (command === "queue_list") return [row];
    if (command === "queue_retry") return { ...row, status: { state: "fetching" }, rev: 3 };
  });
  await clickIn(0, "Retry");
  assert.deepEqual(statusText(), ["Fetching info…"]);
  assert.deepEqual(buttonsIn(0), ["✕"]);
});

test("Q6: ✕ is offered on every row except one downloading or processing", async () => {
  const states = [{ state: "fetching" }, { state: "queued" }, { state: "downloading" }, { state: "processing" }, { state: "done", path: "/d/x.webm" }, { state: "failed", reason: "no" }];
  const list = states.map((status, i) => ({ id: i + 1, url: `https://example.test/${i}`, status, rev: 1, meta: null, attempt: status.state === "downloading" ? 1 : 0 }));
  await mount((command) => (command === "queue_list" ? list : null));
  const hasRemove = list.map((_, i) => buttonsIn(i).includes("✕"));
  assert.deepEqual(hasRemove, [true, true, false, false, true, true]);
  const x = container.querySelector('li button[aria-label="Remove"]');
  assert.equal(x.getAttribute("title"), "Remove from queue");
});

test("Q6: removing a row drops it, and later events or snapshots can't bring it back", async (t) => {
  const a = { id: 1, url: "https://example.test/a", ...FETCHING, attempt: 0 };
  const b = { id: 2, url: "https://example.test/b", ...QUEUED, meta: null, attempt: 0 };
  const calls = [];
  await mount((command, args) => {
    calls.push([command, args]);
    if (command === "queue_list") return [a, b];
    if (command === "queue_remove") return null;
  });
  await clickIn(0, "✕");
  assert.deepEqual(calls.at(-1), ["queue_remove", { id: 1 }]);
  assert.deepEqual(rows(), [b.url]);
  await act(async () => emit("queue://item", { ...a, status: { state: "queued" }, rev: 9 }));
  assert.deepEqual(rows(), [b.url], "a late fetch event stays dropped");
  await act(async () => emit("queue://item", { ...b, status: { state: "downloading" }, rev: 2, attempt: 1 }));
  assert.deepEqual(statusText(), ["Downloading…"], "other rows still update");
});

test("Q6: a rejected remove or retry keeps the row and shows the reason literally", async () => {
  const row = { id: 1, url: "https://example.test/a", status: { state: "failed", reason: "<b>x</b>" }, rev: 2, meta: null, attempt: 0 };
  await mount((command) => {
    if (command === "queue_list") return [row];
    if (command === "queue_retry") throw "That link is already in the queue again.";
    if (command === "queue_remove") throw "A download in progress can't be removed yet.";
  });
  await clickIn(0, "Retry");
  assert.equal(container.querySelector('[role="alert"]').textContent, "That link is already in the queue again.");
  await clickIn(0, "✕");
  assert.equal(container.querySelector('[role="alert"]').textContent, "A download in progress can't be removed yet.");
  assert.deepEqual(rows(), [row.url]);
  assert.equal(container.querySelectorAll("li b").length, 0, "the reason is text, not markup");
});

test("Q6 QA #9: a delayed startup snapshot cannot resurrect a removed row", async (t) => {
  const snapshot = deferred();
  const row = { id: 1, url: "https://example.test/removed", ...FETCHING, attempt: 0 };
  t.after(() => snapshot.resolve([]));
  await mount((command) => {
    if (command === "queue_list") return snapshot.promise;
    if (command === "queue_remove") return null;
    assert.fail(`unexpected command ${command}`);
  });
  await act(async () => emit("queue://item", row));
  await clickIn(0, "✕");
  assert.deepEqual(rows(), []);
  await act(async () => snapshot.resolve([{ ...row, status: { state: "queued" }, rev: 2 }]));
  assert.deepEqual(rows(), [], "a stale initial snapshot stays tombstoned");
});

test("Q6 QA #9: a late retry reply cannot regress a newer failed event", async (t) => {
  const retry = deferred();
  const row = { id: 1, url: "https://example.test/retry", meta: null, attempt: 0,
    status: { state: "failed", reason: "old failure" }, rev: 2 };
  t.after(() => retry.resolve({ ...row, rev: 3, status: { state: "fetching" } }));
  await mount((command) => command === "queue_list" ? [row] : retry.promise);
  await clickIn(0, "Retry");
  await act(async () => emit("queue://item", { ...row, rev: 4,
    status: { state: "failed", reason: "new failure 日本語" } }));
  await act(async () => retry.resolve({ ...row, rev: 3, status: { state: "fetching" } }));
  assert.deepEqual(statusText(), ["Failed: new failure 日本語"]);
  assert.deepEqual(buttonsIn(0), ["Retry", "✕"]);
});

test("Q6 QA #9: removing while Retry is pending cannot be undone by its reply", async (t) => {
  const retry = deferred();
  const row = { id: 1, url: "https://example.test/retry-remove", meta: null, attempt: 0,
    status: { state: "failed", reason: "old failure" }, rev: 2 };
  t.after(() => retry.resolve({ ...row, rev: 3, status: { state: "fetching" } }));
  await mount((command) => {
    if (command === "queue_list") return [row];
    if (command === "queue_retry") return retry.promise;
    if (command === "queue_remove") return null;
    assert.fail(`unexpected command ${command}`);
  });
  await clickIn(0, "Retry");
  await clickIn(0, "✕");
  await act(async () => retry.resolve({ ...row, rev: 3, status: { state: "fetching" } }));
  assert.deepEqual(rows(), []);
});

test("Q6 QA #9: retry remains usable during Download all and newer download events win", async (t) => {
  const retry = deferred();
  const metadata = { title: '日本語 🎵 <img src=x onerror="bad()"> CON', channel: null,
    duration: null, thumbnail: null, site: null };
  const row = { id: 1, url: "https://example.test/retry-all", meta: metadata, attempt: 0,
    status: { state: "failed", reason: "old failure" }, rev: 4 };
  t.after(() => retry.resolve({ ...row, rev: 5, status: { state: "queued" } }));
  await mount((command) => command === "queue_list" ? [row] : retry.promise);
  await act(async () => emit("queue://runner", { running: true }));
  const retryButton = [...container.querySelectorAll("li button")].find((b) => b.textContent === "Retry");
  assert.equal(retryButton.disabled, false);
  await clickIn(0, "Retry");
  await act(async () => emit("queue://item", { ...row, rev: 6, attempt: 1,
    status: { state: "downloading" } }));
  await act(async () => retry.resolve({ ...row, rev: 5, status: { state: "queued" } }));
  assert.deepEqual(statusText(), ["Downloading…"]);
  assert.deepEqual(buttonsIn(0), []);
  assert.equal(container.querySelector('[data-role="title"]').textContent, metadata.title);
  assert.equal(container.querySelectorAll("li img").length, 0);
});

// #10 / D1: the download folder, shown and changed through the OS picker (mocked here).
const folderText = () => container.querySelector('[data-role="folder"]').textContent;
const changeButton = () => [...container.querySelectorAll("button")].find((b) => b.textContent === "Change…");

test("D1: the current download folder is shown, and Change… saves the picked one", async () => {
  const calls = [];
  const picked = { path: "C:\\Users\\Me\\Vidéos 日本語", error: null };
  await mount((command) => {
    calls.push(command);
    if (command === "queue_list") return [];
    if (command === "settings_choose_download_folder") return picked;
  });
  assert.equal(folderText(), DEFAULT_FOLDER.path);
  await act(async () => changeButton().click());
  assert.equal(calls.filter((c) => c === "settings_choose_download_folder").length, 1);
  assert.equal(folderText(), picked.path);
  assert.equal(container.querySelector('[data-role="folder"]').getAttribute("title"), picked.path);
  assert.equal(container.querySelector('[data-role="folder-error"]'), null);
});

test("D1: cancelling the picker keeps the folder; a failed save shows the reason literally", async () => {
  let reply = null;
  await mount((command) => {
    if (command === "queue_list") return [];
    if (command === "settings_choose_download_folder") {
      if (reply instanceof Error) throw reply.message;
      return reply;
    }
  });
  await act(async () => changeButton().click());
  assert.equal(folderText(), DEFAULT_FOLDER.path, "cancel changes nothing");
  reply = new Error("Couldn't save your settings: <b>denied</b>");
  await act(async () => changeButton().click());
  assert.equal(text('[data-role="folder-error"]'), "Couldn't save your settings: <b>denied</b>");
  assert.equal(container.querySelectorAll("p b").length, 0);
  assert.equal(folderText(), DEFAULT_FOLDER.path);
  assert.equal(changeButton().disabled, false, "can try again");
});

test("D1: a chosen folder that's gone is still named, with the reason", async () => {
  mockIPC((command) => {
    if (command === "queue_list") return [];
    if (command === "queue_runner_state") return false;
    if (command === "settings_download_folder")
      return { path: "/Volumes/USB/Videos", error: "The download folder /Volumes/USB/Videos isn't available. Choose another with Change…." };
    return null;
  }, { shouldMockEvents: true });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(App)));
  assert.equal(folderText(), "/Volumes/USB/Videos");
  assert.match(text('[data-role="folder-error"]'), /isn't available/);
});

test("D1: Change… is disabled while the picker is open", async (t) => {
  const pending = deferred();
  await mount((command) => {
    if (command === "queue_list") return [];
    if (command === "settings_choose_download_folder") return pending.promise;
  });
  await act(async () => changeButton().click());
  assert.equal(changeButton().disabled, true);
  await act(async () => pending.resolve({ path: "/new", error: null }));
  assert.equal(changeButton().disabled, false);
  assert.equal(folderText(), "/new");
});

test("D1: cancelling the picker leaves an unresolved startup error showing; a pick clears it", async () => {
  let reply = null;
  mockIPC((command) => {
    if (command === "queue_list") return [];
    if (command === "queue_runner_state") return false;
    if (command === "settings_download_folder") throw "settings lock poisoned";
    if (command === "settings_choose_download_folder") return reply;
    return null;
  }, { shouldMockEvents: true });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(App)));
  assert.equal(text('[data-role="folder-error"]'), "settings lock poisoned");
  await act(async () => changeButton().click());
  assert.equal(text('[data-role="folder-error"]'), "settings lock poisoned", "cancel keeps it");
  reply = { path: "/picked", error: null };
  await act(async () => changeButton().click());
  assert.equal(container.querySelector('[data-role="folder-error"]'), null);
  assert.equal(folderText(), "/picked");
});

// #36: the Settings panel (PRD §6.4), in the app's single window.
const settingsPanel = () => container.querySelector('[role="dialog"][data-role="settings"]');
const gear = () => container.querySelector('button[aria-label="Settings"]');

test("#36: the gear opens Settings with the download folder; ✕ closes it and returns focus", async () => {
  await mount((command) => (command === "queue_list" ? [] : null));
  assert.equal(settingsPanel(), null);
  gear().focus();
  await act(async () => gear().click());
  const panel = settingsPanel();
  assert.ok(panel);
  assert.equal(panel.getAttribute("aria-modal"), "true");
  assert.equal(panel.querySelector("#settings-title").textContent, "Settings");
  assert.equal(panel.querySelector('[data-role="folder"]').textContent, DEFAULT_FOLDER.path);
  assert.equal(document.activeElement.getAttribute("aria-label"), "Close settings", "focus moves in");
  assert.ok(gear().closest("[inert]"), "the queue behind the panel is inert");
  assert.equal(panel.closest("[inert]"), null);
  await act(async () => panel.querySelector('button[aria-label="Close settings"]').click());
  assert.equal(settingsPanel(), null);
  assert.equal(gear().closest("[inert]"), null);
  assert.equal(document.activeElement, gear(), "focus goes back to the gear");
});

test("#36: Escape closes Settings; Ctrl+, opens it (⌘, on macOS)", async () => {
  await mount((command) => (command === "queue_list" ? [] : null));
  const mac = gear().getAttribute("title").includes("⌘");
  const other = mac ? { ctrlKey: true } : { metaKey: true };
  await act(async () => window.dispatchEvent(new window.KeyboardEvent("keydown", { key: ",", ...other, bubbles: true })));
  assert.equal(settingsPanel(), null, "the other OS's modifier does nothing");
  const own = mac ? { metaKey: true } : { ctrlKey: true };
  await act(async () => window.dispatchEvent(new window.KeyboardEvent("keydown", { key: ",", ...own, bubbles: true })));
  assert.ok(settingsPanel());
  await act(async () =>
    document.activeElement.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Escape", bubbles: true })),
  );
  assert.equal(settingsPanel(), null);
  assert.match(gear().getAttribute("title"), /^Settings \((Ctrl\+,|⌘,)\)$/);
});

test("#36: a folder changed in Settings shows on the main 'Saving to' line too", async () => {
  await mount((command) => {
    if (command === "queue_list") return [];
    if (command === "settings_choose_download_folder") return { path: "/Volumes/Media/Video 日本語", error: null };
  });
  await act(async () => gear().click());
  const change = [...settingsPanel().querySelectorAll("button")].find((b) => b.textContent === "Change…");
  await act(async () => change.click());
  const shown = [...container.querySelectorAll('[data-role="folder"]')].map((f) => f.textContent);
  assert.deepEqual(shown, ["/Volumes/Media/Video 日本語", "/Volumes/Media/Video 日本語"], "main line and panel agree");
});

test("#36: the folder is read once, however many places show it", async () => {
  let reads = 0;
  mockIPC((command) => {
    if (command === "queue_list") return [];
    if (command === "queue_runner_state") return false;
    if (command === "settings_download_folder") {
      reads++;
      return DEFAULT_FOLDER;
    }
    return null;
  }, { shouldMockEvents: true });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(App)));
  await act(async () => gear().click());
  await act(async () => settingsPanel().querySelector('button[aria-label="Close settings"]').click());
  await act(async () => gear().click());
  assert.equal(reads, 1, "opening Settings doesn't read it again");
  assert.equal(settingsPanel().querySelector('[data-role="folder"]').textContent, DEFAULT_FOLDER.path);
});

test("#36: Escape closes Settings even when focus has left the panel, with or without ⌘/Ctrl", async () => {
  await mount((command) => (command === "queue_list" ? [] : null));
  for (const mods of [{}, { metaKey: true }, { ctrlKey: true }]) {
    await act(async () => gear().click());
    assert.ok(settingsPanel());
    await act(async () => document.activeElement.blur()); // e.g. a click on the panel background
    assert.equal(document.activeElement, document.body);
    await act(async () => document.body.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Escape", ...mods, bubbles: true })));
    assert.equal(settingsPanel(), null, JSON.stringify(mods));
  }
});
