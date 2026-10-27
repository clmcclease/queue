// #43: the manual CSP probe (qa/manual/csp-probe-43.js) must only pass on real CSP blocks. Runs
// it in a vm with a fake page, so a network failure without a CSP violation can be told apart
// from a block (Codex's review 20261027-110504-review-mlxV).
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import vm from "node:vm";

const probe = readFileSync(new URL("../manual/csp-probe-43.js", import.meta.url), "utf8");

// `page` decides what each resource does: "violation" (CSP block: a violation event, then an
// error), "error" (a network failure with no violation) or "load".
async function runProbe(page) {
  const listeners = [];
  const violate = (directive, uri) => listeners.forEach((fn) => fn({ effectiveDirective: directive, blockedURI: uri }));
  const settle = (el, outcome, directive, uri) => setTimeout(() => {
    if (outcome === "violation") violate(directive, uri);
    (outcome === "load" ? el.onload : el.onerror)?.();
  }, 0);
  const append = (el) => {
    if (el.tag === "script" && el.src) settle(el, page.remoteScript, "script-src", el.src);
  };
  const document = {
    addEventListener: (_, fn) => listeners.push(fn),
    removeEventListener: () => {},
    createElement: (tag) => ({ tag, remove() {} }),
    head: { appendChild: append },
    body: { appendChild: () => {} },
  };
  class Image {
    set src(url) {
      const outcome = url.startsWith("https:") ? page.httpsImage : page.httpImage;
      settle(this, outcome, "img-src", url);
    }
  }
  const fetch = async (url) => {
    if (page.fetch === "load") return {};
    if (page.fetch === "violation") violate("connect-src", url);
    throw new TypeError("Load failed");
  };
  const logs = [];
  let table;
  const context = vm.createContext({
    document, Image, fetch, setTimeout,
    getComputedStyle: () => ({ width: "0px" }),
    console: { table: (rows) => { table = rows; }, log: (line) => logs.push(line) },
    __TAURI_INTERNALS__: { invoke: async () => [] },
  }, { codeGeneration: { strings: false } });
  context.window = context;
  const verdict = await vm.runInContext(probe, context);
  return { verdict, rows: Object.fromEntries(table.map((r) => [r.check, r.actual])), logs };
}

const blocked = { remoteScript: "violation", fetch: "violation", httpImage: "violation", httpsImage: "load" };

test("#43 probe: every block with its CSP violation passes", async () => {
  const { verdict, rows } = await runProbe(blocked);
  assert.equal(verdict, "PASS", JSON.stringify(rows));
  assert.equal(rows["inline <script>"], "blocked");
  assert.equal(rows["eval()"], "blocked");
  assert.equal(rows["IPC invoke(queue_list)"], "works");
});

test("#43 probe: network failures without a CSP violation are inconclusive, not a pass", async () => {
  const { verdict, rows } = await runProbe({ remoteScript: "error", fetch: "error", httpImage: "error", httpsImage: "load" });
  assert.equal(verdict, "FAIL");
  for (const check of ["remote <script>", "fetch https://example.com", "http thumbnail"]) {
    assert.match(rows[check], /^inconclusive/, check);
  }
});

for (const [name, change, check, actual] of [
  ["a remote script that loads", { remoteScript: "load" }, "remote <script>", "loaded"],
  ["a cross-origin fetch that succeeds", { fetch: "load" }, "fetch https://example.com", "allowed"],
  ["an http thumbnail that loads", { httpImage: "load" }, "http thumbnail", "loaded"],
  ["an https thumbnail that's blocked", { httpsImage: "violation" }, "https thumbnail (i.ytimg.com)", "failed"],
]) {
  test(`#43 probe: ${name} fails the probe`, async () => {
    const { verdict, rows } = await runProbe({ ...blocked, ...change });
    assert.equal(verdict, "FAIL");
    assert.equal(rows[check], actual);
  });
}
