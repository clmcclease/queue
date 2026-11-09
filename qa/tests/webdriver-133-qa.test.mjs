// Independent NFR/#133 QA. Run the real CLIs with disposable, offline process/network
// boundaries. No browser, registry, installed driver, or production file is modified.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import test from "node:test";
import { CHECKS } from "../webdriver/csp-probe-133.mjs";

const runner = new URL("../webdriver/csp-probe-133.mjs", import.meta.url);
const fetcher = new URL("../../scripts/fetch-msedgedriver.mjs", import.meta.url);
const preload = String.raw`
import cp from 'node:child_process';
import { EventEmitter } from 'node:events';
import { appendFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { syncBuiltinESMExports } from 'node:module';
const cfg = JSON.parse(process.env.QUEUE_133_CASE);
const log = event => appendFileSync(process.env.QUEUE_133_EVENTS, JSON.stringify(event) + '\n');
const timer = globalThis.setTimeout;
const abortTimeout = AbortSignal.timeout.bind(AbortSignal);
AbortSignal.timeout = ms => abortTimeout(Math.max(1, Math.ceil(ms / 100)));
// The runner's own polling clock advances 100x. A 30 s deadline takes 300 ms;
// a blocked HTTP request still really blocks. Keep the supervisor timeout real.
const now = Date.now();
const started = performance.now();
Date.now = () => now + (performance.now() - started) * 100;
globalThis.setTimeout = (fn, ms, ...args) => timer(fn, Math.max(1, ms / 100), ...args);
if (cfg.kind === 'fetcher') Object.defineProperty(process, 'platform', { value: 'win32' });
cp.spawn = (file, args, options) => {
  log({ kind: 'spawn', file, args, options });
  const child = new EventEmitter();
  child.kill = () => { log({ kind: 'kill' }); return true; };
  if (cfg.startError) queueMicrotask(() => child.emit('error', new Error('fixture ENOENT')));
  if (cfg.earlyExit) queueMicrotask(() => child.emit('exit', 17, null));
  return child;
};
cp.spawnSync = (file, args, options) => {
  log({ kind: 'spawnSync', file, args, options });
  if (file === 'reg') {
    cfg.regCount = (cfg.regCount || 0) + 1;
    if (cfg.noRuntime || cfg.regCount < (cfg.registryAt || 1)) return { status: 1, stdout: '', stderr: 'not found' };
    return { status: 0, stdout: '    pv    REG_SZ    142.0.3595.53\r\n', stderr: '' };
  }
  if (args[0] === '-xf') {
    if (cfg.extractError) return { status: 1, stderr: 'fixture corrupt zip' };
    if (!cfg.missingExe) writeFileSync(join(cfg.dir, 'msedgedriver.exe'), 'fixture');
    return { status: 0, stdout: '', stderr: '' };
  }
  if (args[0] === '--version') return { status: 0, stdout: cfg.driverVersion || 'MSEdgeDriver 142.0.3595.53 (fixture)', stderr: '' };
  throw new Error('unexpected subprocess ' + file);
};
syncBuiltinESMExports();
globalThis.fetch = async (url, options = {}) => {
  const u = new URL(url);
  const body = options.body ? JSON.parse(options.body) : undefined;
  log({ kind: 'fetch', url: String(url), method: options.method || 'GET', body });
  if (cfg.kind === 'fetcher') {
    return { ok: !cfg.httpError, status: cfg.httpError || 200, arrayBuffer: async () => Uint8Array.from([80, 75, 0, 255]).buffer };
  }
  const path = u.pathname;
  const key = (options.method || 'GET') + ' ' + path;
  if (path === '/status' && (cfg.startError || cfg.earlyExit)) throw new Error('fixture connection refused');
  if (key === cfg.hangAt) {
    // A real stalled response: only a request AbortSignal can finish it. The external
    // test supervisor prevents this regression from hanging npm test itself.
    return await new Promise((_, reject) => {
      if (options.signal?.aborted) reject(options.signal.reason);
      options.signal?.addEventListener('abort', () => reject(options.signal.reason), { once: true });
      const keepAlive = setInterval(() => {}, 100);
      options.signal?.addEventListener('abort', () => clearInterval(keepAlive), { once: true });
    });
  }
  if (key === cfg.errorAt) return { ok: false, status: 500, text: async () => JSON.stringify({ value: { error: 'unknown error', message: 'fixture native driver failure' } }) };
  if (key === cfg.badJsonAt) return { ok: true, status: 200, text: async () => '<html>fixture proxy error</html>' };
  let value;
  if (path === '/status') value = { ready: true };
  else if (path === '/session') value = { sessionId: 'qa-session', capabilities: { browserName: 'fixture', browserVersion: '1.2.3.4' } };
  else if (path.endsWith('/execute/sync')) value = !cfg.pageNeverReady;
  else if (path.endsWith('/execute/async')) value = cfg.report;
  else value = null;
  return { ok: true, status: 200, text: async () => JSON.stringify({ value }) };
};
`;

const passing = () => ({ verdict: "PASS", rows: CHECKS.map(check => ({ check, expected: "blocked", actual: "blocked", ok: "✔" })), logs: [] });

function run(t, cfg = {}, args) {
  const dir = mkdtempSync(join(tmpdir(), "queue-133-QA 日本語 & "));
  t.after(() => rmSync(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 }));
  const loader = join(dir, "offline.mjs");
  const eventsPath = join(dir, "events.jsonl");
  writeFileSync(loader, preload);
  const script = cfg.kind === "fetcher" ? fetcher : runner;
  const result = spawnSync(process.execPath, ["--import", pathToFileURL(loader).href, fileURLToPath(script),
    ...(args ?? (cfg.kind === "fetcher" ? [dir] : ["--app", join(dir, "Queue 日本語 & ; $ (app)"), "--native-driver", join(dir, "native driver"), "--tauri-driver", "fixture-driver"]))], {
    encoding: "utf8", timeout: 5000,
    env: { ...process.env, QUEUE_133_EVENTS: eventsPath, QUEUE_133_CASE: JSON.stringify({ report: passing(), ...cfg, dir }) },
  });
  const events = existsSync(eventsPath) ? readFileSync(eventsPath, "utf8").trim().split("\n").map(JSON.parse) : [];
  return { result, events, dir };
}

test("NFR/#133 QA: full CLI uses W3C requests, preserves Unicode paths as argv, and cleans up", t => {
  const { result, events, dir } = run(t);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /PASS, 9\/9 checks/);
  const spawned = events.find(e => e.kind === "spawn");
  assert.equal(spawned.file, "fixture-driver");
  assert.deepEqual(spawned.args, ["--native-driver", resolve(dir, "native driver")]);
  assert.ok(!spawned.options.shell);
  const requests = events.filter(e => e.kind === "fetch");
  assert.deepEqual(requests.map(e => `${e.method} ${new URL(e.url).pathname}`), [
    "GET /status", "POST /session", "POST /session/qa-session/timeouts",
    "POST /session/qa-session/execute/sync", "POST /session/qa-session/execute/async", "DELETE /session/qa-session",
  ]);
  assert.equal(requests[1].body.capabilities.alwaysMatch["tauri:options"].application, resolve(dir, "Queue 日本語 & ; $ (app)"));
  assert.deepEqual(requests[2].body, { script: 90_000 });
  assert.match(requests[4].body.script, /__TAURI_INTERNALS__\.invoke\("queue_list"\)/);
  assert.equal(events.at(-1).kind, "kill");
});

for (const [name, change] of [
  ["failed check", r => { r.rows[0].ok = "✖"; r.rows[0].actual = "ran"; }],
  ["dropped check", r => r.rows.pop()],
  ["duplicate check", r => r.rows.push(r.rows[0])],
  ["probe error", r => { r.verdict = "ERROR"; r.error = "fixture probe failure"; }],
]) {
  test(`NFR/#133 QA: CLI rejects a ${name} and still deletes the session`, t => {
    const report = passing(); change(report);
    const { result, events } = run(t, { report });
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /CSP PROBE \(WebDriver\): (FAIL|ERROR)/);
    assert.equal(events.at(-2).method, "DELETE");
    assert.equal(events.at(-1).kind, "kill");
  });
}

for (const [name, cfg, reason] of [
  ["missing driver", { startError: true }, /failed to start: fixture ENOENT/],
  ["driver exits early", { earlyExit: true }, /exited \(17\)/],
  ["session creation fails", { errorAt: "POST /session" }, /HTTP 500 unknown error/],
  ["malformed response", { badJsonAt: "POST /session" }, /not JSON: <html>/],
  ["script timeout error", { errorAt: "POST /session/qa-session/execute/async" }, /fixture native driver failure/],
  ["page never ready", { pageNeverReady: true }, /timed out after 60000 ms/],
]) {
  test(`NFR/#133 QA: ${name} fails with a diagnostic and kills the driver`, t => {
    const { result, events } = run(t, cfg);
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, reason);
    assert.equal(events.at(-1).kind, "kill");
  });
}

test("NFR/#133 QA: a DELETE error still kills the driver", t => {
  const { result, events } = run(t, { errorAt: "DELETE /session/qa-session" });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stderr, /ending the session: DELETE/);
  assert.equal(events.at(-1).kind, "kill");
});

for (const hangAt of ["GET /status", "POST /session", "POST /session/qa-session/execute/async", "DELETE /session/qa-session"]) {
  test(`NFR/#133 QA: stalled ${hangAt} is bounded and cannot strand the driver`, t => {
    const { result, events } = run(t, { hangAt });
    assert.ok(events.some(e => e.kind === "fetch" && `${e.method} ${new URL(e.url).pathname}` === hangAt), "fixture reached stalled request");
    assert.equal(result.error?.code, undefined, `runner exceeded supervisor deadline: ${result.error?.message}`);
    assert.equal(events.at(-1).kind, "kill", "cleanup must reach driver.kill even when HTTP stalls");
  });
}

for (const args of [[], ["--app"], ["--app", "x", "--unknown", "y"]]) {
  test(`NFR/#133 QA: invalid arguments ${JSON.stringify(args)} fail before spawn`, t => {
    const { result, events } = run(t, {}, args);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /required|usage:/);
    assert.equal(events.length, 0);
  });
}

test("NFR/#133 QA: Windows fetcher falls back to per-user registry and verifies exact downloaded version", t => {
  const { result, events, dir } = run(t, { kind: "fetcher", registryAt: 3 });
  assert.equal(result.status, 0, result.stderr);
  const commands = events.filter(e => e.kind === "spawnSync");
  assert.equal(commands.filter(e => e.file === "reg").length, 3);
  assert.match(commands[2].args[1], /^HKCU\\/);
  assert.equal(events.find(e => e.kind === "fetch").url, "https://msedgedriver.microsoft.com/142.0.3595.53/edgedriver_win64.zip");
  assert.deepEqual(commands[3].args, ["-xf", join(dir, "edgedriver_win64.zip"), "-C", dir, "msedgedriver.exe"]);
  assert.deepEqual(commands[4].args, ["--version"]);
  assert.ok(commands.every(e => !e.options?.shell));
  assert.match(result.stdout, /sha256 [a-f0-9]{64}/);
});

for (const [name, cfg, diagnostic] of [
  ["runtime absent", { noRuntime: true }, /no WebView2 runtime version/],
  ["download 404", { httpError: 404 }, /HTTP 404/],
  ["corrupt archive", { extractError: true }, /extracting msedgedriver.exe: fixture corrupt zip/],
  ["missing extracted executable", { missingExe: true }, /missing after extraction/],
  ["wrong driver build", { driverVersion: "MSEdgeDriver 142.0.3595.54" }, /expected version 142\.0\.3595\.53/],
]) {
  test(`NFR/#133 QA: Windows fetcher rejects ${name}`, t => {
    const { result, events } = run(t, { kind: "fetcher", ...cfg });
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, diagnostic);
    if (cfg.noRuntime) assert.ok(events.every(e => e.kind !== "fetch"));
  });
}
