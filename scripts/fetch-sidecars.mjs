#!/usr/bin/env node
// Fetch the pinned yt-dlp and deno sidecars into src-tauri/binaries (#2, D-012).
// ffmpeg/ffprobe are built from source separately: scripts/build-ffmpeg.sh.
//
//   node scripts/fetch-sidecars.mjs [--target <triple>] [--check]
//
// --target defaults to the host. --check runs yt-dlp's verbose header to confirm it sees our
// ffmpeg, ffprobe and deno (skipped when the target can't run on this host).
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { arch, platform } from "node:process";
import { fileURLToPath } from "node:url";
import { inflateRawSync } from "node:zlib";

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

function install(name, buf) {
  const dest = sidecar(name);
  mkdirSync(binDir, { recursive: true });
  writeFileSync(`${dest}.part`, buf);
  chmodSync(`${dest}.part`, 0o755);
  renameSync(`${dest}.part`, dest);
  console.log(`installed ${dest} (${buf.length} bytes)`);
}

install("yt-dlp", await download("ytdlp"));
install("deno", unzipEntry(await download("deno"), `deno${exe}`));

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
  }
}
