// #133 (D-037): the WebDriver CSP runner and the msedgedriver fetcher. The real runs need the
// built app and a native WebDriver, so CI's `webdriver` job does those; here the pure parts are
// checked: the probe wrapper runs the unchanged probe and hands back its rows, the report check
// can't pass a dropped or failed check, and only a plain WebView2 version reaches the URL.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import vm from "node:vm";
import { CHECKS, probeScript, reportProblems } from "../webdriver/csp-probe-133.mjs";
import { driverUrl, isVersion, parseRegVersion } from "../../scripts/fetch-msedgedriver.mjs";

const root = new URL("../../", import.meta.url);
const probe = readFileSync(new URL("qa/manual/csp-probe-43.js", root), "utf8");

// A fake page where every CSP block fires its violation (as in csp-probe-43.test.mjs), or not.
// `allow` lets one restriction through, as a page without that CSP rule would: "inline-script",
// "eval" or "inline-style".
function fakePage({ blocks = true, allow } = {}) {
  const listeners = [];
  const violate = (directive, uri) => listeners.forEach((fn) => fn({ effectiveDirective: directive, blockedURI: uri }));
  const settle = (el, ok, directive, uri) => setTimeout(() => {
    if (!ok && blocks) violate(directive, uri);
    (ok ? el.onload : el.onerror)?.();
  }, 0);
  const document = {
    addEventListener: (_, fn) => listeners.push(fn),
    removeEventListener: () => {},
    createElement: (tag) => ({ tag, remove() {} }),
    head: {
      appendChild: (el) => {
        if (el.tag === "script" && el.src) settle(el, false, "script-src", el.src);
        else if (el.tag === "script" && allow === "inline-script") vm.runInContext(el.textContent, context);
      },
    },
    body: { appendChild: () => {} },
  };
  class Image { set src(url) { settle(this, url.startsWith("https:"), "img-src", url); } }
  const fetch = async (url) => { if (blocks) violate("connect-src", url); throw new TypeError("Load failed"); };
  const printed = [];
  const console = { table: (rows) => printed.push(rows), log: () => {} };
  const context = vm.createContext({
    document, Image, fetch, setTimeout, console,
    getComputedStyle: () => ({ width: allow === "inline-style" ? "123px" : "0px" }),
    __TAURI_INTERNALS__: { invoke: async () => [] },
  }, { codeGeneration: { strings: allow === "eval" } });
  context.window = context;
  return { context, console, printed };
}

// Runs the wrapper the way WebDriver's "execute async" does: as a function body, with the
// callback as its last argument.
function runWrapped(page) {
  return new Promise((resolve) => {
    page.context.__done = resolve;
    vm.runInContext(`(function () {\n${probeScript(probe)}\n})(__done);`, page.context);
  });
}

test("#133 wrapper: the unchanged probe passes 9/9 and its rows come back", async () => {
  const page = fakePage();
  const { table, log } = page.console;
  const report = await runWrapped(page);
  assert.equal(report.verdict, "PASS", JSON.stringify(report.rows));
  assert.deepEqual(Array.from(report.rows, (r) => r.check), CHECKS); // a copy in this realm
  assert.ok(report.logs.some((l) => l === "CSP PROBE: PASS"));
  assert.deepEqual(reportProblems(report), []);
  // The page's console is restored and still got the table.
  assert.equal(page.console.table, table);
  assert.equal(page.console.log, log);
  assert.equal(page.printed.length, 1);
});

test("#133 wrapper: blocks without CSP violations fail the run", async () => {
  const report = await runWrapped(fakePage({ blocks: false }));
  assert.equal(report.verdict, "FAIL");
  const problems = reportProblems(report);
  for (const check of ["remote <script>", "fetch https://example.com", "http thumbnail"]) {
    assert.ok(problems.some((p) => p.startsWith(`${check}:`)), `${check} in ${problems}`);
  }
});

test("#133 wrapper: a probe that throws reports ERROR", async () => {
  const page = fakePage();
  page.context.__TAURI_INTERNALS__ = undefined;
  page.context.getComputedStyle = () => { throw new Error("boom"); };
  const report = await runWrapped(page);
  assert.equal(report.verdict, "ERROR");
  assert.match(report.error, /boom/);
  assert.ok(reportProblems(report).length > 0);
});

// Negative controls (Codex's review 20261109-100600-review-7vgv): each relaxed rule fails its check.
for (const [allow, checks] of [
  ["inline-script", ["inline <script>"]],
  ["eval", ["eval()", "new Function()"]],
  ["inline-style", ["inline <style>"]],
]) {
  test(`#133 wrapper: a page that allows ${allow} fails ${checks.join(" and ")}`, async () => {
    const report = await runWrapped(fakePage({ allow }));
    assert.equal(report.verdict, "FAIL");
    const problems = reportProblems(report);
    for (const check of checks) assert.ok(problems.some((p) => p.startsWith(`${check}:`)), `${check} in ${problems}`);
    assert.equal(problems.length, checks.length + 1, problems.join("\n")); // + the FAIL verdict
  });
}

const passing = () => ({ verdict: "PASS", rows: CHECKS.map((check) => ({ check, expected: "x", actual: "x", ok: "✔" })) });

test("#133 report: only a full PASS with every check once passes", () => {
  assert.deepEqual(reportProblems(passing()), []);
  for (const [name, mutate] of [
    ["no report", () => null],
    ["FAIL verdict", (r) => ({ ...r, verdict: "FAIL" })],
    ["PASS verdict but a failed row", (r) => { r.rows[8] = { ...r.rows[8], actual: "failed: no", ok: "✖" }; return r; }],
    ["a dropped check", (r) => ({ ...r, rows: r.rows.slice(1) })],
    ["a duplicated check", (r) => ({ ...r, rows: [...r.rows, r.rows[0]] })],
    ["an unknown check", (r) => ({ ...r, rows: [...r.rows, { check: "extra", ok: "✔" }] })],
    ["no table", (r) => ({ verdict: "PASS" })],
  ]) {
    assert.ok(reportProblems(mutate(passing())).length > 0, name);
  }
});

test("#133 msedgedriver: the registry version is parsed and only a plain version reaches the URL", () => {
  const out = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}\r\n    pv    REG_SZ    142.0.3595.53\r\n\r\n";
  assert.equal(parseRegVersion(out), "142.0.3595.53");
  assert.equal(driverUrl("142.0.3595.53"), "https://msedgedriver.microsoft.com/142.0.3595.53/edgedriver_win64.zip");
  for (const bad of ["", "pv REG_SZ", "    pv    REG_SZ    0.0.0.0", "    pv    REG_SZ    1.2.3", "    pv    REG_SZ    1.2.3.4/../x", "    pv    REG_DWORD    0x1"]) {
    assert.equal(parseRegVersion(bad), null, bad);
  }
  for (const bad of ["1.2.3", "1.2.3.4.5", "1.2.3.x", "../1.2.3.4", "1.2.3.4?a", "", undefined, "0.0.0.0"]) {
    assert.equal(isVersion(bad), false, String(bad));
    assert.throws(() => driverUrl(bad), /not a WebView2 version/);
  }
});

test("#133 CI: the webdriver job runs the probe on Linux and Windows with the pinned tauri-driver", () => {
  const workflow = readFileSync(new URL(".github/workflows/build.yml", root), "utf8").replaceAll("\r\n", "\n");
  const start = workflow.indexOf("\n  webdriver:\n");
  assert.ok(start >= 0, "missing webdriver job");
  const job = workflow.slice(start, workflow.indexOf("\n  bundle:\n"));
  assert.match(job, /cargo install tauri-driver --version 2\.1\.0 --locked/);
  assert.match(job, /runner: ubuntu-24\.04/);
  assert.match(job, /runner: windows-2025/);
  assert.doesNotMatch(job, /macos/);
  assert.match(job, /webkit2gtk-driver/);
  assert.match(job, /npm run tauri build -- --debug --no-bundle/);
  assert.match(job, /xvfb-run -a node qa\/webdriver\/csp-probe-133\.mjs --app src-tauri\/target\/debug\/queue\n/);
  assert.match(job, /node scripts\/fetch-msedgedriver\.mjs "\$RUNNER_TEMP\/msedgedriver"/);
  assert.match(job, /csp-probe-133\.mjs --app src-tauri\/target\/debug\/queue\.exe --native-driver "\$RUNNER_TEMP\/msedgedriver\/msedgedriver\.exe"/);
  assert.match(job, /needs: \[ffmpeg, verify-signatures\]/);
  assert.match(job, /MSEDGEDRIVER_TELEMETRY_OPTOUT: "1"/);
  // The Windows diagnostics run only after the probe itself fails, and keep the job failed.
  const diag = job.slice(job.indexOf("- name: Diagnose a failed WebView2 session"));
  assert.match(job, /id: probe-windows/);
  assert.match(diag, /if: failure\(\) && steps\.probe-windows\.outcome == 'failure'\n/);
  assert.match(diag, /\n {10}exit 1\n?$/);
  assert.match(diag, /MSEDGEDRIVER_TELEMETRY_OPTOUT: "1"/);
});
