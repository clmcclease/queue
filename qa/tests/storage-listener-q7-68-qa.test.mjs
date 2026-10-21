// Independent QA for #68 / Q7 / D-025. Real React effects and DOM; controlled
// Tauri IPC replies and virtual polling time. No native webview coverage.
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

const WARNING = "Queue couldn't save the list (磁碟已滿 / café 🎵), so recent changes would be lost if it quit now. It keeps trying.";
function deferred() {
  let resolve, reject;
  const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}

function backend({ listen = "fail", read = () => null } = {}) {
  const active = new Map();
  const registrations = [];
  let id = 0, reads = 0;
  const handler = (command, args) => {
    if (command === "plugin:event|listen") {
      const register = () => {
        const eventId = ++id;
        active.set(eventId, { event: args.event, handler: args.handler });
        return eventId;
      };
      if (args.event === "queue://storage") {
        if (listen === "fail") throw new Error("storage listener unavailable");
        if (listen === "defer") {
          const reply = deferred();
          registrations.push({ reject: reply.reject, succeed: () => reply.resolve(register()) });
          return reply.promise;
        }
      }
      return register();
    }
    if (command === "plugin:event|unlisten") {
      assert.ok(active.delete(args.eventId), "only an active listener may be removed");
      return null;
    }
    if (command === "queue_storage_problem") return read(++reads);
    if (command === "queue_list") return [];
    if (command === "queue_runner_state") return false;
    if (command === "settings_download_folder") return { path: "/tmp/Downloads", error: null };
    if (command === "settings_naming_template") return "title"; // #17
    if (command === "naming_previews") return [];
    if (command === "settings_format") return "default";
    throw new Error(`unexpected command ${command}`);
  };
  const emit = (payload) => {
    for (const [eventId, listener] of active) {
      if (listener.event === "queue://storage") {
        window.__TAURI_INTERNALS__.runCallback(listener.handler, {
          event: listener.event, id: eventId, payload,
        });
      }
    }
  };
  return { handler, registrations, active, emit, get reads() { return reads; } };
}

async function mount(ipc, strict = false) {
  mockIPC(ipc.handler);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  const app = React.createElement(App);
  await act(async () => root.render(strict ? React.createElement(React.StrictMode, null, app) : app));
}

const warning = () => container.querySelector('[data-role="unsaved"]')?.textContent ?? null;
const tick = async (t, ms = 2000) => act(async () => t.mock.timers.tick(ms));

test("Q7 #68 QA: initial Unicode warning is literal, survives a rejected poll, and clears on recovery", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const problem = `${WARNING} <img src=x onerror=alert(1)>`;
  const ipc = backend({ read: (n) => {
    if (n === 1) return problem;
    if (n === 2) throw new Error("status temporarily unavailable");
    return null;
  } });
  await mount(ipc);
  assert.equal(warning(), problem);
  assert.equal(container.querySelector('[data-role="unsaved"]').getAttribute("role"), "alert");
  assert.equal(container.querySelector('[data-role="unsaved"] img'), null);
  await tick(t);
  assert.equal(warning(), problem, "a failed read must not imply that saving recovered");
  await tick(t);
  assert.equal(warning(), null);
});

test("Q7 #68 QA: rejected startup/status reads do not stop subsequent polling", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const ipc = backend({ read: (n) => {
    if (n < 3) return Promise.reject(new Error("IPC temporarily unavailable"));
    return WARNING;
  } });
  await mount(ipc);
  await tick(t);
  await tick(t);
  assert.equal(ipc.reads, 3);
  assert.equal(warning(), WARNING);
});

test("Q7 #68 QA: healthy event delivery wins over a delayed startup read and needs no polling", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const startup = deferred();
  const ipc = backend({ listen: "ok", read: () => startup.promise });
  await mount(ipc);
  await act(async () => ipc.emit(WARNING));
  await act(async () => startup.resolve(null));
  assert.equal(warning(), WARNING);
  await tick(t, 10000);
  assert.equal(ipc.reads, 1, "successful registration must not start the fallback timer");
  await act(async () => ipc.emit(null));
  assert.equal(warning(), null);
});

test("Q7 #68 QA: StrictMode registration failures leave only one polling loop, stopped on unmount", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const ipc = backend({ read: () => WARNING });
  await mount(ipc, true);
  const before = ipc.reads;
  await tick(t);
  assert.equal(ipc.reads - before, 1, "the discarded StrictMode effect must not poll");
  assert.equal(warning(), WARNING);
  await act(async () => root.unmount());
  root = undefined;
  const atUnmount = ipc.reads;
  await tick(t, 10000);
  assert.equal(ipc.reads, atUnmount);
  assert.equal(ipc.active.size, 0);
});

test("Q7 #68 QA: an obsolete StrictMode listener failure cannot start polling beside the live listener", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const ipc = backend({ listen: "defer" });
  await mount(ipc, true);
  assert.equal(ipc.registrations.length, 2);
  await act(async () => ipc.registrations[1].succeed());
  await act(async () => ipc.registrations[0].reject(new Error("late obsolete failure")));
  const before = ipc.reads;
  await act(async () => ipc.emit(WARNING));
  await tick(t, 10000);
  assert.equal(ipc.reads, before);
  assert.equal(warning(), WARNING);
});

test("Q7 #68 QA: a poll pending at unmount cannot affect a fresh mount", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const pending = deferred();
  const old = backend({ read: (n) => n === 1 ? null : pending.promise });
  await mount(old);
  await tick(t);
  assert.equal(old.reads, 2);
  await act(async () => root.unmount());
  root = undefined;
  container.remove();
  const fresh = backend({ read: () => WARNING });
  await mount(fresh);
  await act(async () => pending.resolve(null));
  assert.equal(warning(), WARNING);
  await tick(t);
  assert.equal(old.reads, 2);
  assert.equal(fresh.reads, 2);
});

for (const [name, older, newer] of [
  ["save failure", null, WARNING],
  ["save recovery", WARNING, null],
]) {
  test(`Q7 #68 QA: delayed startup status cannot overwrite a newer polled ${name}`, async (t) => {
    t.mock.timers.enable({ apis: ["setInterval"] });
    const startup = deferred();
    const ipc = backend({ read: (n) => n === 1 ? startup.promise : newer });
    await mount(ipc);
    await tick(t);
    assert.equal(warning(), newer);
    await act(async () => startup.resolve(older));
    assert.equal(warning(), newer, "the older startup snapshot must not replace a later status");
  });

  test(`Q7 #68 QA: overlapping polls cannot overwrite a newer ${name} with an older reply`, async (t) => {
    t.mock.timers.enable({ apis: ["setInterval"] });
    const slowPoll = deferred();
    const ipc = backend({ read: (n) => n === 2 ? slowPoll.promise : n >= 3 ? newer : older });
    await mount(ipc);
    await tick(t);
    await tick(t);
    assert.equal(warning(), newer);
    await act(async () => slowPoll.resolve(older));
    assert.equal(warning(), newer, "an out-of-order poll reply must not replace a later status");
  });
}

// #68 review round 1 (qa/reports/20261018-114333-review-ndOG.md).
test("Q7 #68: replies slower than the poll interval still show the warning, then clear it", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const replies = [];
  const ipc = backend({ read: () => { const reply = deferred(); replies.push(reply); return reply.promise; } });
  await mount(ipc);
  await tick(t); // a second read starts before the first has answered
  assert.equal(replies.length, 2);
  await act(async () => replies[0].resolve(WARNING));
  assert.equal(warning(), WARNING, "a slow reply lands even though a newer read has started");
  await act(async () => replies[1].resolve(WARNING));
  await tick(t);
  await act(async () => replies[2].resolve(null));
  assert.equal(warning(), null, "and a later slow reply clears it");
});

test("Q7 #68: a newer read that fails doesn't discard an older usable reply", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const replies = [];
  const ipc = backend({ read: () => { const reply = deferred(); replies.push(reply); return reply.promise; } });
  await mount(ipc);
  await tick(t);
  await act(async () => replies[1].reject(new Error("read failed")));
  await act(async () => replies[0].resolve(WARNING));
  assert.equal(warning(), WARNING);
});
