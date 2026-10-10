// Independent QA: #2 / D6 / D-012. Run the unchanged scripts in disposable roots.
// Network and process faults are injected through Node's preload hook, never production edits.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { deflateRawSync } from "node:zlib";
import test from "node:test";

const project = new URL("../../", import.meta.url);
const host = { "darwin-arm64": "aarch64-apple-darwin", "darwin-x64": "x86_64-apple-darwin", "linux-x64": "x86_64-unknown-linux-gnu", "win32-x64": "x86_64-pc-windows-msvc" }[`${process.platform}-${process.arch}`];
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");
const payload = Buffer.from("fixture executable bytes\n");

function zip(name, bytes, method = 8) {
  const filename = Buffer.from(name);
  const data = method === 8 ? deflateRawSync(bytes) : bytes;
  // Standard non-ZIP64 local header, central directory, and end record.
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ ((crc & 1) ? 0xedb88320 : 0);
  }
  crc = (crc ^ 0xffffffff) >>> 0;
  const local = Buffer.alloc(30);
  local.writeUInt32LE(0x04034b50); local.writeUInt16LE(20, 4);
  local.writeUInt16LE(method, 8); local.writeUInt32LE(crc, 14);
  local.writeUInt32LE(data.length, 18); local.writeUInt32LE(bytes.length, 22);
  local.writeUInt16LE(filename.length, 26);
  const central = Buffer.alloc(46);
  central.writeUInt32LE(0x02014b50); central.writeUInt16LE(20, 4); central.writeUInt16LE(20, 6);
  central.writeUInt16LE(method, 10); central.writeUInt32LE(crc, 16);
  central.writeUInt32LE(data.length, 20); central.writeUInt32LE(bytes.length, 24);
  central.writeUInt16LE(filename.length, 28);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50); end.writeUInt16LE(1, 8); end.writeUInt16LE(1, 10);
  end.writeUInt32LE(central.length + filename.length, 12);
  end.writeUInt32LE(local.length + filename.length + data.length, 16);
  return Buffer.concat([local, filename, data, central, filename, end]);
}

function fixture(t, { target = host, archive, method = 8, dirname = "Queue 日本語 & spaces" } = {}) {
  assert.ok(target, "this fixture requires a supported host");
  const parent = mkdtempSync(join(tmpdir(), "queue-sidecar-qa-"));
  t.after(() => rmSync(parent, { recursive: true, force: true }));
  const root = join(parent, dirname);
  const scripts = join(root, "scripts");
  const bin = join(root, "src-tauri", "binaries");
  mkdirSync(scripts, { recursive: true }); mkdirSync(bin, { recursive: true });
  copyFileSync(new URL("scripts/fetch-sidecars.mjs", project), join(scripts, "fetch-sidecars.mjs"));
  const ext = target.includes("windows") ? ".exe" : "";
  const deno = archive ?? zip(`deno${ext}`, payload, method);
  const manifest = {
    ytdlp: { version: "2026.08.19", url: "https://qa.invalid/{version}/{asset}", assets: { [target]: { asset: "yt-dlp", sha256: digest(payload) } } },
    deno: { version: "v2.9.7", url: "https://qa.invalid/{version}/{asset}", assets: { [target]: { asset: "deno.zip", sha256: digest(deno) } } },
    ffmpeg: { version: "9.0.2" },
  };
  writeFileSync(join(scripts, "sidecars.json"), JSON.stringify(manifest));
  writeFileSync(join(root, "payload"), payload); writeFileSync(join(root, "deno.zip"), deno);
  const preload = join(root, "faults.mjs");
  writeFileSync(preload, `
import { readFileSync, appendFileSync } from 'node:fs';
import cp from 'node:child_process';
import { syncBuiltinESMExports } from 'node:module';
globalThis.fetch = async (url) => {
  appendFileSync(new URL('./requests', import.meta.url), url + '\\n');
  if (process.env.QA_HTTP_FAIL) return { ok: false, status: 503 };
  const bytes = process.env.QA_BAD_HASH ? Buffer.from('corrupt') : readFileSync(new URL(url.endsWith('deno.zip') ? './deno.zip' : './payload', import.meta.url));
  return { ok: true, arrayBuffer: async () => bytes };
};
if (process.env.QA_PROCESS) {
  cp.spawnSync = (file, args, options) => {
    if (!Array.isArray(args) || options.timeout !== 120000) throw new Error('invalid process contract');
    const header = '[debug] yt-dlp version stable@2026.08.19 fixture\\n[debug] exe versions: ffmpeg 9.0.2, ffprobe 9.0.2\\n[debug] JS runtimes: deno-2.9.7\\n';
    if (process.env.QA_PROCESS === 'timeout') return { stdout: '', stderr: header, status: null, signal: 'SIGTERM', error: Object.assign(new Error('spawnSync ETIMEDOUT'), { code: 'ETIMEDOUT' }) };
    if (process.env.QA_PROCESS === 'missing') return { stdout: '', stderr: '[debug] JS runtimes: none', status: 2 };
    return { stdout: '', stderr: header, status: 2 };
  };
  syncBuiltinESMExports();
}
`);
  const path = (name) => join(bin, `queue-${name}-${target}${ext}`);
  const run = (args = [], env = {}) => spawnSync(process.execPath, ["--import", pathToFileURL(preload).href, join(scripts, "fetch-sidecars.mjs"), "--target", target, ...args], {
    env: { ...process.env, ...env }, encoding: "utf8", timeout: 10000,
  });
  return { root, bin, scripts, manifest, path, run, deno };
}

for (const method of [0, 8]) {
  test(`installs byte-exact binaries from ZIP method ${method} in a unicode/spaced root`, (t) => {
    const f = fixture(t, { method }); const run = f.run();
    assert.equal(run.status, 0, run.stderr);
    for (const name of ["yt-dlp", "deno"]) {
      assert.deepEqual(readFileSync(f.path(name)), payload);
      assert.equal(existsSync(`${f.path(name)}.part`), false);
      if (process.platform !== "win32") assert.equal(statSync(f.path(name)).mode & 0o777, 0o755);
    }
  });
}

test("extracts the Windows deno.exe entry and installs triple-suffixed .exe files", (t) => {
  const f = fixture(t, { target: "x86_64-pc-windows-msvc" }); const run = f.run();
  assert.equal(run.status, 0, run.stderr);
  assert.deepEqual(readFileSync(f.path("deno")), payload);
  assert.deepEqual(readFileSync(f.path("yt-dlp")), payload);
});

test("checksum failure leaves an existing executable intact and caches no bad download", (t) => {
  const f = fixture(t); writeFileSync(f.path("yt-dlp"), "existing");
  const run = f.run([], { QA_BAD_HASH: "1" });
  assert.notEqual(run.status, 0); assert.match(run.stderr, /sha256 mismatch/);
  assert.equal(readFileSync(f.path("yt-dlp"), "utf8"), "existing");
  assert.equal(existsSync(join(f.bin, ".cache")), false);
  assert.equal(existsSync(`${f.path("yt-dlp")}.part`), false);
});

test("HTTP failure is reported without installing an executable", (t) => {
  const f = fixture(t); const run = f.run([], { QA_HTTP_FAIL: "1" });
  assert.notEqual(run.status, 0); assert.match(run.stderr, /HTTP 503/);
  assert.equal(existsSync(f.path("yt-dlp")), false);
});

test("verified cache avoids network; corrupt cache is downloaded and verified again", (t) => {
  const f = fixture(t);
  assert.equal(f.run().status, 0);
  const requests = join(f.root, "requests");
  assert.equal(readFileSync(requests, "utf8").trim().split("\n").length, 2);
  assert.equal(f.run([], { QA_HTTP_FAIL: "1" }).status, 0);
  assert.equal(readFileSync(requests, "utf8").trim().split("\n").length, 2);
  writeFileSync(join(f.bin, ".cache", "ytdlp-2026.08.19-yt-dlp"), "corrupt cache");
  assert.equal(f.run().status, 0);
  assert.equal(readFileSync(requests, "utf8").trim().split("\n").length, 3);
  assert.deepEqual(readFileSync(f.path("yt-dlp")), payload);
});

for (const [description, archive, error] of [
  ["not a ZIP", Buffer.from("not a zip"), /not a zip file/],
  ["missing deno entry", zip("wrong-entry", payload), /not found in zip/],
  ["unsupported compression", zip("deno", payload, 99), /unsupported zip method/],
]) {
  test(`rejects a verified archive with ${description}`, (t) => {
    // Use Linux target so the expected entry is deno even when tests run on Windows.
    const f = fixture(t, { target: "x86_64-unknown-linux-gnu", archive }); const run = f.run();
    assert.notEqual(run.status, 0); assert.match(run.stderr, error);
    assert.equal(existsSync(f.path("deno")), false);
  });
}

test("unsupported target exits 2 without downloading", (t) => {
  const f = fixture(t);
  const invalid = spawnSync(process.execPath, [join(f.scripts, "fetch-sidecars.mjs"), "--target", "unsupported"], { encoding: "utf8", timeout: 10000 });
  assert.equal(invalid.status, 2); assert.match(invalid.stderr, /unsupported target/);
  assert.equal(existsSync(join(f.root, "requests")), false);
});

test("--check rejects missing ffmpeg/ffprobe and missing debug versions", (t) => {
  const f = fixture(t);
  let run = f.run(["--check"], { QA_PROCESS: "valid" });
  assert.notEqual(run.status, 0); assert.match(run.stderr, /missing; run scripts\/build-ffmpeg/);
  writeFileSync(f.path("ffmpeg"), payload); writeFileSync(f.path("ffprobe"), payload);
  run = f.run(["--check"], { QA_PROCESS: "missing" });
  assert.notEqual(run.status, 0); assert.match(run.stderr, /debug header is missing/);
  run = f.run(["--check"], { QA_PROCESS: "valid" });
  assert.equal(run.status, 0, run.stderr); assert.match(run.stdout, /check ok/);
});

test("--check rejects a subprocess timeout even after the expected header is printed", (t) => {
  const f = fixture(t);
  writeFileSync(f.path("ffmpeg"), payload); writeFileSync(f.path("ffprobe"), payload);
  const run = f.run(["--check"], { QA_PROCESS: "timeout" });
  assert.notEqual(run.status, 0, "a timed-out yt-dlp invocation must not print check ok and exit successfully");
});
