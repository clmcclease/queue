#!/usr/bin/env node
// Fetch the pinned yt-dlp and deno sidecars into src-tauri/binaries (#2, D-012), and
// llama.cpp's llama-server with its libraries into src-tauri/binaries/llama/ (#30, D-029).
// ffmpeg/ffprobe are built from source separately: scripts/build-ffmpeg.sh.
//
//   node scripts/fetch-sidecars.mjs [--target <triple>] [--check]
//   node scripts/fetch-sidecars.mjs [--target <triple>] --check-llama <dir>
//   node scripts/fetch-sidecars.mjs --test-model <dir>
//   node scripts/fetch-sidecars.mjs --verify-signatures
//
// --target defaults to the host. --check runs yt-dlp's verbose header to confirm it sees our
// ffmpeg, ffprobe and deno, and runs llama-server --version and --list-devices to confirm it
// and its libraries load (skipped when the target can't run on this host). --test-model only
// fetches the pinned test GGUF (CI's real llama-server test, #30(b)) into <dir>.
// --verify-signatures checks, with gpg, that yt-dlp's signed SHA2-256SUMS for the pinned version
// pairs every pinned asset name with its pinned sha256 (#44), and that the pinned ffmpeg source
// tarball carries FFmpeg's release signature (#86, D-035). CI's own `verify-signatures` job runs it
// before any job builds or runs a sidecar; other modes never need gpg.
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { arch, platform } from "node:process";
import { fileURLToPath } from "node:url";
import { gunzipSync, inflateRawSync } from "node:zlib";

const root = new URL("../", import.meta.url);
const manifest = JSON.parse(readFileSync(new URL("scripts/sidecars.json", root), "utf8"));
const binDir = fileURLToPath(new URL("src-tauri/binaries/", root));
const cacheDir = join(binDir, ".cache");

const HOSTS = {
  "darwin-arm64": "aarch64-apple-darwin",
  "darwin-x64": "x86_64-apple-darwin",
  "win32-x64": "x86_64-pc-windows-msvc",
  "linux-x64": "x86_64-unknown-linux-gnu",
};
const host = HOSTS[`${platform}-${arch}`];

const args = process.argv.slice(2);

if (args.includes("--verify-signatures")) {
  try {
    await verifySignatures();
    process.exit(0);
  } catch (e) {
    console.error(`--verify-signatures: ${e.message}`);
    process.exit(1);
  }
}

// Checks every signed sidecar with gpg, each in its own throwaway home: yt-dlp's signed
// SHA2-256SUMS (#44), then the ffmpeg source tarball's .asc (#86). Every download is untrusted
// data: only gpg's file arguments, the sums parser's input or a hasher's input.
async function verifySignatures() {
  // Loaded only here, so the other modes run from a copy of this file alone.
  const { parseSums, unsignedAssets } = await import("./lib/signature.mjs");
  const sha256 = (buf) => createHash("sha256").update(buf).digest("hex");
  const { GPG_MISSING, verifyDetached } = await import("./lib/gpg.mjs");
  for (const name of ["ytdlp", "ffmpeg"]) {
    const { signing } = manifest[name];
    if (!signing?.fingerprint || !signing?.key) throw new Error(`sidecars.json has no ${name}.signing key and fingerprint`);
  }
  // gpg is checked first, so a machine without it fails before any download.
  if (spawnSync("gpg", ["--version"]).error?.code === "ENOENT") throw new Error(GPG_MISSING);
  const fetchInto = async (home, src, name) => {
    const res = await fetch(src);
    if (!res.ok) throw new Error(`${src}: HTTP ${res.status}`);
    const buf = Buffer.from(await res.arrayBuffer());
    writeFileSync(join(home, name), buf);
    return buf;
  };
  const inHome = async (work) => {
    const home = mkdtempSync(join(tmpdir(), "queue-gpg-"));
    try {
      return await work(home);
    } finally {
      rmSync(home, { recursive: true, force: true });
    }
  };
  const keyOf = ({ signing }) => ({ keyFile: fileURLToPath(new URL(signing.key, root)), fingerprint: signing.fingerprint });

  // yt-dlp (#44): the signed sums must pair every pinned asset name with its pinned sha256.
  await inHome(async (home) => {
    const { version, url, assets } = manifest.ytdlp;
    const from = (name) => url.replace("{version}", version).replace("{asset}", name);
    const sums = (await fetchInto(home, from("SHA2-256SUMS"), "SHA2-256SUMS")).toString("utf8");
    await fetchInto(home, from("SHA2-256SUMS.sig"), "SHA2-256SUMS.sig");
    const want = verifyDetached({ home, ...keyOf(manifest.ytdlp), dataFile: join(home, "SHA2-256SUMS"), sigFile: join(home, "SHA2-256SUMS.sig") });
    const problems = unsignedAssets(parseSums(sums), assets);
    if (problems.length > 0) throw new Error(`pinned assets don't match the signed sums:\n${problems.join("\n")}`);
    console.log(`VALIDSIG ${want}: yt-dlp ${version}'s signed sums pair every pinned asset with its sha256 (${new Set(Object.values(assets).map((a) => a.asset)).size} assets)`);
  });

  // ffmpeg (#86): the pinned tarball's sha256 is checked before gpg, so the signed bytes are the
  // pinned bytes.
  await inHome(async (home) => {
    const { version, url, sha256: pinned } = manifest.ffmpeg;
    const tarball = url.split("/").pop();
    const got = sha256(await fetchInto(home, url, tarball));
    if (got !== pinned) throw new Error(`${tarball}: sha256 mismatch, got ${got}, want ${pinned}`);
    await fetchInto(home, `${url}.asc`, `${tarball}.asc`);
    const want = verifyDetached({ home, ...keyOf(manifest.ffmpeg), dataFile: join(home, tarball), sigFile: join(home, `${tarball}.asc`) });
    console.log(`VALIDSIG ${want}: ffmpeg ${version}'s source tarball matches its pinned sha256 and its signature`);
  });
}

const checkOnly = args.indexOf("--check-llama");
const targetIndex = args.indexOf("--target");
const target = targetIndex >= 0 ? args[targetIndex + 1] : host;
if (!target || !manifest.ytdlp.assets[target]) {
  console.error(`unsupported target ${target ?? `(host ${platform}-${arch})`}; expected one of ${Object.keys(HOSTS).map((k) => HOSTS[k]).join(", ")}`);
  process.exit(2);
}
const exe = target.includes("windows") ? ".exe" : "";
const sidecar = (name) => join(binDir, `queue-${name}-${target}${exe}`);
const sha256 = (buf) => createHash("sha256").update(buf).digest("hex");

async function download(component) {
  const { version, url, assets } = manifest[component];
  const { asset, sha256: want } = assets[target];
  const cached = join(cacheDir, `${component}-${version}-${asset}`);
  if (existsSync(cached) && sha256(readFileSync(cached)) === want) return readFileSync(cached);
  const src = url.replace("{version}", version).replace("{asset}", asset);
  console.log(`downloading ${src}`);
  const res = await fetch(src);
  if (!res.ok) throw new Error(`${src}: HTTP ${res.status}`);
  const buf = Buffer.from(await res.arrayBuffer());
  const got = sha256(buf);
  if (got !== want) throw new Error(`${asset}: sha256 mismatch, got ${got}, want ${want}`);
  mkdirSync(cacheDir, { recursive: true });
  writeFileSync(`${cached}.part`, buf);
  renameSync(`${cached}.part`, cached);
  return buf;
}

// Minimal zip reader: find one entry by name via the central directory (stored or deflated).
function unzipEntry(zip, name) {
  const eocd = zip.lastIndexOf(Buffer.from([0x50, 0x4b, 0x05, 0x06]));
  if (eocd < 0) throw new Error("not a zip file");
  let p = zip.readUInt32LE(eocd + 16);
  for (let i = 0, n = zip.readUInt16LE(eocd + 10); i < n; i++) {
    const method = zip.readUInt16LE(p + 10);
    const size = zip.readUInt32LE(p + 20);
    const nameLen = zip.readUInt16LE(p + 28);
    const skip = nameLen + zip.readUInt16LE(p + 30) + zip.readUInt16LE(p + 32);
    const local = zip.readUInt32LE(p + 42);
    if (zip.toString("utf8", p + 46, p + 46 + nameLen) === name) {
      const start = local + 30 + zip.readUInt16LE(local + 26) + zip.readUInt16LE(local + 28);
      const data = zip.subarray(start, start + size);
      if (method === 0) return data;
      if (method === 8) return inflateRawSync(data);
      throw new Error(`${name}: unsupported zip method ${method}`);
    }
    p += 46 + skip;
  }
  throw new Error(`${name} not found in zip`);
}

// Minimal tar.gz reader: the regular files (ustar or plain), by full name. Symlinks are
// skipped; the manifest names each library's real file.
function untar(gz) {
  const tar = gunzipSync(gz);
  const files = new Map();
  for (let p = 0; p + 512 <= tar.length; ) {
    const header = tar.subarray(p, p + 512);
    if (header.every((b) => b === 0)) break;
    const text = (from, to) => header.toString("utf8", from, to).replace(/\0.*$/s, "");
    const size = parseInt(text(124, 136).trim() || "0", 8);
    const type = text(156, 157);
    const prefix = text(257, 263).startsWith("ustar") ? text(345, 500) : "";
    const name = prefix ? `${prefix}/${text(0, 100)}` : text(0, 100);
    if (type === "0" || type === "") files.set(name, tar.subarray(p + 512, p + 512 + size));
    p += 512 + Math.ceil(size / 512) * 512;
  }
  return files;
}

// llama-server and its libraries go in one folder, bundled as a resource (D-029): upstream's
// @loader_path/$ORIGIN rpath and ggml's backend search both look beside the executable. The
// folder is built aside and swapped in whole, and records its target.
const llamaDir = join(binDir, "llama");
const llamaServer = join(llamaDir, `llama-server${exe}`);

function installLlama(archive) {
  const { prefix, files, asset, repoFiles = {} } = manifest.llama.assets[target];
  const entries = asset.endsWith(".zip") ? null : untar(archive);
  const staging = `${llamaDir}.part`;
  rmSync(staging, { recursive: true, force: true });
  mkdirSync(staging, { recursive: true });
  let total = 0;
  for (const [name, entry] of Object.entries(files)) {
    const buf = entries ? entries.get(prefix + entry) : unzipEntry(archive, prefix + entry);
    if (!buf) throw new Error(`${asset}: ${prefix + entry} not found`);
    writeFileSync(join(staging, name), buf);
    chmodSync(join(staging, name), 0o755);
    total += buf.length;
  }
  for (const [name, path] of Object.entries(repoFiles)) {
    writeFileSync(join(staging, name), readFileSync(new URL(path, root)));
  }
  writeFileSync(join(staging, "target.txt"), `${target}\n`);
  rmSync(llamaDir, { recursive: true, force: true });
  renameSync(staging, llamaDir);
  console.log(`installed ${llamaDir} (${Object.keys(files).length} files, ${total} bytes)`);
}

function install(name, buf) {
  const dest = sidecar(name);
  mkdirSync(binDir, { recursive: true });
  writeFileSync(`${dest}.part`, buf);
  chmodSync(`${dest}.part`, 0o755);
  renameSync(`${dest}.part`, dest);
  console.log(`installed ${dest} (${buf.length} bytes)`);
}

// --test-model <dir>: only fetch the test GGUF, checked by size, sha256 and its GGUF magic.
const testModelAt = args.indexOf("--test-model");
if (testModelAt >= 0) {
  const { url, file, size, sha256: want } = manifest.testModel;
  const dest = resolve(args[testModelAt + 1], file);
  const cached = join(cacheDir, `test-model-${want}`);
  let buf = existsSync(cached) ? readFileSync(cached) : null;
  if (!buf || sha256(buf) !== want) {
    console.log(`downloading ${url}`);
    const res = await fetch(url);
    if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
    buf = Buffer.from(await res.arrayBuffer());
  }
  if (buf.length !== size) throw new Error(`${file}: ${buf.length} bytes, want ${size}`);
  const got = sha256(buf);
  if (got !== want) throw new Error(`${file}: sha256 mismatch, got ${got}, want ${want}`);
  if (buf.subarray(0, 4).toString("latin1") !== "GGUF") throw new Error(`${file}: not a GGUF file`);
  mkdirSync(cacheDir, { recursive: true });
  writeFileSync(cached, buf);
  mkdirSync(resolve(args[testModelAt + 1]), { recursive: true });
  writeFileSync(dest, buf);
  console.log(`installed ${dest} (${buf.length} bytes)`);
  process.exit(0);
}

// --check-llama <dir>: only check an installed llama/ folder (CI runs it on the copy
// tauri-build puts beside the dev executable).
if (checkOnly >= 0) {
  checkLlama(args[checkOnly + 1]);
  process.exit(0);
}

install("yt-dlp", await download("ytdlp"));
install("deno", unzipEntry(await download("deno"), `deno${exe}`));
installLlama(await download("llama"));

if (args.includes("--check")) {
  const runnable = target === host || (host === "aarch64-apple-darwin" && target === "x86_64-apple-darwin");
  if (!runnable) {
    console.log(`--check skipped: ${target} binaries can't run on ${host}`);
  } else {
    for (const name of ["ffmpeg", "ffprobe"]) {
      if (!existsSync(sidecar(name))) throw new Error(`${sidecar(name)} missing; run scripts/build-ffmpeg.sh ${target}`);
    }
    // No URL: yt-dlp prints its debug header and exits with a usage error. No network needed.
    const run = spawnSync(sidecar("yt-dlp"), [
      "-v", "--ignore-config", "--no-update",
      "--ffmpeg-location", sidecar("ffmpeg"),
      "--no-js-runtimes", "--js-runtimes", `deno:${sidecar("deno")}`,
    ], { encoding: "utf8", timeout: 120_000 });
    // Without a URL yt-dlp exits 2 after printing the header; anything else (spawn error,
    // timeout, signal, other status) means the check didn't run cleanly.
    if (run.error || run.signal || run.status !== 2) {
      throw new Error(`yt-dlp check did not finish cleanly: ${run.error?.message ?? `status ${run.status}, signal ${run.signal}`}\n${run.stderr}`);
    }
    const out = `${run.stdout}\n${run.stderr}`;
    const want = [
      `[debug] yt-dlp version stable@${manifest.ytdlp.version} `,
      `[debug] exe versions: ffmpeg ${manifest.ffmpeg.version}, ffprobe ${manifest.ffmpeg.version}`,
      `[debug] JS runtimes: deno-${manifest.deno.version.replace(/^v/, "")}`,
    ];
    const missing = want.filter((s) => !out.includes(s));
    if (missing.length) {
      console.error(out);
      throw new Error(`yt-dlp debug header is missing: ${missing.join(", ")}`);
    }
    console.log(`check ok: ${want.join(", ")}`);
    checkLlama(llamaDir);
  }
}

// Runs llama-server --version and --list-devices from its own folder, as Queue will: proves
// the binary and every library it links load on this OS, and that the device listing runs.
// --list-devices shows accelerators only (Metal on macOS arm64; "(none)" on a CPU-only
// machine), and the CPU backend, a separate library on Windows and Linux, is only loaded with
// a model, so that is #30(b)'s test (first CI run, PR #95). Also run as
// `--check-llama <dir>` on the copy beside the dev executable.
function checkLlama(folder) {
  const dir = resolve(folder);
  const server = join(dir, `llama-server${exe}`);
  const recorded = readFileSync(join(dir, "target.txt"), "utf8").trim();
  if (recorded !== target) throw new Error(`${dir} holds ${recorded}, not ${target}; fetch again`);
  const run = (flag) => {
    const r = spawnSync(server, [flag], { cwd: dir, encoding: "utf8", timeout: 60_000 });
    if (r.error || r.signal || r.status !== 0) {
      throw new Error(`llama-server ${flag} failed: ${r.error?.message ?? `status ${r.status}, signal ${r.signal}`}\n${r.stdout ?? ""}\n${r.stderr ?? ""}`);
    }
    return `${r.stdout}\n${r.stderr}`;
  };
  const version = run("--version");
  const build = manifest.llama.version.replace(/^b/, "");
  if (!version.includes(build)) throw new Error(`llama-server --version doesn't say ${build}:\n${version}`);
  const devices = run("--list-devices");
  if (!devices.includes("Available devices")) {
    throw new Error(`llama-server --list-devices didn't list devices:\n${devices}`);
  }
  console.log(`check ok: llama-server ${build} in ${dir}`);
}
