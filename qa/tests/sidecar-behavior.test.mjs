// Independent QA: #2 / D6 / D-012. Run the unchanged scripts in disposable roots.
// Network and process faults are injected through Node's preload hook, never production edits.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { deflateRawSync, gzipSync } from "node:zlib";
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

// A gzipped ustar archive: [name, bytes] are regular files, [name, null, target] symlinks.
function targz(entries) {
  const blocks = [];
  for (const [name, bytes, link] of entries) {
    const header = Buffer.alloc(512);
    header.write(name, 0, "utf8");
    header.write("0000755\0", 100); header.write("0000000\0", 108); header.write("0000000\0", 116);
    header.write(`${(bytes?.length ?? 0).toString(8).padStart(11, "0")}\0`, 124);
    header.write("00000000000\0", 136);
    header.write(bytes ? "0" : "2", 156);
    if (link) header.write(link, 157);
    header.write("ustar\0", 257); header.write("00", 263);
    header.fill(" ", 148, 156);
    let sum = 0;
    for (const b of header) sum += b;
    header.write(`${sum.toString(8).padStart(6, "0")}\0 `, 148);
    blocks.push(header);
    if (bytes) blocks.push(bytes, Buffer.alloc((512 - (bytes.length % 512)) % 512));
  }
  blocks.push(Buffer.alloc(1024));
  return gzipSync(Buffer.concat(blocks));
}

function fixture(t, { target = host, archive, method = 8, dirname = "Queue 日本語 & spaces", llamaArchive, llamaFiles, llamaFormat = "tar.gz" } = {}) {
  assert.ok(target, "this fixture requires a supported host");
  const parent = mkdtempSync(join(tmpdir(), "queue-sidecar-qa-"));
  t.after(() => rmSync(parent, { recursive: true, force: true }));
  const root = join(parent, dirname);
  const scripts = join(root, "scripts");
  const bin = join(root, "src-tauri", "binaries");
  mkdirSync(scripts, { recursive: true }); mkdirSync(bin, { recursive: true });
  copyFileSync(new URL("scripts/fetch-sidecars.mjs", project), join(scripts, "fetch-sidecars.mjs"));
  mkdirSync(join(scripts, "lib"));
  copyFileSync(new URL("scripts/lib/signature.mjs", project), join(scripts, "lib/signature.mjs"));
  const ext = target.includes("windows") ? ".exe" : "";
  const deno = archive ?? zip(`deno${ext}`, payload, method);
  // #30: llama-server, as a one-entry zip on every host (the script picks its reader by name).
  const llama = llamaArchive ?? zip(`llama-server${ext}`, payload, method);
  const manifest = {
    ytdlp: { version: "2026.08.19", url: "https://qa.invalid/{version}/{asset}", assets: { [target]: { asset: "yt-dlp", sha256: digest(payload) } } },
    deno: { version: "v2.9.7", url: "https://qa.invalid/{version}/{asset}", assets: { [target]: { asset: "deno.zip", sha256: digest(deno) } } },
    llama: { version: "b11669", url: "https://qa.invalid/{version}/{asset}", assets: { [target]: llamaArchive
      ? { asset: `llama.${llamaFormat}`, sha256: digest(llama), prefix: llamaFormat === "zip" ? "" : "llama-b11669/", files: llamaFiles }
      : { asset: "llama.zip", sha256: digest(llama), prefix: "", files: { [`llama-server${ext}`]: `llama-server${ext}` } } } },
    ffmpeg: { version: "9.0.2" },
  };
  writeFileSync(join(scripts, "sidecars.json"), JSON.stringify(manifest));
  writeFileSync(join(root, "payload"), payload); writeFileSync(join(root, "deno.zip"), deno);
  writeFileSync(join(root, llamaArchive ? `llama.${llamaFormat}` : "llama.zip"), llama);
  const preload = join(root, "faults.mjs");
  writeFileSync(preload, `
import { readFileSync, appendFileSync } from 'node:fs';
import cp from 'node:child_process';
import { syncBuiltinESMExports } from 'node:module';
globalThis.fetch = async (url) => {
  appendFileSync(new URL('./requests', import.meta.url), url + '\\n');
  if (process.env.QA_HTTP_FAIL) return { ok: false, status: 503 };
  if (process.env.QA_LLAMA_HTTP_FAIL && /llama\\.(zip|tar\\.gz)$/.test(url)) return { ok: false, status: 503 };
  if (process.env.QA_BAD_LLAMA_HASH && /llama\\.(zip|tar\\.gz)$/.test(url)) return { ok: true, arrayBuffer: async () => Buffer.from('corrupt llama') };
  const bytes = process.env.QA_BAD_HASH ? Buffer.from('corrupt') : readFileSync(new URL(url.endsWith('deno.zip') ? './deno.zip' : url.endsWith('llama.zip') ? './llama.zip' : url.endsWith('llama.tar.gz') ? './llama.tar.gz' : './payload', import.meta.url));
  return { ok: true, arrayBuffer: async () => bytes };
};
if (process.env.QA_PROCESS || process.env.QA_LLAMA_PROCESS) {
  cp.spawnSync = (file, args, options) => {
    // #30: llama-server --version / --list-devices, run from its folder.
    if (Array.isArray(args) && ['--version', '--list-devices'].includes(args[0]) && options.timeout === 60000 && options.cwd) {
      appendFileSync(new URL('./llama-calls', import.meta.url), JSON.stringify({ file, args, options }) + '\\n');
      const fail = process.env.QA_LLAMA_FLAG ?? '--version';
      if (args[0] === fail) {
        const mode = process.env.QA_LLAMA_PROCESS;
        if (mode === 'timeout') return { stdout: 'version: 11669', stderr: 'loaded CPU backend', status: null, signal: 'SIGTERM', error: Object.assign(new Error('spawnSync ETIMEDOUT'), { code: 'ETIMEDOUT' }) };
        if (mode === 'signal') return { stdout: 'version: 11669', stderr: 'loaded CPU backend', status: null, signal: 'SIGKILL' };
        if (mode === 'exit') return { stdout: 'version: 11669', stderr: 'loaded CPU backend', status: 1 };
        if (mode === 'missing') return { stdout: '', stderr: '', status: null, error: Object.assign(new Error('spawnSync ENOENT'), { code: 'ENOENT' }) };
        if (mode === 'wrong-version') return { stdout: 'version: 11668', stderr: '', status: 0 };
        // What CI's CPU-only Windows and Linux runners print (PR #95).
        if (mode === 'cpu-only') return { stdout: 'Available devices:\\n  (none)\\n', stderr: '', status: 0 };
        if (mode === 'no-list') return { stdout: 'usage: llama-server [options]', stderr: '', status: 0 };
        if (mode === 'metal') return { stdout: 'Available devices:\\n  MTL0: Apple M1 (10922 MiB, 10922 MiB free)\\n', stderr: '', status: 0 };
        if (mode === 'stderr-list') return { stdout: '', stderr: 'Available devices:\\n  (none)\\n', status: 0 };
      }
      if (args[0] === '--list-devices') return { stdout: 'Available devices:\\n  (none)\\n', stderr: '', status: 0 };
      return { stdout: 'version: 11669 (fixture)\\n', stderr: '', status: 0 };
    }
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
  // yt-dlp, deno and (#30) llama-server.
  assert.equal(readFileSync(requests, "utf8").trim().split("\n").length, 3);
  assert.equal(f.run([], { QA_HTTP_FAIL: "1" }).status, 0);
  assert.equal(readFileSync(requests, "utf8").trim().split("\n").length, 3);
  writeFileSync(join(f.bin, ".cache", "ytdlp-2026.08.19-yt-dlp"), "corrupt cache");
  assert.equal(f.run().status, 0);
  assert.equal(readFileSync(requests, "utf8").trim().split("\n").length, 4);
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

test("#30: llama-server installs from a tar.gz with only the listed files, under their loader names", (t) => {
  const lib = Buffer.from("versioned library bytes\n");
  const archive = targz([
    ["llama-b11669/llama-server", payload],
    ["llama-b11669/libllama.0.7.0.dylib", lib],
    ["llama-b11669/libllama.0.dylib", null, "libllama.0.7.0.dylib"],
    ["llama-b11669/llama-cli", Buffer.from("not wanted")],
  ]);
  const f = fixture(t, {
    llamaArchive: archive,
    llamaFiles: { "llama-server": "llama-server", "libllama.0.dylib": "libllama.0.7.0.dylib" },
  });
  const result = f.run();
  assert.equal(result.status, 0, result.stderr);
  const dir = join(f.bin, "llama");
  assert.deepEqual(readFileSync(join(dir, "llama-server")), payload);
  assert.deepEqual(readFileSync(join(dir, "libllama.0.dylib")), lib, "a real file under the loader's name");
  assert.equal(readFileSync(join(dir, "target.txt"), "utf8"), `${host}\n`);
  assert.ok(!existsSync(join(dir, "llama-cli")), "only the listed files");
  assert.ok(!existsSync(join(dir, "libllama.0.7.0.dylib")));
  assert.ok(!existsSync(`${dir}.part`), "the staging folder is gone");
  if (process.platform !== "win32") assert.equal(statSync(join(dir, "llama-server")).mode & 0o111, 0o111);
  // A listed file missing from the archive fails without touching the installed folder.
  const g = fixture(t, { llamaArchive: archive, llamaFiles: { "llama-server": "llama-server", "libmissing.dylib": "libmissing.dylib" } });
  const missing = g.run();
  assert.notEqual(missing.status, 0);
  assert.match(missing.stderr, /llama-b11669\/libmissing\.dylib not found/);
  assert.ok(!existsSync(join(g.bin, "llama")));
});

// Independent N6 / #30(a) regressions. Use the unchanged installer in disposable roots;
// faults target llama only so earlier yt-dlp/deno failures cannot mask these assertions.
function zipFiles(entries) {
  const locals = [], centrals = [];
  let offset = 0;
  for (const [name, bytes] of entries) {
    const single = zip(name, bytes);
    const end = single.subarray(single.length - 22);
    const centralOffset = end.readUInt32LE(16);
    const local = single.subarray(0, centralOffset);
    const central = Buffer.from(single.subarray(centralOffset, single.length - 22));
    central.writeUInt32LE(offset, 42);
    locals.push(local); centrals.push(central);
    offset += local.length;
  }
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50);
  end.writeUInt16LE(entries.length, 8); end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(centrals.reduce((n, b) => n + b.length, 0), 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, ...centrals, end]);
}

const llamaPins = JSON.parse(readFileSync(new URL("scripts/sidecars.json", project), "utf8")).llama;
for (const [target, pin] of Object.entries(llamaPins.assets)) {
  test(`#30(a): extracts every pinned server, library and license for ${target}`, (t) => {
    const entries = [...new Set(Object.values(pin.files))].map((name) => [pin.prefix + name, Buffer.from(`bytes for ${name}\n`)]);
    entries.push([pin.prefix + "unlisted-tool", Buffer.from("must not install")]);
    const llamaFormat = pin.asset.endsWith(".zip") ? "zip" : "tar.gz";
    const f = fixture(t, { target, llamaFormat, llamaFiles: pin.files, llamaArchive: llamaFormat === "zip" ? zipFiles(entries) : targz(entries) });
    const result = f.run();
    assert.equal(result.status, 0, result.stderr);
    const dir = join(f.bin, "llama");
    for (const [loaderName, entry] of Object.entries(pin.files)) {
      assert.equal(readFileSync(join(dir, loaderName), "utf8"), `bytes for ${entry}\n`, loaderName);
      assert.ok(statSync(join(dir, loaderName)).isFile(), `${loaderName} is a regular file`);
    }
    assert.equal(readFileSync(join(dir, "target.txt"), "utf8"), `${target}\n`);
    assert.ok(!existsSync(join(dir, "unlisted-tool")));
  });
}

for (const [env, message] of [[{ QA_BAD_LLAMA_HASH: "1" }, /sha256 mismatch/], [{ QA_LLAMA_HTTP_FAIL: "1" }, /HTTP 503/]]) {
  test(`#30(a): llama ${Object.keys(env)[0]} preserves the installed folder and rejects the download`, (t) => {
    const f = fixture(t);
    const dir = join(f.bin, "llama");
    mkdirSync(dir);
    writeFileSync(join(dir, "sentinel"), "previous installation");
    const result = f.run([], env);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, message);
    assert.equal(readFileSync(join(dir, "sentinel"), "utf8"), "previous installation");
    assert.ok(!existsSync(join(f.bin, ".cache", "llama-b11669-llama.zip")));
    assert.ok(!existsSync(`${dir}.part`));
  });
}

test("#30(a): a missing library preserves an existing installation; retry removes stale staging and libraries", (t) => {
  const f = fixture(t, {
    llamaArchive: targz([["llama-b11669/llama-server", payload]]),
    llamaFiles: { "llama-server": "llama-server", "missing.so": "missing.so" },
  });
  const dir = join(f.bin, "llama");
  mkdirSync(dir);
  writeFileSync(join(dir, "old-library"), "keep until the replacement is complete");
  const failed = f.run();
  assert.notEqual(failed.status, 0);
  assert.match(failed.stderr, /missing\.so not found/);
  assert.equal(readFileSync(join(dir, "old-library"), "utf8"), "keep until the replacement is complete");
  // The next attempt may use a corrected manifest, but must not keep half-written files
  // from the failed attempt or obsolete libraries from the previous installation.
  writeFileSync(join(f.bin, "llama.part", "stale-staging"), "stale");
  delete f.manifest.llama.assets[host].files["missing.so"];
  writeFileSync(join(f.scripts, "sidecars.json"), JSON.stringify(f.manifest));
  const retried = f.run([], { QA_HTTP_FAIL: "1" });
  assert.equal(retried.status, 0, retried.stderr);
  assert.deepEqual(readFileSync(join(dir, "llama-server")), payload);
  assert.ok(!existsSync(join(dir, "old-library")));
  assert.ok(!existsSync(join(dir, "stale-staging")));
  assert.ok(!existsSync(`${dir}.part`));
});

test("#30(a): corrupt llama cache is fetched and verified again", (t) => {
  const f = fixture(t);
  assert.equal(f.run().status, 0);
  writeFileSync(join(f.bin, ".cache", "llama-b11669-llama.zip"), "corrupt cache");
  const result = f.run();
  assert.equal(result.status, 0, result.stderr);
  const requests = readFileSync(join(f.root, "requests"), "utf8").trim().split("\n");
  assert.equal(requests.length, 4, "only llama needs another request");
  assert.match(requests.at(-1), /llama\.zip$/);
});

for (const flag of ["--version", "--list-devices"]) {
  for (const fault of ["timeout", "signal", "exit", "missing"]) {
    test(`#30(a): ${flag} ${fault} cannot be reported as a successful llama check`, (t) => {
      const f = fixture(t);
      assert.equal(f.run().status, 0);
      const result = f.run(["--check-llama", join(f.bin, "llama")], { QA_LLAMA_FLAG: flag, QA_LLAMA_PROCESS: fault });
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, /llama-server .* failed/);
      assert.doesNotMatch(result.stdout, /check ok/);
      const calls = readFileSync(join(f.root, "llama-calls"), "utf8").trim().split("\n").map(JSON.parse);
      assert.equal(calls.at(-1).args[0], flag);
    });
  }
}

test("#30(a): check-only uses argv and the unicode resource folder without downloads", (t) => {
  const f = fixture(t, { dirname: "Queue 日本語 ' & $(echo nope); space" });
  assert.equal(f.run().status, 0);
  const dir = join(f.bin, "llama");
  const requests = readFileSync(join(f.root, "requests"), "utf8");
  const result = f.run(["--check-llama", dir], { QA_PROCESS: "valid", QA_HTTP_FAIL: "1" });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(readFileSync(join(f.root, "requests"), "utf8"), requests);
  const calls = readFileSync(join(f.root, "llama-calls"), "utf8").trim().split("\n").map(JSON.parse);
  assert.deepEqual(calls.map((c) => c.args), [["--version"], ["--list-devices"]]);
  for (const call of calls) {
    assert.equal(call.file, join(dir, `llama-server${host.includes("windows") ? ".exe" : ""}`));
    assert.equal(call.options.cwd, dir);
    assert.equal(call.options.timeout, 60000);
    assert.ok(!call.options.shell);
  }
});

test("#30(a): check-only refuses the wrong target before spawning", (t) => {
  const f = fixture(t);
  assert.equal(f.run().status, 0);
  const dir = join(f.bin, "llama");
  writeFileSync(join(dir, "target.txt"), "wrong-target\n");
  const result = f.run(["--check-llama", dir], { QA_PROCESS: "valid" });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /holds wrong-target, not/);
  assert.ok(!existsSync(join(f.root, "llama-calls")));
});

test("#30(a): check-only rejects a different build", (t) => {
  const f = fixture(t);
  assert.equal(f.run().status, 0);
  const result = f.run(["--check-llama", join(f.bin, "llama")], { QA_LLAMA_PROCESS: "wrong-version" });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /doesn't say 11669/);
});

for (const target of ["x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"]) {
  test(`#30(a): ${target} passes with a CPU-only device list, and fails without one`, (t) => {
    const f = fixture(t, { target });
    assert.equal(f.run().status, 0);
    const dir = join(f.bin, "llama");
    const cpu = f.run(["--check-llama", dir], { QA_LLAMA_FLAG: "--list-devices", QA_LLAMA_PROCESS: "cpu-only" });
    assert.equal(cpu.status, 0, cpu.stderr);
    const result = f.run(["--check-llama", dir], { QA_LLAMA_FLAG: "--list-devices", QA_LLAMA_PROCESS: "no-list" });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /didn't list devices/);
  });
}

for (const target of ["aarch64-apple-darwin", "x86_64-apple-darwin"]) {
  test(`#30(a) review: ${target} accepts an accelerator list or a stderr-only list, and needs a list`, (t) => {
    const f = fixture(t, { target });
    assert.equal(f.run().status, 0);
    const dir = join(f.bin, "llama");
    for (const mode of ["metal", "stderr-list"]) {
      const ok = f.run(["--check-llama", dir], { QA_LLAMA_FLAG: "--list-devices", QA_LLAMA_PROCESS: mode });
      assert.equal(ok.status, 0, `${mode}: ${ok.stderr}`);
    }
    const none = f.run(["--check-llama", dir], { QA_LLAMA_FLAG: "--list-devices", QA_LLAMA_PROCESS: "no-list" });
    assert.notEqual(none.status, 0);
    assert.match(none.stderr, /didn't list devices/);
  });
}
