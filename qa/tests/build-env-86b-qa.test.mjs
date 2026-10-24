// Independent #86(b), NFR / D-035. Execute the workflow's shell blocks with
// fixture probes; never install packages, build ffmpeg, or access the network.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";

const workflow = readFileSync(new URL("../../.github/workflows/build.yml", import.meta.url), "utf8").replaceAll("\r\n", "\n");
const job = workflow.match(/^  ffmpeg:\n([\s\S]*?)(?=^  [\w-]+:)/m)?.[1];
assert.ok(job, "ffmpeg job exists");
const entries = job.split(/^      - /m).slice(1);
const record = entries.find((s) => s.startsWith("name: Record the build environment\n"));
const split = entries.find((s) => s.startsWith("name: Split per target\n"));
const cache = entries.find((s) => s.startsWith("id: cache\n"));
function script(step) {
  assert.ok(step, "workflow step exists");
  return step.match(/^        run: \|\n((?:          .*\n|\n)*)/m)[1]
    .split("\n").map((line) => line.replace(/^          /, "")).join("\n");
}
const matrices = [
  { os: "macOS", key: "macos", targets: ["aarch64-apple-darwin", "x86_64-apple-darwin"] },
  { os: "Linux", key: "linux-windows", targets: ["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"] },
];
// These bash steps run only on macOS/Linux, even though npm test also runs on Windows.
const shellTest = (name, fn) => test(name, { skip: process.platform === "win32" }, fn);
const defaults = {
  xcode: "Xcode 26.1\nBuild version 17B55\n", sdk: "26.1\n",
  clang: "Apple clang version 17.0.0\nTarget: arm64-apple-darwin\n",
  gcc: "gcc (Ubuntu 13.3.0-6ubuntu2) 13.3.0\nCopyright fixture\n",
  make: "GNU Make 4.3\nBuilt for fixture\n",
  apt: "mingw-w64:\n  Installed: (none)\n  Candidate: 11.0.1-3\n  Version table:\n     11.0.1-3 500\n        500 fixture Packages\ngcc-mingw-w64-x86-64:\n  Installed: (none)\n  Candidate: 13.2.0-6\nlibz-mingw-w64-dev:\n  Installed: (none)\n  Candidate: 1.3.1-1\nzlib1g-dev:\n  Installed: 1:1.3.dfsg-3\n  Candidate: 1:1.3.dfsg-3\n",
};

function fixture(t, matrix) {
  const dir = mkdtempSync(join(tmpdir(), "queue-86b-qa-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const bin = join(dir, "probes");
  mkdirSync(bin);
  const probe = join(bin, "probe.cjs");
  writeFileSync(probe, `
const fs = require('node:fs');
const name = process.argv[2];
fs.appendFileSync(process.env.QA_CALLS, JSON.stringify([name, ...process.argv.slice(3)]) + '\\n');
if (process.env.QA_FAIL === name) process.exit(17);
const values = JSON.parse(process.env.QA_VERSIONS);
const key = { xcodebuild: 'xcode', xcrun: 'sdk', clang: 'clang', gcc: 'gcc', make: 'make', 'apt-cache': 'apt' }[name];
if (key) process.stdout.write(values[key]);
`);
  for (const command of ["xcodebuild", "xcrun", "clang", "sudo", "apt-cache", "gcc", "make"]) {
    writeFileSync(join(bin, command), `#!/bin/sh\nexec "$QA_NODE" "$QA_PROBE" ${command} "$@"\n`, { mode: 0o755 });
  }
  const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, RUNNER_OS: matrix.os,
    ImageOS: matrix.os === "macOS" ? "macos26" : "ubuntu24", ImageVersion: "20261020.1",
    QA_NODE: process.execPath, QA_PROBE: probe, QA_CALLS: join(dir, "calls.jsonl"), QA_VERSIONS: JSON.stringify(defaults) };
  const run = (body, extra = {}) => spawnSync("bash", ["--noprofile", "--norc", "-e", "-o", "pipefail", "-c", body], {
    cwd: dir, env: { ...env, ...extra }, encoding: "utf8", timeout: 5000,
  });
  const capture = (versions = defaults) => {
    const result = run(script(record), { QA_VERSIONS: JSON.stringify(versions) });
    assert.equal(result.status, 0, result.stderr);
    return readFileSync(join(dir, "build-env.txt"), "utf8");
  };
  return { dir, run, capture, calls: () => readFileSync(env.QA_CALLS, "utf8").trim().split("\n").map(JSON.parse) };
}

test("#86(b): recording is unconditional and precedes an environment-specific exact cache lookup", () => {
  assert.ok(entries.indexOf(record) < entries.indexOf(cache));
  assert.doesNotMatch(record, /^        if:/m, "also capture on a cache hit");
  assert.match(cache, /hashFiles\('build-env\.txt'\)/);
  assert.match(cache, /hashFiles\('scripts\/build-ffmpeg\.sh', 'scripts\/sidecars\.json'\)/);
  assert.match(cache, /matrix\.key/);
  assert.doesNotMatch(cache, /^          restore-keys:/m, "no fallback to a different environment");
  assert.match(cache, /^          path: src-tauri\/binaries\/queue-ff\*$/m);
});

for (const matrix of matrices) {
  shellTest(`#86(b): ${matrix.os} records expected values and invokes only its platform probes`, (t) => {
    const f = fixture(t, matrix);
    const text = f.capture();
    assert.equal(text, matrix.os === "macOS"
      ? "image macos26 20261020.1\nXcode 26.1\nBuild version 17B55\nSDK 26.1\nApple clang version 17.0.0\nGNU Make 4.3\n"
      : "image ubuntu24 20261020.1\nmingw-w64: 11.0.1-3\ngcc-mingw-w64-x86-64: 13.2.0-6\nlibz-mingw-w64-dev: 1.3.1-1\nzlib1g-dev: 1:1.3.dfsg-3\ngcc (Ubuntu 13.3.0-6ubuntu2) 13.3.0\nGNU Make 4.3\n");
    assert.deepEqual(f.calls(), matrix.os === "macOS"
      ? [["xcodebuild", "-version"], ["xcrun", "--show-sdk-version"], ["clang", "--version"], ["make", "--version"]]
      : [["sudo", "apt-get", "update"], ["apt-cache", "policy", "mingw-w64", "gcc-mingw-w64-x86-64", "libz-mingw-w64-dev", "zlib1g-dev"], ["gcc", "--version"], ["make", "--version"]]);
  });

  shellTest(`#86(b): ${matrix.os} replaces stale metadata and keeps identical environments stable`, (t) => {
    const f = fixture(t, matrix);
    writeFileSync(join(f.dir, "build-env.txt"), "stale metadata\n");
    const first = f.capture();
    assert.doesNotMatch(first, /stale metadata/);
    assert.equal(f.capture(), first);
  });

  for (const key of matrix.os === "macOS" ? ["xcode", "sdk", "clang", "make"] : ["apt", "gcc", "make"]) {
    shellTest(`#86(b): ${matrix.os} ${key} version change changes the environment digest`, (t) => {
      const f = fixture(t, matrix);
      const hash = (s) => createHash("sha256").update(s).digest("hex");
      const original = hash(f.capture());
      const updated = defaults[key].replace(/\d/, (digit) => String((Number(digit) + 1) % 10));
      assert.notEqual(hash(f.capture({ ...defaults, [key]: updated })), original);
    });
  }

  // The runner image covers the linker and libraries no probe reads (#86 review).
  for (const variable of ["ImageOS", "ImageVersion"]) {
    shellTest(`#86(b): ${matrix.os} ${variable} change alone changes the environment digest`, (t) => {
      const f = fixture(t, matrix);
      const hash = (text) => createHash("sha256").update(text).digest("hex");
      const before = f.run(script(record));
      assert.equal(before.status, 0, before.stderr);
      const original = hash(readFileSync(join(f.dir, "build-env.txt")));
      const after = f.run(script(record), { [variable]: "changed" });
      assert.equal(after.status, 0, after.stderr);
      assert.notEqual(hash(readFileSync(join(f.dir, "build-env.txt"))), original);
    });
  }

  for (const hit of [false, true]) {
    shellTest(`#86(b): ${matrix.os} cache ${hit ? "hit" : "miss"} uploads metadata beside clean target folders`, (t) => {
      const f = fixture(t, matrix);
      const expected = f.capture();
      const binaries = join(f.dir, "src-tauri", "binaries");
      mkdirSync(binaries, { recursive: true });
      for (const target of matrix.targets) {
        for (const program of ["ffmpeg", "ffprobe"]) {
          const name = `queue-${program}-${target}${target.includes("windows") ? ".exe" : ""}`;
          writeFileSync(join(binaries, name), hit ? "restored cache fixture" : "fresh build fixture");
        }
      }
      assert.doesNotMatch(split, /^        if:/m, "metadata must be uploaded on hits too");
      const body = script(split).replaceAll("${{ matrix.targets }}", matrix.targets.join(" "))
        .replaceAll("${{ matrix.key }}", matrix.key);
      const result = f.run(body);
      assert.equal(result.status, 0, result.stderr);
      const filename = `build-env-${matrix.key}.txt`;
      assert.equal(readFileSync(join(f.dir, "out", filename), "utf8"), expected);
      assert.deepEqual(readdirSync(join(f.dir, "out")).sort(), [filename, ...matrix.targets].sort());
      for (const target of matrix.targets) {
        assert.equal(readdirSync(join(f.dir, "out", target)).length, 2);
        assert.ok(readdirSync(join(f.dir, "out", target)).every((name) => name.startsWith("queue-ff")));
      }
      const upload = entries.find((s) => s.startsWith("uses: actions/upload-artifact@"));
      assert.match(upload, /^          path: out\/$/m);
      assert.doesNotMatch(upload, /^        if:/m);
    });
  }
}

for (const [matrix, command] of [[matrices[0], "xcodebuild"], [matrices[0], "xcrun"], [matrices[0], "clang"], [matrices[1], "sudo"], [matrices[1], "apt-cache"], [matrices[1], "gcc"], [matrices[1], "make"]]) {
  shellTest(`#86(b): a failing ${matrix.os} ${command} probe stops before cache lookup`, (t) => {
    const f = fixture(t, matrix);
    const result = f.run(script(record), { QA_FAIL: command });
    assert.notEqual(result.status, 0, "GitHub's explicit bash shell uses -e -o pipefail");
  });
}
