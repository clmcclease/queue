// Independent #69 / NFR regressions: EOF alone does not prove the watchdog sleep died.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import test from "node:test";

const skip = process.platform === "win32" && "Unix bash watchdog";
const source = new URL("../../scripts/codex-qa", import.meta.url);
const pids = (file) => existsSync(file) ? readFileSync(file, "utf8").trim().split(/\s+/).filter(Boolean).map(Number) : [];
function alive(pid) {
  try { process.kill(pid, 0); return true; }
  catch (error) { if (error.code === "ESRCH") return false; throw error; }
}

function fixture(t) {
  // Spaces and Unicode also exercise the wrapper's quoting of report/repository paths.
  const dir = realpathSync(mkdtempSync(join(tmpdir(), "queue QA café 69-")));
  const sleepPids = join(dir, "sleep.pids"), codexPids = join(dir, "codex.pids");
  const descendantPids = join(dir, "descendant.pids");
  t.after(() => {
    // Clean up even when a regression leaves a long-lived sleeper or fake Codex behind.
    for (const file of [sleepPids, descendantPids, codexPids]) {
      for (const pid of pids(file)) {
        try { process.kill(pid, "SIGKILL"); } catch (error) { if (error.code !== "ESRCH") throw error; }
      }
    }
    rmSync(dir, { recursive: true, force: true });
  });
  mkdirSync(join(dir, "scripts"));
  mkdirSync(join(dir, "bin"));
  mkdirSync(join(dir, "qa", "prompts"), { recursive: true });
  copyFileSync(source, join(dir, "scripts", "codex-qa"));
  for (const mode of ["audit", "review"]) writeFileSync(join(dir, "qa", "prompts", `${mode}.md`), "QA fixture\n");
  const executable = (name, body) => {
    const path = join(dir, "bin", name);
    writeFileSync(path, `#!/usr/bin/env bash\n${body}\n`);
    chmodSync(path, 0o755);
  };
  executable("sleep", 'echo "$$" >> "$SLEEP_PIDS"\nexec /bin/sleep "$@"');
  executable("codex", `echo "$$" >> "$CODEX_PIDS"
while [[ $# -gt 0 ]]; do
  if [[ $1 == -o ]]; then report=$2; shift; fi
  shift
done
# Wait until the watchdog actually starts; otherwise fast success could hide missing cleanup.
for ((i=0; i<300; i++)); do
  [[ -s $SLEEP_PIDS ]] && break
  /bin/sleep 0.01
done
[[ -s $SLEEP_PIDS ]] || exit 99
[[ $FAKE_REPORT == yes ]] && echo "fixture report" > "$report"
if [[ $FAKE_HANG == yes ]]; then
  /bin/sleep 60 &
  echo "$!" > "$DESCENDANT_PIDS"
  wait
fi
echo "fixture diagnostic" >&2
exit "$FAKE_STATUS"`);
  const init = spawnSync("git", ["init", "-q", dir], { encoding: "utf8" });
  assert.equal(init.status, 0, init.stderr);
  return { dir, sleepPids, codexPids, descendantPids };
}

async function run(t, f, env = {}, mode = "audit") {
  const started = Date.now();
  const child = spawn("bash", [join(f.dir, "scripts", "codex-qa"), mode], {
    env: { ...process.env, PATH: `${join(f.dir, "bin")}:${process.env.PATH}`,
      FAKE_REPORT: "yes", FAKE_HANG: "no", FAKE_STATUS: "0",
      CODEX_QA_TIMEOUT: "30", SLEEP_PIDS: f.sleepPids, CODEX_PIDS: f.codexPids,
      DESCENDANT_PIDS: f.descendantPids, ...env },
    stdio: ["ignore", "pipe", "pipe"],
  });
  t.after(() => { if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL"); });
  let stdout = "", stderr = "";
  child.stdout.on("data", (d) => { stdout += d; });
  child.stderr.on("data", (d) => { stderr += d; });
  const status = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("wrapper did not reach EOF within 8s")), 8000);
    child.once("error", (error) => { clearTimeout(timer); reject(error); });
    child.once("close", (code) => { clearTimeout(timer); resolve(code); });
  });
  const logs = readdirSync(join(f.dir, "qa", "reports")).filter((name) => name.endsWith(".log"));
  assert.equal(logs.length, 1);
  return { status, stdout, stderr, ms: Date.now() - started,
    log: readFileSync(join(f.dir, "qa", "reports", logs[0]), "utf8") };
}

async function assertGone(file) {
  const recorded = pids(file);
  assert.equal(recorded.length, 1, `expected a process recorded in ${file}`);
  const deadline = Date.now() + 1500;
  while (alive(recorded[0]) && Date.now() < deadline) await delay(20);
  assert.equal(alive(recorded[0]), false, `process ${recorded[0]} survived wrapper completion`);
}

test("#69: successful review terminates/reaps its watchdog sleep, including Unicode paths", { skip }, async (t) => {
  const f = fixture(t);
  const result = await run(t, f, {}, "review");
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /codex-qa report: .*review-.*\.md/);
  assert.doesNotMatch(result.log, /timed out/);
  await assertGone(f.sleepPids);
  await assertGone(f.codexPids);
});

test("#69: early Codex failure preserves diagnostics/status and kills its watchdog sleep", { skip }, async (t) => {
  const f = fixture(t);
  const result = await run(t, f, { FAKE_STATUS: "17" });
  assert.equal(result.status, 17);
  assert.match(result.stderr, /FAILED \(codex exit 17\)/);
  assert.match(result.stderr, /fixture diagnostic/);
  assert.doesNotMatch(result.log, /timed out/);
  await assertGone(f.sleepPids);
});

test("#69: success without a report still fails promptly and cleans up its watchdog", { skip }, async (t) => {
  const f = fixture(t);
  const result = await run(t, f, { FAKE_REPORT: "no" });
  assert.equal(result.status, 3);
  assert.match(result.stderr, /wrote no report/);
  await assertGone(f.sleepPids);
});

test("#69: timeout waits for its deadline, logs once, and removes the whole ordinary process group", { skip }, async (t) => {
  const f = fixture(t);
  const result = await run(t, f, { CODEX_QA_TIMEOUT: "1", FAKE_HANG: "yes" });
  assert.notEqual(result.status, 0);
  assert.ok(result.ms >= 900, `timeout fired too early: ${result.ms}ms`);
  assert.equal((result.log.match(/timed out after 1s/g) ?? []).length, 1);
  assert.match(result.stderr, /FAILED/);
  await assertGone(f.sleepPids);
  await assertGone(f.codexPids);
  await assertGone(f.descendantPids);
});
