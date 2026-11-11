#!/usr/bin/env node
// #133 (D-037): run #43's CSP probe (qa/manual/csp-probe-43.js) in the real Queue app under
// WebDriver, so the release CSP is checked in the real webview: WebKitGTK on Linux, WebView2 on
// Windows. macOS has no WebDriver for WKWebView, so there the probe stays a manual check.
//
//   node qa/webdriver/csp-probe-133.mjs --app <built app> [--native-driver <path>] [--tauri-driver <path>]
//
// Build the app with the release CSP first: `npm run tauri build -- --debug --no-bundle`, then
// pass src-tauri/target/debug/queue (queue.exe on Windows). tauri-driver 2.1.0 (pinned, D-037)
// must be on PATH or given with --tauri-driver. On Linux it finds WebKitWebDriver on PATH (run
// under xvfb-run); on Windows pass msedgedriver with --native-driver (scripts/fetch-msedgedriver.mjs).
// WebDriver is plain HTTP, so this uses Node's fetch rather than a client library (D-037).
import { spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

// The probe's checks, by the names it prints. All must be present and pass.
export const CHECKS = [
  "inline <script>",
  "eval()",
  "new Function()",
  "remote <script>",
  "fetch https://example.com",
  "inline <style>",
  "https thumbnail (i.ytimg.com)",
  "http thumbnail",
  "IPC invoke(queue_list)",
];

export const PORT = 4444; // tauri-driver's default; it runs the native driver on 4445.

// The WebDriver "execute async" body: run the probe unchanged, capture the rows it hands to
// console.table and the lines it logs, and return them with its PASS/FAIL verdict.
export function probeScript(probeSource) {
  const expr = probeSource.trim().replace(/;$/, "");
  return `const done = arguments[arguments.length - 1];
const captured = { rows: null, logs: [] };
const { table, log } = console;
console.table = (rows) => { captured.rows = rows; table.call(console, rows); };
console.log = (...parts) => { captured.logs.push(parts.join(" ")); log.apply(console, parts); };
const restore = () => { console.table = table; console.log = log; };
Promise.resolve(
${expr}
).then(
  (verdict) => { restore(); done({ verdict, rows: captured.rows, logs: captured.logs }); },
  (e) => { restore(); done({ verdict: "ERROR", error: String((e && e.stack) || e), rows: captured.rows, logs: captured.logs }); },
);`;
}

// Problems with a probe report; empty means a pass. Every named check must appear exactly once
// and pass, so a probe that silently drops a check can't pass here.
export function reportProblems(report) {
  if (!report || typeof report !== "object") return ["no report from the probe"];
  const problems = [];
  if (report.verdict !== "PASS") problems.push(`verdict ${report.verdict}${report.error ? `: ${report.error}` : ""}`);
  const rows = Array.isArray(report.rows) ? report.rows : [];
  if (!Array.isArray(report.rows)) problems.push("the probe printed no results table");
  for (const name of CHECKS) {
    const matching = rows.filter((r) => r && r.check === name);
    if (matching.length !== 1) problems.push(`${name}: expected once, found ${matching.length}`);
    else if (matching[0].ok !== "✔") problems.push(`${name}: expected ${matching[0].expected}, got ${matching[0].actual}`);
  }
  for (const r of rows) if (!CHECKS.includes(r?.check)) problems.push(`unexpected check ${r?.check}`);
  return problems;
}

function parseArgs(argv) {
  const opts = { tauriDriver: "tauri-driver" };
  for (let i = 0; i < argv.length; i++) {
    const key = { "--app": "app", "--native-driver": "nativeDriver", "--tauri-driver": "tauriDriver" }[argv[i]];
    if (!key || i + 1 >= argv.length) throw new Error(`usage: --app <path> [--native-driver <path>] [--tauri-driver <path>] (bad argument ${argv[i]})`);
    opts[key] = argv[++i];
  }
  if (!opts.app) throw new Error("--app <path to the built app> is required");
  return opts;
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// One WebDriver request. Every request has a deadline, so a stalled driver can't hold the run
// (or its cleanup) past it (Codex's test 20261109-095754-test-EjdQ).
export async function wd(method, path, body, ms = 30_000) {
  const res = await fetch(`http://127.0.0.1:${PORT}${path}`, {
    method,
    headers: body ? { "content-type": "application/json" } : {},
    body: body ? JSON.stringify(body) : undefined,
    signal: AbortSignal.timeout(Math.max(1, ms)),
  });
  const text = await res.text();
  let json;
  try { json = JSON.parse(text); } catch { throw new Error(`${method} ${path}: HTTP ${res.status}, not JSON: ${text.slice(0, 500)}`); }
  if (!res.ok || json?.value?.error) {
    throw new Error(`${method} ${path}: HTTP ${res.status} ${json?.value?.error ?? ""}: ${json?.value?.message ?? text.slice(0, 500)}`);
  }
  return json.value;
}

export async function until(what, ms, fn) {
  const end = Date.now() + ms;
  let last;
  while (Date.now() < end) {
    // Each attempt gets only the time left, so one stalled request can't outlast the deadline.
    try { if (await fn(Math.max(1, end - Date.now()))) return; } catch (e) { if (e.fatal) throw e; last = e; }
    await sleep(250);
  }
  throw new Error(`timed out after ${ms} ms waiting for ${what}${last ? ` (last error: ${last.message})` : ""}`);
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  const probe = readFileSync(new URL("../manual/csp-probe-43.js", import.meta.url), "utf8");
  const driverArgs = opts.nativeDriver ? ["--native-driver", resolve(opts.nativeDriver)] : [];
  const driver = spawn(opts.tauriDriver, driverArgs, { stdio: ["ignore", "inherit", "inherit"] });
  let driverExit;
  driver.on("exit", (code, signal) => { driverExit = `tauri-driver exited (${code ?? signal})`; });
  driver.on("error", (e) => { driverExit = `tauri-driver failed to start: ${e.message}`; });
  let session;
  const started = Date.now();
  try {
    await until("tauri-driver", 30_000, async (left) => {
      if (driverExit) throw Object.assign(new Error(driverExit), { fatal: true });
      return (await wd("GET", "/status", undefined, left)) !== undefined;
    });
    // Starting the app can take a while on a cold runner.
    const created = await wd("POST", "/session", {
      capabilities: { alwaysMatch: { "tauri:options": { application: resolve(opts.app) } } },
    }, 120_000);
    session = created.sessionId;
    console.log(`session ${session}: ${JSON.stringify(created.capabilities?.browserName ?? "")} ${created.capabilities?.browserVersion ?? ""}`);
    await wd("POST", `/session/${session}/timeouts`, { script: 90_000 });
    await until("the app's page and IPC bridge", 60_000, (left) =>
      wd("POST", `/session/${session}/execute/sync`, {
        script: "return document.readyState === 'complete' && !!window.__TAURI_INTERNALS__",
        args: [],
      }, left));
    // Past the 90 s script timeout, so the driver's own timeout error is what's reported.
    const report = await wd("POST", `/session/${session}/execute/async`, { script: probeScript(probe), args: [] }, 120_000);
    console.table(report?.rows ?? []);
    for (const line of report?.logs ?? []) console.log(line);
    const problems = reportProblems(report);
    console.log(`(${((Date.now() - started) / 1000).toFixed(1)} s)`);
    if (problems.length) {
      console.error(`CSP PROBE (WebDriver): FAIL\n  ${problems.join("\n  ")}`);
      process.exitCode = 1;
    } else {
      console.log(`CSP PROBE (WebDriver): PASS, ${CHECKS.length}/${CHECKS.length} checks`);
    }
  } finally {
    try {
      if (session) await wd("DELETE", `/session/${session}`, undefined, 10_000).catch((e) => console.error(`ending the session: ${e.message}`));
    } finally {
      driver.kill();
    }
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((e) => {
    console.error(`CSP PROBE (WebDriver): ERROR\n  ${e.stack ?? e}`);
    process.exit(1);
  });
}
