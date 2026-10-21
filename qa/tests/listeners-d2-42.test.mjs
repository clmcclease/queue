// #42 / D2: under React.StrictMode, with listener registration delayed, exactly one listener per
// event survives the effect replay and it receives completion events. Real React DOM, mocked
// Tauri IPC with its own listener registry: the stock event mock never removes a listener.
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

const ITEM = { id: 1, url: "https://example.test/a", status: { state: "queued" }, rev: 1 };

// Registrations wait in `pending` until the test releases them; `active` holds what's
// registered and not yet unlistened, as the backend would.
function backend() {
  const pending = [];
  const active = new Map();
  let nextId = 0;
  const handler = (command, args) => {
    if (command === "plugin:event|listen") {
      return new Promise((resolve) => {
        const release = () => {
          const id = ++nextId;
          active.set(id, { event: args.event, handler: args.handler });
          resolve(id);
        };
        release.event = args.event;
        pending.push(release);
      });
    }
    if (command === "plugin:event|unlisten") {
      assert.ok(active.delete(args.eventId), `unlisten of unknown listener ${args.eventId}`);
      return null;
    }
    if (command === "queue_list") return [ITEM];
    if (command === "queue_runner_state") return false;
    if (command === "settings_download_folder") return { path: "/Users/me/Downloads", error: null };
    if (command === "settings_naming_template") return "title"; // #17
    if (command === "naming_previews") return [];
    if (command === "settings_format") return "default";
    if (command === "queue_storage_problem") return null;
    throw new Error(`unexpected ${command}`);
  };
  const count = (event) => [...active.values()].filter((l) => l.event === event).length;
  // Delivers to every registered listener for `event`, as the backend would.
  const emit = (event, payload) => {
    for (const [id, l] of active) {
      if (l.event === event) window.__TAURI_INTERNALS__.runCallback(l.handler, { event, id, payload });
    }
  };
  return { pending, active, handler, count, emit };
}

for (const order of ["oldest", "newest"]) {
  test(`D2 #42 QA: unmount before registration resolves (${order} first) leaves no listeners`, async () => {
    const ipc = backend();
    await mountStrict(ipc);
    assert.ok(ipc.pending.length >= 2);
    await act(async () => root.unmount());
    root = undefined;
    await releaseAll(ipc, order);
    assert.equal(ipc.pending.length, 0);
    assert.equal(ipc.active.size, 0, "late registrations must be unlistened after unmount");
  });
}

test("D2 #42 QA: unmount while runner registration is pending cleans both event streams", async () => {
  const ipc = backend();
  await mountStrict(ipc);
  // Resolve only item subscriptions: the replay's first is discarded, and the live one
  // starts subscribing to the runner. Storage and runner registration are still pending.
  while (ipc.pending.some((release) => release.event === "queue://item")) {
    const at = ipc.pending.findIndex((release) => release.event === "queue://item");
    const [release] = ipc.pending.splice(at, 1);
    await act(async () => release());
  }
  assert.equal(ipc.count("queue://item"), 1);
  assert.equal(ipc.count("queue://runner"), 0);
  assert.ok(ipc.pending.some((release) => release.event === "queue://runner"));
  await act(async () => root.unmount());
  root = undefined;
  assert.equal(ipc.count("queue://item"), 0, "already registered subscription is stopped immediately");
  await releaseAll(ipc, "newest");
  assert.equal(ipc.active.size, 0, "pending runner and storage subscriptions are also stopped");
});

test("D2 #42 QA: old delayed registrations cannot leak into a fresh StrictMode mount", async () => {
  const ipc = backend();
  await mountStrict(ipc);
  await act(async () => root.unmount());
  root = undefined;
  container.remove();
  await mountStrict(ipc);
  // Complete the new mount before the old mount's pending registrations.
  await releaseAll(ipc, "newest");
  for (const event of ["queue://item", "queue://runner", "queue://storage"]) {
    assert.equal(ipc.count(event), 1, `${event}: exactly one listener belongs to the fresh mount`);
  }
  await act(async () => ipc.emit("queue://item", {
    ...ITEM, rev: 10, status: { state: "done", path: "/tmp/日本語 🎵.webm" },
  }));
  assert.deepEqual(statusText(), ["✓ 日本語 🎵.webm"]);
  await act(async () => root.unmount());
  root = undefined;
  assert.equal(ipc.active.size, 0);
});

async function mountStrict(ipc) {
  mockIPC(ipc.handler);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(React.StrictMode, null, React.createElement(App))));
}

// Releases held registrations, oldest or newest first, until none are left (each released
// item listener lets that effect go on to register its runner listener).
async function releaseAll(ipc, order) {
  while (ipc.pending.length) {
    const next = order === "newest" ? ipc.pending.pop() : ipc.pending.shift();
    await act(async () => next());
  }
}

const statusText = () => [...container.querySelectorAll("li")].map((row) => row.querySelector("[data-role=url] + div").textContent);

for (const order of ["oldest", "newest"]) {
  test(`D2 #42: StrictMode with delayed registration (${order} first) keeps exactly one listener per event`, async () => {
    const ipc = backend();
    await mountStrict(ipc);
    assert.ok(ipc.pending.length >= 2, "StrictMode's replay started a second registration before the first finished");
    assert.equal(ipc.active.size, 0);
    await releaseAll(ipc, order);

    for (const event of ["queue://item", "queue://runner", "queue://storage"]) {
      assert.equal(ipc.count(event), 1, `${event}: one active listener after the replay's cleanup`);
    }
    assert.equal(ipc.active.size, 3);

    let calls = 0;
    const done = { ...ITEM, rev: 3, status: { state: "done", path: "/Users/me/Downloads/Song.m4a" } };
    await act(async () => {
      calls = ipc.count("queue://item");
      ipc.emit("queue://item", done);
    });
    assert.equal(calls, 1);
    assert.deepEqual(statusText(), ["✓ Song.m4a"], "the surviving listener delivers completion");
    assert.equal(container.querySelectorAll("li").length, 1);

    await act(async () => ipc.emit("queue://runner", { running: true }));
    await act(async () => root.unmount());
    root = undefined;
    assert.equal(ipc.active.size, 0, "unmount removes every listener");
  });
}
