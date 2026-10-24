// #122: scripts/codex-qa closes Codex's stdin. `codex exec` reads a piped stdin to EOF, and a
// background job keeps the caller's stdin under `set -m`, so a caller holding a pipe open used to
// hang the gate until the watchdog fired (two #86(a) runs, D-035).
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("../../scripts/codex-qa", import.meta.url));
const skip = process.platform === "win32" && "bash script";

// A scratch repo with a copy of the script and a fake codex that writes its -o report, then runs
// `$FAKE_CODEX_THEN` (as in the #69 watchdog test).
function scratchRepo(t) {
  const dir = realpathSync(mkdtempSync(join(tmpdir(), "queue-codex-qa-122-")));
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

test("#122: Codex sees EOF on stdin while the caller holds the script's stdin open", { skip }, async (t) => {
  const dir = scratchRepo(t);
  const stdinFile = join(dir, "stdin-result");
  const started = Date.now();
  const child = spawn("bash", [join(dir, "scripts", "codex-qa"), "audit"], {
    env: {
      PATH: `${join(dir, "bin")}:${process.env.PATH}`,
      HOME: process.env.HOME,
      CODEX_QA_TIMEOUT: "20",
      FAKE_CODEX_STDIN_FILE: stdinFile,
      // `read` returns 1 at EOF; a timeout would be above 128.
      FAKE_CODEX_THEN: 'read -t 5 -r _; echo "read=$?" >"$FAKE_CODEX_STDIN_FILE"',
    },
    // A pipe that is never written to or closed: the held-open stdin of the incident.
    stdio: ["pipe", "pipe", "pipe"],
  });
  let stderr = "";
  child.stdout.resume();
  child.stderr.on("data", (d) => { stderr += d; });
  const status = await new Promise((resolve) => child.on("close", resolve));
  const ms = Date.now() - started;
  child.stdin.destroy();
  assert.equal(status, 0, stderr);
  assert.equal(readFileSync(stdinFile, "utf8").trim(), "read=1", "Codex got EOF, not the caller's pipe");
  assert.ok(ms < 4000, `took ${ms} ms: Codex waited on the caller's stdin`);
});
