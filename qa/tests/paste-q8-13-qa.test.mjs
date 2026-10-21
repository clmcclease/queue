// Independent QA: #13 / Q8. React DOM + mocked Tauri IPC; no native clipboard coverage.
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

function deferred() {
  let resolve, reject;
  const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}
const item = (id, url) => ({ id, url, rev: 1, status: { state: "fetching" }, meta: null, attempt: 0 });
const field = () => container.querySelector('input[aria-label="Link to download"]');
const rows = () => [...container.querySelectorAll("li")];
const urls = () => rows().map((row) => row.querySelector('[data-role="url"]').textContent);
const text = (selector) => container.querySelector(selector)?.textContent ?? null;

async function mount(handler, initial = []) {
  mockIPC((command, args) => {
    if (command === "queue_list") return initial;
    if (command === "queue_runner_state") return false;
    if (command === "queue_storage_problem") return null;
    if (command === "settings_download_folder") return { path: "/Downloads 日本語", error: null };
    if (command === "settings_naming_template") return "title"; // #17
    if (command === "naming_previews") return [];
    if (command === "settings_format") return "default";
    return handler(command, args);
  }, { shouldMockEvents: true });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(App)));
}
async function type(value) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, "value").set.call(field(), value);
    field().dispatchEvent(new dom.window.Event("input", { bubbles: true }));
  });
}
async function paste(value) {
  const event = new dom.window.Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", {
    value: { getData: (type) => type === "text/plain" ? value : "" },
  });
  await act(async () => field().dispatchEvent(event));
  return event;
}

test("#13: Unicode and encoded delimiters survive splitting; invalid lines reach validation intact", async () => {
  const { splitLinks } = await server.ssrLoadModule("/src/queue.ts");
  const links = [
    "https://例え.jp/音楽/🎵?title=e\u0301&name=CON%20(OFFICIAL%20VIDEO)",
    "https://example.test/a%0Ab?x=+&x=%2B&x=%20#片段",
    "https://example.test/a\tb", "https://example.test/b\0suffix",
  ];
  for (const newline of ["\n", "\r\n", "\r"]) {
    const pasted = `\uFEFF\u3000${links[0]} \t${newline}  ${newline}${links.slice(1).join(newline)}${newline}`;
    assert.deepEqual(splitLinks(pasted), links);
  }
  assert.equal(splitLinks("https://example.test/a%0Ab"), null, "an encoded newline is URL data");
  assert.deepEqual(splitLinks("not a URL\nhttps://example.test/a https://example.test/b"), [
    "not a URL", "https://example.test/a https://example.test/b",
  ], "split by line, never silently extract URLs from an invalid line");
});

test("#13: a delayed add preserves batch order and newly typed text without waiting for metadata", async () => {
  const first = deferred();
  const links = ["https://example.test/first", "https://example.test/second"];
  const calls = [];
  await mount((command, args) => {
    assert.equal(command, "queue_add");
    calls.push(args.url);
    return calls.length === 1 ? first.promise : item(2, args.url);
  });
  await type("draft before paste");
  assert.equal((await paste(links.join("\r\n"))).defaultPrevented, true);
  assert.deepEqual(calls, links.slice(0, 1), "second add waits for first queue insertion");
  await type("draft written while the batch is pending");
  await act(async () => first.resolve(item(1, links[0])));
  assert.deepEqual(calls, links);
  assert.deepEqual(urls(), links);
  assert.deepEqual(rows().map((r) => r.dataset.state), ["fetching", "fetching"]);
  assert.equal(field().value, "draft written while the batch is pending");
  assert.equal(text('[data-role="toast"]'), "Added 2 links.");
});

test("#13: a timed-out IPC reply, invalid line and duplicate do not abort later links", async () => {
  const pending = deferred();
  const links = ["https://example.test/timeout", "bad <img src=x>", "https://example.test/duplicate", "https://example.test/good"];
  const calls = [];
  const existing = item(1, links[2]);
  await mount((command, args) => {
    assert.equal(command, "queue_add");
    calls.push(args.url);
    if (args.url === links[0]) return pending.promise;
    if (args.url === links[1]) throw { kind: "invalid", message: "Not a link" };
    if (args.url === links[2]) throw { kind: "duplicate", message: "Already queued", id: 1 };
    return item(2, args.url);
  }, [existing]);
  await paste(links.join("\n"));
  await act(async () => pending.reject(new Error("IPC request timed out")));
  assert.deepEqual(calls, links);
  assert.deepEqual(urls(), [links[2], links[3]]);
  assert.equal(text('[data-role="toast"]'), "Added 1 of 4 links: 1 already in the queue, 1 not a link, 1 couldn't be added.");
  assert.equal(text('[role="alert"]'), 'Not a link: "bad <img src=x>". Error: IPC request timed out');
  assert.equal(container.querySelector("img"), null);
});

test("#13: repeated URLs within one paste go through normal duplicate handling", async () => {
  const links = ["https://example.test/a", "https://example.test/b"];
  const stored = new Map();
  await mount((command, args) => {
    assert.equal(command, "queue_add");
    if (stored.has(args.url)) throw { kind: "duplicate", message: "Already queued", id: stored.get(args.url).id };
    const added = item(stored.size + 1, args.url);
    stored.set(args.url, added);
    return added;
  });
  await paste([links[0], links[0], links[1], links[0], links[1]].join("\n"));
  assert.deepEqual(urls(), links);
  assert.equal(text('[data-role="toast"]'), "Added 2 of 5 links: 3 already in the queue.");
  assert.equal(text('[role="alert"]'), null);
});

test("#13: metadata arriving before a batch reply wins over stale fetching snapshots", async () => {
  const pending = deferred();
  const links = ["https://example.test/a", "https://example.test/b"];
  await mount((command, args) => {
    assert.equal(command, "queue_add");
    return args.url === links[0] ? pending.promise : item(2, args.url);
  });
  await paste(links.join("\n"));
  const title = 'NUL.txt 日本語 🎵 [4K] <b>OFFICIAL VIDEO</b> e\u0301';
  await act(async () => emit("queue://item", {
    ...item(1, links[0]), rev: 2, status: { state: "queued" },
    meta: { title, channel: "Artist", duration: 123, thumbnail: null, site: "test" },
  }));
  await act(async () => pending.resolve(item(1, links[0])));
  assert.deepEqual(urls(), links);
  assert.deepEqual(rows().map((r) => r.dataset.state), ["queued", "fetching"]);
  assert.equal(text('[data-role="title"]'), title);
  assert.equal(container.querySelector("b"), null, "metadata is rendered as literal text");
});

test("#13: removing an event-created row while its add reply is pending does not resurrect it", async () => {
  const pending = deferred();
  const links = ["https://example.test/a", "https://example.test/b"];
  await mount((command, args) => {
    if (command === "queue_remove") { assert.equal(args.id, 1); return null; }
    assert.equal(command, "queue_add");
    return args.url === links[0] ? pending.promise : item(2, args.url);
  });
  await paste(links.join("\n"));
  await act(async () => emit("queue://item", item(1, links[0])));
  const remove = rows()[0].querySelector('button[aria-label="Remove"]');
  assert.ok(remove, "row can be removed before its command reply");
  await act(async () => remove.click());
  await act(async () => pending.resolve(item(1, links[0])));
  assert.deepEqual(urls(), [links[1]]);
});

test("#13: overlapping paste batches retain every row when replies arrive out of order", async () => {
  const pending = deferred();
  const links = ["https://example.test/a", "https://example.test/b", "https://example.test/c", "https://example.test/d"];
  const calls = [];
  await mount((command, args) => {
    assert.equal(command, "queue_add");
    calls.push(args.url);
    const added = item(calls.length, args.url);
    return args.url === links[0] ? pending.promise : added;
  });
  await paste(links.slice(0, 2).join("\n"));
  await paste(links.slice(2).join("\n"));
  await act(async () => pending.resolve(item(1, links[0])));
  assert.deepEqual(calls, [links[0], links[2], links[3], links[1]]);
  assert.deepEqual(urls(), calls, "rows follow backend IDs without losing either batch");
  assert.equal(new Set(urls()).size, 4);
});

test("#13: a 100-link paste creates all rows, skips blank lines and never downloads automatically", async () => {
  const links = Array.from({ length: 100 }, (_, i) => `https://example.test/video/${i}?title=CON%20日本語`);
  const calls = [];
  await mount((command, args) => {
    assert.equal(command, "queue_add", "pasting must not invoke download commands");
    calls.push(args.url);
    return item(calls.length, args.url);
  });
  await paste(`\r\n${links.map((url) => ` \t${url} `).join("\r\n\r\n")}\r\n`);
  assert.deepEqual(calls, links);
  assert.deepEqual(urls(), links);
  assert.equal(text('[data-role="toast"]'), "Added 100 links.");
  assert.equal(text('[role="alert"]'), null);
});
