// #69: scripts/codex-qa's watchdog must not hold the caller's output pipe. A finished run reaches
// EOF at once, and a hung Codex is still killed and reported at the timeout.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("../../scripts/codex-qa", import.meta.url));
const skip = process.platform === "win32" && "bash script";

// A scratch repo holding a copy of the script, so reports never land in this checkout's qa/reports.
// The fake codex writes its -o report, then runs `$FAKE_CODEX_THEN` (a hang, or nothing).
function scratchRepo(t) {
  const dir = realpathSync(mkdtempSync(join(tmpdir(), "queue-codex-qa-")));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  mkdirSync(join(dir, "scripts"));
  mkdirSync(join(dir, "qa", "prompts"), { recursive: true });
  mkdirSync(join(dir, "bin"));
  copyFileSync(script, join(dir, "scripts", "codex-qa"));
  for (const mode of ["audit", "review", "test"]) writeFileSync(join(dir, "qa", "prompts", `${mode}.md`), `${mode} prompt\n`);
  writeFileSync(join(dir, "bin", "codex"), [
    "#!/usr/bin/env bash",
    'while [[ $# -gt 0 ]]; do [[ $1 == -o ]] && { echo "fake report" >"$2"; shift; }; shift; done',
    'eval "${FAKE_CODEX_THEN:-}"',
    "",
  ].join("\n"));
  chmodSync(join(dir, "bin", "codex"), 0o755);
  assert.equal(spawnSync("git", ["init", "-q", dir]).status, 0);
  return dir;
}

// Resolves when the script exits *and* its stdout/stderr pipes close, which is what a caller
// reading to EOF waits for.
function runToEof(dir, env) {
  const started = Date.now();
  const child = spawn("bash", [join(dir, "scripts", "codex-qa"), "audit"], {
    env: { PATH: `${join(dir, "bin")}:${process.env.PATH}`, HOME: process.env.HOME, ...env },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let stdout = "", stderr = "";
  child.stdout.on("data", (d) => { stdout += d; });
  child.stderr.on("data", (d) => { stderr += d; });
  return new Promise((resolve) => child.on("close", (status) => resolve({ status, stdout, stderr, ms: Date.now() - started })));
}

test("a finished run closes the caller's pipes at once, not after the timeout", { skip }, async (t) => {
  const dir = scratchRepo(t);
  const result = await runToEof(dir, { CODEX_QA_TIMEOUT: "20" });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /codex-qa report: .*-audit-.*\.md/);
  assert.ok(result.ms < 10000, `EOF took ${result.ms} ms; the watchdog's sleep held the pipe`);
});

test("a hung Codex is killed at the timeout, reported, and the pipes close", { skip }, async (t) => {
  const dir = scratchRepo(t);
  // The hang is a child of the fake codex, in its process group, like a Codex tool subprocess.
  const result = await runToEof(dir, { CODEX_QA_TIMEOUT: "1", FAKE_CODEX_THEN: "sleep 30" });
  assert.notEqual(result.status, 0, "a timed-out run fails");
  assert.ok(result.ms < 10000, `took ${result.ms} ms`);
  assert.match(result.stderr, /codex-qa FAILED/);
  const logs = readdirSync(join(dir, "qa", "reports")).filter((f) => f.endsWith(".log"));
  assert.equal(logs.length, 1);
  assert.match(readFileSync(join(dir, "qa", "reports", logs[0]), "utf8"), /timed out after 1s/);
});
