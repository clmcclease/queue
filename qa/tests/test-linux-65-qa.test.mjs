// #65 / PRD §7 NFR: exercise the guard through the real script with local transport stubs.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("../../scripts/test-linux.sh", import.meta.url));
const options = { skip: process.platform === "win32" && "bash script" };
const mark = ".queue-test-checkout";

function fixture(t) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "queue-65-guard-")));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const home = join(root, "home 日本語");
  const bin = join(root, "bin");
  const trace = join(root, "trace");
  mkdirSync(home);
  mkdirSync(bin);
  // Execute SSH's real stdin locally. No host is contacted, and rsync never copies/deletes.
  writeFileSync(join(bin, "ssh"), '#!/bin/bash\nprintf "ssh\\n" >> "$QA65_TRACE"\nexec /bin/bash -c "$2"\n', { mode: 0o755 });
  writeFileSync(join(bin, "rsync"), '#!/bin/bash\nprintf "rsync\\n" >> "$QA65_TRACE"\nexit 73\n', { mode: 0o755 });
  return {
    root, home,
    run(dest) {
      writeFileSync(trace, "");
      const result = spawnSync("bash", [script], {
        encoding: "utf8", timeout: 5000,
        env: { PATH: `${bin}:${process.env.PATH}`, HOME: home, QA65_TRACE: trace,
          QUEUE_LINUX_HOST: "local-fixture", QUEUE_LINUX_DIR: dest, QUEUE_LINUX_XVFB: "0" },
      });
      assert.ifError(result.error);
      return { ...result, calls: readFileSync(trace, "utf8").trim().split("\n") };
    },
  };
}

test("#65: a nested scratch checkout is marked before sync and may be reused with files", options, (t) => {
  const f = fixture(t);
  const dest = "work/queue-test.v1";
  for (let n = 0; n < 2; n++) {
    const result = f.run(dest);
    assert.equal(result.status, 73, result.stderr); // deliberate stop at the rsync stub
    assert.deepEqual(result.calls, ["ssh", "rsync"]);
    assert.ok(existsSync(join(f.home, dest, mark)));
    writeFileSync(join(f.home, dest, "owner.txt"), "preserve");
  }
  assert.equal(readFileSync(join(f.home, dest, "owner.txt"), "utf8"), "preserve");
});

test("#65: hidden files in an unmarked checkout refuse sync without marking it", options, (t) => {
  const f = fixture(t);
  const dest = join(f.home, "mine");
  mkdirSync(dest);
  writeFileSync(join(dest, ".keep"), "private data");
  const result = f.run("mine");
  assert.equal(result.status, 3, result.stderr);
  assert.match(result.stderr, /wasn't made by scripts\/test-linux\.sh/);
  assert.deepEqual(result.calls, ["ssh"]);
  assert.ok(!existsSync(join(dest, mark)));
  assert.equal(readFileSync(join(dest, ".keep"), "utf8"), "private data");
});

test("#65: marked targets outside home and aliases of home refuse sync", options, (t) => {
  const f = fixture(t);
  // A prefix lookalike catches a guard that checks '$home*' instead of '$home/?*'.
  const outside = `${f.home}-other`;
  mkdirSync(outside);
  writeFileSync(join(outside, mark), "existing marker");
  writeFileSync(join(outside, "keep"), "outside data");
  symlinkSync(outside, join(f.home, "outside"));
  symlinkSync(".", join(f.home, "self"));
  for (const dest of ["outside", "self", "self/self"]) {
    const result = f.run(dest);
    assert.equal(result.status, 3, `${dest}: ${result.stderr}`);
    assert.match(result.stderr, /not a folder inside/);
    assert.deepEqual(result.calls, ["ssh"]);
  }
  assert.ok(!existsSync(join(f.home, mark)));
  assert.equal(readFileSync(join(outside, "keep"), "utf8"), "outside data");
});

test("#65: an alias resolving strictly inside home is accepted", options, (t) => {
  const f = fixture(t);
  mkdirSync(join(f.home, "real"));
  symlinkSync("real", join(f.home, "alias"));
  const result = f.run("alias");
  assert.equal(result.status, 73, result.stderr);
  assert.deepEqual(result.calls, ["ssh", "rsync"]);
  assert.ok(existsSync(join(f.home, "real", mark)));
});
