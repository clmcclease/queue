// #41: scripts/test-linux.sh refuses settings that a remote shell could split or interpret,
// before it touches the network (ssh and rsync both hand the path to a shell on the box).
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("../../scripts/test-linux.sh", import.meta.url));
const run = (env) => spawnSync("bash", [script], {
  encoding: "utf8", timeout: 10000, env: { PATH: process.env.PATH, HOME: process.env.HOME, ...env },
});

test("unsafe hosts, folders and xvfb values are refused before any ssh or rsync", { skip: process.platform === "win32" && "bash script" }, () => {
  for (const [name, value] of [
    ["QUEUE_LINUX_DIR", "queue tests"], ["QUEUE_LINUX_DIR", "../outside"], ["QUEUE_LINUX_DIR", "/etc"],
    ["QUEUE_LINUX_DIR", "-rf"], ["QUEUE_LINUX_DIR", "$(id)"], ["QUEUE_LINUX_DIR", "a;b"],
    // These would point `rsync --delete` at the remote home itself (codex-qa review, P1).
    ["QUEUE_LINUX_DIR", "."], ["QUEUE_LINUX_DIR", "./"], ["QUEUE_LINUX_DIR", "./."], ["QUEUE_LINUX_DIR", "a/."],
    ["QUEUE_LINUX_DIR", ".ssh"], ["QUEUE_LINUX_DIR", "a//b"], ["QUEUE_LINUX_DIR", "a/"],
    ["QUEUE_LINUX_HOST", "-oProxyCommand=x"], ["QUEUE_LINUX_HOST", "host name"], ["QUEUE_LINUX_XVFB", "yes"],
  ]) {
    const result = run({ [name]: value, QUEUE_LINUX_HOST: name === "QUEUE_LINUX_HOST" ? value : "nohost.invalid" });
    assert.equal(result.status, 2, `${name}=${JSON.stringify(value)}: ${result.stderr}`);
    assert.doesNotMatch(result.stdout, /Syncing/, "nothing was attempted");
  }
});

test("the box rebuilds ffmpeg when its build script or manifest changes, or a binary is missing", () => {
  const text = readFileSync(script, "utf8");
  assert.match(text, /cat scripts\/build-ffmpeg\.sh scripts\/sidecars\.json \| sha256sum/);
  assert.match(text, /queue-ffmpeg-\$TARGET/);
  assert.match(text, /queue-ffprobe-\$TARGET/);
});

test("a valid folder passes the local checks, and the box-side guard runs before any rsync --delete", { skip: process.platform === "win32" && "bash script" }, () => {
  const text = readFileSync(script, "utf8");
  const guard = text.indexOf("<<'GUARD'"), sync = text.indexOf("rsync -az --delete");
  assert.ok(guard > 0 && sync > guard, "the remote guard must come first");
  assert.match(text, /case "\$real" in "\$home"\/\?\*\) ;;/, "strictly inside the home, symlinks resolved");
  assert.match(text, /\.queue-test-checkout/, "only a folder this script made may be overwritten");
  for (const dest of ["queue-test", "work/queue-test", "q1.2_x-y"]) {
    const result = run({ QUEUE_LINUX_DIR: dest, QUEUE_LINUX_HOST: "nohost.invalid" });
    assert.notEqual(result.status, 2, `${dest} must pass the local checks: ${result.stderr}`);
  }
});

// #65: run the box-side guard itself (the script between <<'GUARD' and GUARD) against a scratch
// home, rather than only matching its text.
test("the box-side guard refuses folders outside the home and folders it didn't make", { skip: process.platform === "win32" && "bash script" }, (t) => {
  const text = readFileSync(script, "utf8");
  const guard = text.slice(text.indexOf("<<'GUARD'\n") + "<<'GUARD'\n".length, text.indexOf("\nGUARD\n"));
  assert.match(guard, /^set -euo pipefail/);
  const scratch = realpathSync(mkdtempSync(join(tmpdir(), "queue-guard-")));
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const home = join(scratch, "home 日本語"), outside = join(scratch, "outside");
  mkdirSync(home);
  mkdirSync(outside);
  writeFileSync(join(outside, "keep"), "outside data");
  const guardRun = (dest) => spawnSync("bash", ["-s", "--", dest], {
    input: guard, encoding: "utf8", timeout: 10000, env: { PATH: process.env.PATH, HOME: home },
  });
  const MARK = ".queue-test-checkout";

  let result = guardRun("queue-test");
  assert.equal(result.status, 0, result.stderr);
  assert.ok(existsSync(join(home, "queue-test", MARK)), "a new folder gets the marker");
  assert.equal(guardRun("queue-test").status, 0, "its own folder passes again");

  mkdirSync(join(home, "mine"));
  writeFileSync(join(home, "mine", "notes.txt"), "keep me");
  result = guardRun("mine");
  assert.equal(result.status, 3, "a folder with files and no marker is refused");
  assert.match(result.stderr, /wasn't made by scripts\/test-linux\.sh/);
  assert.ok(!existsSync(join(home, "mine", MARK)), "and isn't marked");

  symlinkSync(outside, join(home, "elsewhere"));
  symlinkSync(".", join(home, "self"));
  for (const dest of ["elsewhere", "self", "self/self"]) {
    result = guardRun(dest);
    assert.equal(result.status, 3, `${dest}: ${result.stderr}`);
    assert.match(result.stderr, /not a folder inside/);
  }
  assert.ok(!existsSync(join(outside, MARK)) && !existsSync(join(home, MARK)), "nothing outside was marked");
  assert.equal(readFileSync(join(outside, "keep"), "utf8"), "outside data");
});
