// Independent QA: #39 / D7. Real React components with mocked Tauri IPC.
// OS labels are exercised in isolated modules; native file managers are not launched.
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
const apps = new Map();
const servers = [];
let root, container;
const original = { path: "/original 日本語", error: null };
const selected = { path: "C:\\Vidéos 日本語 e\u0301 & 'quote'", error: null };
const platforms = [
  ["MacIntel", "Show in Finder", "Finder"],
  ["Win32", "Show in File Explorer", "File Explorer"],
  ["Linux x86_64", "Show in folder", "your file manager"],
];

before(async () => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  try {
    for (const [platform] of platforms) {
      Object.defineProperty(globalThis, "navigator", { configurable: true, value: { platform } });
      const server = await createServer({
        root: fileURLToPath(new URL("../../", import.meta.url)), configFile: false,
        logLevel: "silent", appType: "custom",
        server: { middlewareMode: true, hmr: false, watch: null },
      });
      servers.push(server);
      apps.set(platform, (await server.ssrLoadModule("/src/App.tsx")).default);
    }
  } finally {
    if (previous) Object.defineProperty(globalThis, "navigator", previous);
    else delete globalThis.navigator;
  }
});
afterEach(async () => {
  if (root) await act(async () => root.unmount());
  root = undefined;
  container?.remove();
  clearMocks();
});
after(async () => {
  await Promise.all(servers.map((server) => server.close()));
  dom.window.close();
  delete globalThis.window;
  delete globalThis.document;
  delete globalThis.IS_REACT_ACT_ENVIRONMENT;
});

function item(id, status, rev = 1) {
  return { id, url: `https://example.test/${id}`, rev, status, meta: null, attempt: 0, progress: null };
}
function done(id = 61, path = "/original 日本語/_CON e\u0301 (2).webm") {
  return item(id, { state: "done", path }, 5);
}
function deferred() {
  let resolve, reject;
  const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}
async function mount(handler = () => undefined, platform = "Win32") {
  mockIPC((command, args) => {
    const reply = handler(command, args);
    if (reply !== undefined) return reply;
    if (command === "queue_list") return [];
    if (command === "queue_runner_state") return false;
    if (command === "settings_download_folder") return original;
    return null;
  }, { shouldMockEvents: true });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(apps.get(platform))));
}
const button = (text, scope = container) => [...scope.querySelectorAll("button")].find((b) => b.textContent === text);
const rows = () => [...container.querySelectorAll("li")];
async function click(element) { assert.ok(element, "button exists"); await act(async () => element.click()); }
async function update(row) { await act(async () => emit("queue://item", row)); }
const alert = () => container.querySelector('[role="alert"]');

for (const [platform, label, manager] of platforms) {
  test(`#39 D7 QA: ${platform} renders native wording and only Done rows reveal their own id`, async () => {
    const saved = done(61, "C:\\Vidéos\\日本語 e\u0301 🎵 & 'quote' _CON (2).mp4");
    saved.meta = { title: "NUL 🔥 <script>untrusted</script>", channel: null, duration: null, thumbnail: null };
    const list = [
      item(4, { state: "fetching" }), item(12, { state: "queued" }),
      item(22, { state: "downloading" }), item(31, { state: "processing" }),
      item(47, { state: "failed", reason: "timeout" }), saved,
      done(82, "/older folder/_NUL (3).opus"),
    ];
    const calls = [];
    await mount((command, args) => {
      if (command === "queue_list") return list;
      if (command === "reveal_download" || command === "open_download_folder") {
        calls.push([command, args]); return null;
      }
    }, platform);
    assert.equal(button("Open folder").title, `Open the download folder in ${manager}`);
    assert.equal(button("Open folder").type, "button");
    assert.deepEqual(rows().map((row) => !!button(label, row)), [false, false, false, false, false, true, true]);
    const reveal = button(label, rows()[5]);
    assert.equal(reveal.disabled, false, "reveal works while another row is downloading");
    assert.equal(reveal.type, "button");
    assert.ok(rows()[5].textContent.includes("✓ 日本語 e\u0301 🎵 & 'quote' _CON (2).mp4"));
    assert.equal(rows()[5].querySelector("script"), null);
    await click(reveal);
    await click(button(label, rows()[6]));
    await click(button("Open folder"));
    assert.deepEqual(calls, [["reveal_download", { id: 61 }], ["reveal_download", { id: 82 }], ["open_download_folder", {}]]);
    assert.equal(rows().length, list.length);
    if (platform !== "MacIntel") assert.ok(!container.textContent.includes("Finder"));
  });
}

test("#39 D7 QA: 100% and Processing offer no reveal; Done wins over a late snapshot and late progress", async (t) => {
  const snapshot = deferred();
  t.after(() => snapshot.resolve([]));
  const calls = [];
  await mount((command, args) => {
    if (command === "queue_list") return snapshot.promise;
    if (command === "reveal_download") { calls.push(args); return null; }
  });
  const active = item(61, { state: "downloading" }, 2);
  active.progress = { percent: 100, speed: null, eta: 0, part: 1 };
  await update(active);
  assert.equal(button("Show in File Explorer"), undefined);
  await update(item(61, { state: "processing" }, 3));
  assert.equal(button("Show in File Explorer"), undefined);
  const saved = done();
  await update(saved);
  await act(async () => snapshot.resolve([active]));
  await update(active);
  await click(button("Show in File Explorer"));
  assert.deepEqual(calls, [{ id: 61 }]);
  assert.equal(rows()[0].getAttribute("data-state"), "done");
});

test("#39 D7 QA: Open folder uses the backend's current choice while old rows still reveal by id", async () => {
  let current = original;
  const calls = [];
  await mount((command, args) => {
    if (command === "queue_list") return [done()];
    if (command === "settings_download_folder") return current;
    if (command === "settings_choose_download_folder") { current = selected; return selected; }
    if (command === "open_download_folder") { calls.push([command, args, current.path]); return null; }
    if (command === "reveal_download") { calls.push([command, args]); return null; }
  });
  await click(button("Open folder"));
  await click(button("Change…"));
  assert.equal(container.querySelector('[data-role="folder"]').textContent, selected.path);
  await click(button("Open folder"));
  await click(button("Show in File Explorer"));
  assert.deepEqual(calls, [
    ["open_download_folder", {}, original.path], ["open_download_folder", {}, selected.path],
    ["reveal_download", { id: 61 }],
  ]);
});

test("#39 D7 QA: unavailable folder errors are literal and choosing a usable folder permits recovery", async () => {
  const message = "The download folder <img src=x> 日本語 isn't available. Choose another with Change….";
  let usable = false;
  await mount((command) => {
    if (command === "settings_download_folder") return { path: "Z:\\offline", error: message };
    if (command === "open_download_folder") { if (!usable) throw message; return null; }
    if (command === "settings_choose_download_folder") { usable = true; return selected; }
  });
  await click(button("Open folder"));
  assert.equal(alert().textContent, message);
  assert.equal(container.querySelector("img"), null);
  await click(button("Change…"));
  await click(button("Open folder"));
  assert.equal(alert(), null);
  assert.equal(container.querySelector('[data-role="folder-error"]'), null);
});

test("#39 D7 QA: moved-file and opener failures keep the Done row and a subsequent reveal recovers", async () => {
  const messages = [
    "_CON 日本語 (2).webm isn't there anymore. It may have been moved or deleted.",
    "Couldn't show C:\\Vidéos\\<b>literal</b>.webm: denied & timeout",
  ];
  let attempt = 0;
  await mount((command) => {
    if (command === "queue_list") return [done()];
    if (command === "reveal_download") { if (attempt < messages.length) throw messages[attempt++]; return null; }
  });
  for (const message of messages) {
    await click(button("Show in File Explorer"));
    assert.equal(alert().textContent, message);
    assert.equal(rows()[0].getAttribute("data-state"), "done");
    assert.equal(container.querySelector("b"), null);
  }
  await click(button("Show in File Explorer"));
  assert.equal(alert(), null);
  assert.equal(rows().length, 1);
});

test("#39 D7 QA: removing a Done row during a pending reveal cannot resurrect it", async (t) => {
  const pending = deferred();
  t.after(() => pending.resolve(null));
  let reveals = 0;
  await mount((command) => {
    if (command === "queue_list") return [done()];
    if (command === "reveal_download") { reveals++; return pending.promise; }
  });
  await click(button("Show in File Explorer"));
  await click(rows()[0].querySelector('button[aria-label="Remove"]'));
  await act(async () => pending.resolve(null));
  await update(done());
  assert.equal(reveals, 1);
  assert.equal(rows().length, 0);
  assert.equal(button("Show in File Explorer"), undefined);
});

test("#39 D7 QA: slow Open folder leaves queue events responsive and its rejection is recoverable", async (t) => {
  const pending = deferred();
  t.after(() => pending.resolve(null));
  let calls = 0;
  await mount((command) => {
    if (command === "queue_list") return [item(61, { state: "processing" }, 3)];
    if (command === "open_download_folder") return ++calls === 1 ? pending.promise : null;
  });
  await click(button("Open folder"));
  await update(done());
  assert.ok(button("Show in File Explorer"));
  await act(async () => pending.reject("Couldn't open 日本語: file manager timed out"));
  assert.equal(alert().textContent, "Couldn't open 日本語: file manager timed out");
  await click(button("Open folder"));
  assert.equal(alert(), null);
  assert.equal(rows()[0].getAttribute("data-state"), "done");
});
