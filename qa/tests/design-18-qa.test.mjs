// Independent #18 / D-045 QA: real React DOM with mocked IPC. These cover dynamic
// surfaces outside the screenshot fixture. CSS media queries and native focus rendering
// still require the three OS webviews; jsdom cannot establish their visual appearance.
import assert from "node:assert/strict";
import { after, afterEach, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { JSDOM } from "jsdom";
import React, { act } from "react";
import { createServer } from "vite";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

const dom = new JSDOM("<!doctype html><html><body></body></html>", { url: "http://localhost/" });
globalThis.window = dom.window;
globalThis.document = dom.window.document;
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const { createRoot } = await import("react-dom/client");
let server, QueueList, ApiSetup, root, container;
before(async () => {
  server = await createServer({
    root: fileURLToPath(new URL("../../", import.meta.url)),
    configFile: false, logLevel: "silent", appType: "custom",
    server: { middlewareMode: true, hmr: false, watch: null },
  });
  ({ default: QueueList } = await server.ssrLoadModule("/src/components/QueueList.tsx"));
  ({ default: ApiSetup } = await server.ssrLoadModule("/src/components/ApiSetup.tsx"));
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
function mount(handler = () => null) {
  mockIPC(handler);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
}
const row = (state = "queued", extra = {}) => ({
  id: 1, url: "https://example.test/1", status: { state }, rev: 1,
  meta: { title: "Café 日本語", channel: "Björk", duration: 65, thumbnail: null, site: null },
  name: "Café 日本語", name_source: "rules", attempt: 1, ...extra,
});
async function render(items, extra = {}) {
  await act(async () => root.render(React.createElement(QueueList, {
    items, onDownload() {}, onRetry() {}, onRename() {}, onRemove() {},
    onCancel() {}, onReveal() {}, connected: true, busy: false,
    cancelling: new Set(), highlight: null, ...extra,
  })));
}
const find = (selector) => container.querySelector(selector);
const key = (target, value) => act(async () => target.dispatchEvent(new dom.window.KeyboardEvent(
  "keydown", { key: value, bubbles: true, cancelable: true },
)));

test("#18 QA: messy Unicode and markup stay literal in titles, names, channels and failure reasons", async () => {
  mount();
  const hostile = '🔥 日本語 e\u0301 <img src=x onerror="alert(1)"> & <script>bad()</script>';
  const reason = `${hostile} ${"long reason ".repeat(100)}`;
  await render([row("failed", {
    status: { state: "failed", reason }, name: hostile,
    meta: { title: hostile, channel: hostile, duration: 65, thumbnail: null },
  })]);
  assert.equal(find('[data-role="title"]').textContent, hostile);
  assert.equal(find('[data-role="title"]').title, hostile, "untruncated tooltip");
  assert.equal(find('[data-role="name"]').textContent, hostile);
  assert.equal(find('[data-role="byline"]').textContent, `${hostile} · 1:05`);
  assert.equal(find('[role="status"]').textContent, `Failed: ${reason}`);
  assert.equal(find('[role="status"]').title, reason);
  assert.equal(find("img, script"), null, "website and error text cannot create elements");
});

test("#18 QA: Unicode collision filenames are shown intact for Windows and POSIX output paths", async () => {
  mount();
  for (const path of ["C:\\Users\\me\\Downloads\\日本語 – Café (2).m4a", "/tmp/日本語 – Café (2).m4a"]) {
    await render([row("done", { status: { state: "done", path } })]);
    const done = [...container.querySelectorAll("span")].find((el) => el.title === path);
    assert.equal(done?.textContent, "✓ 日本語 – Café (2).m4a");
    assert.equal(done.title, path);
  }
});

test("#18 QA: failed thumbnails are isolated by row identity across reorder and metadata changes", async () => {
  mount();
  const src = "https://cdn.example.test/same.jpg";
  const a = row("queued", { meta: { title: "A", thumbnail: src } });
  const b = row("queued", { id: 2, meta: { title: "B", thumbnail: src } });
  await render([a, b]);
  await act(async () => find("li img").dispatchEvent(new dom.window.Event("error")));
  let rows = [...container.querySelectorAll("li")];
  assert.equal(rows[0].querySelector("img"), null);
  assert.equal(rows[1].querySelector("img")?.getAttribute("src"), src);
  await render([b, a]);
  rows = [...container.querySelectorAll("li")];
  assert.equal(rows[0].querySelector("img")?.getAttribute("src"), src);
  assert.equal(rows[1].querySelector("img"), null, "failure follows its row, not position");
  await render([row("queued", { meta: { title: "A", thumbnail: null } })]);
  assert.equal(find("img"), null);
  assert.ok(find("li > div").className.includes("bg-neutral-200"));
  await render([row("queued", { meta: { title: "A", thumbnail: "https://cdn.example.test/new.jpg" } })]);
  assert.equal(find("img")?.getAttribute("src"), "https://cdn.example.test/new.jpg");
});

test("#18 QA: a reused row drops stale spinners and progress through retry and completion", async () => {
  mount();
  const lifecycle = [
    [row("fetching", { meta: null, name: null }), true, false, "Fetching info…"],
    [row(), false, false, null],
    [row("downloading", { progress: { percent: 42.9, speed: 3200000, eta: 35, part: 2 } }), false, true, "42%"],
    [row("processing"), true, false, "Processing…"],
    [row("failed", { status: { state: "failed", reason: "ffmpeg failed" } }), false, false, "Failed: ffmpeg failed"],
    [row("fetching", { meta: null, name: null }), true, false, "Fetching info…"],
    [row("done", { status: { state: "done", path: "/tmp/Café (2).mp4" } }), false, false, "✓ Café (2).mp4"],
  ];
  for (const [item, spinner, progress, text] of lifecycle) {
    await render([item]);
    assert.equal(!!find('[data-role="spinner"]'), spinner, item.status.state);
    assert.equal(!!find('[role="progressbar"]'), progress, item.status.state);
    if (text) assert.ok(container.textContent.includes(text), text);
    if (progress) {
      assert.equal(find('[role="progressbar"]').getAttribute("aria-valuenow"), "42");
      assert.equal(find('[role="progressbar"] > div').style.width, "42.9%");
      assert.ok(container.textContent.includes("3.2 MB/s · 0:35 left (part 2)"));
    }
  }
});

test("#18 QA: missing progress never invents a bar or numbers", async () => {
  mount();
  for (const progress of [undefined, null, { percent: null, speed: null, eta: null, part: 1 }]) {
    await render([row("downloading", { progress })]);
    assert.equal(find('[role="progressbar"]'), null);
    assert.ok(container.textContent.includes("Downloading…"));
    assert.doesNotMatch(container.textContent, /NaN|undefined|null|Infinity|\d+%/);
  }
});

test("#18 QA: fallback and AI badges follow naming updates without labelling manual or rules names", async () => {
  mount();
  for (const state of ["queued", "processing", "done"]) {
    for (const [source, label] of [["ai", "AI"], ["fallback", "rules"], ["rules", null], ["edited", null]]) {
      await render([row(state, {
        name_source: source,
        status: state === "done" ? { state, path: "/tmp/Café.mp4" } : { state },
      })]);
      const tag = find('[data-role="name-tag"]');
      assert.equal(tag?.textContent.trim() ?? null, label, `${state}: ${source}`);
      if (source === "fallback") assert.match(tag.title, /didn't give a usable name/);
    }
  }
});

test("#18 QA: queued and done notices keep their full text beside a readable status", async () => {
  mount();
  const notice = `Compatibility fallback: 日本語 ${"message ".repeat(100)}`;
  for (const state of ["queued", "done"]) {
    await render([row(state, {
      notice, status: state === "done" ? { state, path: "/tmp/Café.mp4" } : { state },
    })]);
    assert.equal(find('[data-role="notice"]').textContent, notice);
    assert.ok(find('[data-role="notice"]').className.includes("whitespace-normal"));
    if (state === "done") assert.ok(container.textContent.includes("✓ Café.mp4"));
  }
});

test("#18 QA: row focus survives status/highlight changes and keyboard navigation still works", async () => {
  mount();
  const items = [row(), row("queued", { id: 2 })];
  const left = [];
  const onLeave = () => left.push(true);
  await render(items, { onLeave });
  const first = find("li");
  first.focus();
  await render([row("processing"), items[1]], { highlight: 1, onLeave });
  assert.equal(document.activeElement, first);
  assert.ok(first.hasAttribute("data-highlight"));
  await key(first, "ArrowDown");
  assert.equal(document.activeElement, container.querySelectorAll("li")[1]);
  await key(document.activeElement, "Escape");
  assert.equal(left.length, 1);
  await render(items, { onLeave });
  assert.equal(find("[data-highlight]"), null);
});

test("#18 QA: a timeout after a successful API test removes the check mark and renders the reason literally", async () => {
  let reject;
  const pending = new Promise((_, no) => { reject = no; });
  let tests = 0;
  mount((command) => {
    if (command === "settings_api") return {
      kind: "openai_compatible", baseUrl: "https://api.example.test/v1", model: "m",
      keySet: false, keyHost: "api.example.test", storeError: null,
    };
    if (command === "api_test_connection") {
      return ++tests === 1 ? { ok: true, message: "Connected." } : pending;
    }
    return null;
  });
  await act(async () => root.render(React.createElement(ApiSetup)));
  await act(async () => find('[data-role="api-test"]').click());
  assert.equal(find('[data-role="api-test-ok"]')?.textContent.trim(), "✓");
  await act(async () => find('[data-role="api-test"]').click());
  assert.equal(find('[data-role="api-test-ok"]'), null, "no stale success while waiting");
  assert.equal(find('[data-role="api-test-result"]'), null);
  assert.equal(find('[data-role="api-test"]').disabled, true);
  const reason = 'Timed out: <img src=x onerror="bad()"> 日本語';
  await act(async () => reject(reason));
  assert.equal(find('[data-role="api-test-ok"]'), null);
  assert.equal(find('[data-role="api-test-result"]').textContent, reason);
  assert.equal(find('[data-role="api-test-result"]').getAttribute("role"), "status");
  assert.equal(find('[data-role="api-test"]').disabled, false);
  assert.equal(find("img"), null);
});
