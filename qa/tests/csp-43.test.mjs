// #43: the webview's Content Security Policy (D-035). Reads tauri.conf.json; the real webviews
// are the owner's hand check (macOS) and #71's (Windows, Linux).
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const config = JSON.parse(readFileSync(new URL("../../src-tauri/tauri.conf.json", import.meta.url), "utf8"));
const { csp, devCsp } = config.app.security;

function directives(policy) {
  return Object.fromEntries(
    policy.split(";").map((d) => d.trim()).filter(Boolean).map((d) => {
      const [name, ...sources] = d.split(/\s+/);
      return [name, sources];
    }),
  );
}

test("#43: the release policy allows only the app's own scripts and styles", () => {
  assert.equal(typeof csp, "string");
  const d = directives(csp);
  assert.deepEqual(d["default-src"], ["'self'"]);
  assert.deepEqual(d["script-src"], ["'self'"]);
  assert.deepEqual(d["style-src"], ["'self'"]);
  assert.deepEqual(d["object-src"], ["'none'"]);
  assert.deepEqual(d["base-uri"], ["'none'"]);
  assert.deepEqual(d["frame-ancestors"], ["'none'"]);
  assert.doesNotMatch(csp, /unsafe-eval|unsafe-inline|\*/);
});

test("#43: thumbnails from any https site load, and IPC reaches the backend on every OS", () => {
  const d = directives(csp);
  assert.deepEqual(d["img-src"], ["'self'", "https:", "data:"]);
  // ipc: on macOS and Linux, http://ipc.localhost on Windows.
  assert.deepEqual(d["connect-src"], ["ipc:", "http://ipc.localhost"]);
});

test("#43: the dev policy adds only the Vite dev server and its HMR socket", () => {
  const d = directives(devCsp);
  assert.deepEqual(d["connect-src"], ["ipc:", "http://ipc.localhost", "http://localhost:1420", "ws://localhost:1420"]);
  assert.doesNotMatch(devCsp, /unsafe-eval|\*/);
  assert.equal(config.build.devUrl, "http://localhost:1420", "the dev server the policy names");
});

// Codex's #43 review (20261027-103514-review-FOBq): check the HMR socket Vite actually opens.
async function viteServer(host) {
  if (host === undefined) delete process.env.TAURI_DEV_HOST;
  else process.env.TAURI_DEV_HOST = host;
  try {
    // A fresh query string re-evaluates the config, which reads TAURI_DEV_HOST at load.
    const { default: config } = await import(`../../vite.config.ts?host=${host ?? ""}`);
    return config({ command: "serve", mode: "development" }).server;
  } finally {
    delete process.env.TAURI_DEV_HOST;
  }
}

test("#43: without TAURI_DEV_HOST, Vite's HMR socket is the dev server's own, which the dev policy allows", async () => {
  const server = await viteServer(undefined);
  assert.equal(server.hmr, undefined, "HMR shares the page's host and port");
  assert.equal(server.port, 1420);
  assert.ok(directives(devCsp)["connect-src"].includes(`ws://localhost:${server.port}`));
});

test("#43 known limitation (owner-accepted): with TAURI_DEV_HOST set, the dev policy blocks Vite's HMR socket", async () => {
  // Desktop-only today; TAURI_DEV_HOST is for phone and remote-device development. If the dev
  // policy or vite.config.ts starts covering this socket, update this test and STATUS.
  const server = await viteServer("192.0.2.10");
  assert.deepEqual(server.hmr, { protocol: "ws", host: "192.0.2.10", port: 1421 });
  assert.equal(directives(devCsp)["connect-src"].includes("ws://192.0.2.10:1421"), false);
});
