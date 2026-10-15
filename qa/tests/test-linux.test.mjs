// #41: scripts/test-linux.sh refuses settings that a remote shell could split or interpret,
// before it touches the network (ssh and rsync both hand the path to a shell on the box).
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
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
