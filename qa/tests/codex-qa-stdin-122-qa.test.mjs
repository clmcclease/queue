// Independent #122 / NFR / D-035 tests. All invocations use a fake Codex in a scratch
// repository; no real gate, account, network request or new commit is involved.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const root = fileURLToPath(new URL("../../", import.meta.url));
const source = readFileSync(new URL("../../scripts/codex-qa", import.meta.url), "utf8");
const skip = process.platform === "win32" && "Unix bash wrapper";

function fixture(t, withoutRedirect = false) {
  const dir = realpathSync(mkdtempSync(join(tmpdir(), "queue QA 日本語 122-")));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  // Reuse existing history read-only, so test mode can exercise real worktree and patch
  // creation without creating commits or modifying this checkout's git metadata.
  const clone = spawnSync("git", ["clone", "--quiet", "--shared", "--no-checkout", "--", root, dir], { encoding: "utf8" });
  assert.equal(clone.status, 0, clone.stderr);
  mkdirSync(join(dir, "scripts"));
  mkdirSync(join(dir, "qa", "prompts"), { recursive: true });
  mkdirSync(join(dir, "bin"));
  writeFileSync(join(dir, ".gitignore"), "qa/reports/\n");
  const script = withoutRedirect ? source.replace('codex "$@" </dev/null', 'codex "$@"') : source;
  if (withoutRedirect) assert.notEqual(script, source, "negative control removes only the stdin redirect");
  writeFileSync(join(dir, "scripts", "codex-qa"), script);
  for (const mode of ["audit", "review", "test"]) {
    writeFileSync(join(dir, "qa", "prompts", `${mode}.md`), `${mode} fixture prompt\n`);
  }
  writeFileSync(join(dir, "bin", "codex"), `#!/usr/bin/env bash
printf '%s\\n' "$@" > "$FIXTURE_DIR/argv"
while [[ $# -gt 0 ]]; do
  if [[ $1 == -o ]]; then report=$2; shift; fi
  shift
done
if [[ /dev/stdin -ef /dev/null ]]; then null=yes; else null=no; fi
read -r -t 1 line
read_status=$?
printf 'null=%s\\nread=%s\\nline=%s\\n' "$null" "$read_status" "$line" > "$FIXTURE_DIR/stdin"
echo 'fake diagnostic' >&2
if [[ $FAKE_BEHAVIOR == hang ]]; then sleep 30; fi
if [[ $FAKE_BEHAVIOR != empty ]]; then echo 'fake QA report' > "$report"; fi
if [[ $FAKE_BEHAVIOR == fail ]]; then exit 17; fi
exit 0
`);
  chmodSync(join(dir, "bin", "codex"), 0o755);
  return dir;
}

async function run(t, dir, args, { buffered = false, behavior = "pass" } = {}) {
  const started = Date.now();
  const child = spawn("bash", [join(dir, "scripts", "codex-qa"), ...args], {
    env: {
      PATH: `${join(dir, "bin")}:${process.env.PATH}`,
      HOME: process.env.HOME,
      FIXTURE_DIR: dir,
      FAKE_BEHAVIOR: behavior,
      CODEX_QA_TIMEOUT: behavior === "hang" ? "1" : "10",
    },
    stdio: ["pipe", "pipe", "pipe"],
  });
  // Always keep the pipe open until close. Buffered content must not be consumed either.
  child.stdin.on("error", (error) => { if (error.code !== "EPIPE") throw error; });
  if (buffered) child.stdin.write("untrusted caller input: café 日本語\n");
  let stdout = "", stderr = "";
  child.stdout.on("data", (data) => { stdout += data; });
  child.stderr.on("data", (data) => { stderr += data; });
  t.after(() => {
    child.stdin.destroy();
    if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
  });
  const code = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("wrapper failed to close output pipes within 15s")), 15000);
    child.once("error", (error) => { clearTimeout(timer); reject(error); });
    child.once("close", (status) => { clearTimeout(timer); resolve(status); });
  });
  child.stdin.destroy();
  const logName = readdirSync(join(dir, "qa", "reports")).find((name) => name.endsWith(".log"));
  return { code, stdout, stderr, ms: Date.now() - started,
    stdin: readFileSync(join(dir, "stdin"), "utf8"),
    argv: readFileSync(join(dir, "argv"), "utf8"),
    log: readFileSync(join(dir, "qa", "reports", logName), "utf8"),
  };
}

for (const args of [["audit"], ["review"], ["review", "--base", "main"], ["review", "--commit", "HEAD"], ["test", "#122 café 日本語"]]) {
  for (const buffered of [false, true]) {
    test(`#122 QA: ${args.join(" ")} gets EOF with ${buffered ? "buffered" : "empty"} caller stdin held open`, { skip }, async (t) => {
      const dir = fixture(t);
      const result = await run(t, dir, args, { buffered });
      assert.equal(result.code, 0, result.stderr);
      assert.equal(result.stdin, "null=yes\nread=1\nline=\n");
      assert.match(result.stdout, /codex-qa exit: 0\n$/);
      assert.match(result.argv, /^exec\n/);
      assert.match(result.argv, new RegExp(`${args[0]} fixture prompt`));
      assert.doesNotMatch(result.log, /timed out/);
      if (args[0] === "test") {
        assert.match(result.argv, /#122 café 日本語/);
        assert.match(result.stdout, /codex-qa tests patch: .*\(0 files;/);
        const wt = result.stdout.match(/codex-qa worktree: (.*?)   \(remove with:/)?.[1];
        assert.ok(wt, result.stdout);
        t.after(() => rmSync(wt, { recursive: true, force: true }));
      }
    });
  }
}

for (const [behavior, expected] of [["fail", 17], ["empty", 3], ["hang", 143]]) {
  test(`#122 QA: held-open stdin preserves ${behavior} gate failure`, { skip }, async (t) => {
    const dir = fixture(t);
    const result = await run(t, dir, ["review"], { behavior });
    assert.equal(result.stdin, "null=yes\nread=1\nline=\n");
    assert.equal(result.code, expected, result.stderr);
    assert.match(result.stdout, new RegExp(`codex-qa exit: ${expected}\\n$`));
    if (behavior === "empty") assert.match(result.stderr, /wrote no report/);
    else assert.match(result.stderr, /fake diagnostic/);
    if (behavior === "hang") assert.match(result.log, /timed out after 1s/);
  });
}

test("#122 QA: negative control without the redirect waits for input instead of seeing EOF", { skip }, async (t) => {
  const dir = fixture(t, true);
  const result = await run(t, dir, ["audit"]);
  assert.equal(result.code, 0, result.stderr);
  // macOS's bash 3.2 returns 1 for BOTH a read timeout and EOF. Check the actual
  // descriptor and elapsed time so the test cannot mistake that timeout for EOF.
  assert.match(result.stdin, /^null=no\nread=\d+\nline=\n$/);
  assert.ok(result.ms >= 900, `expected the 1s read timeout, got ${result.ms}ms`);
});
