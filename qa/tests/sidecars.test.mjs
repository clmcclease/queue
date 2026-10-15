// #2 (D6), D-012: pinned sidecar manifest, Tauri wiring, and CI fetch contracts.
// Run: npm test
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

const root = new URL("../../", import.meta.url);
// Normalize CRLF so a Windows checkout made without .gitattributes still parses.
const read = (path) => readFileSync(new URL(path, root), "utf8").replaceAll("\r\n", "\n");
const manifest = JSON.parse(read("scripts/sidecars.json"));
const TARGETS = ["aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"];

test("every downloaded sidecar is pinned to an exact version and sha256 for all four targets", () => {
  for (const component of ["ytdlp", "deno"]) {
    const { version, url, assets } = manifest[component];
    assert.match(version, /^v?\d+\.\d+\.\d+$/, `${component} version must be exact`);
    assert.match(url, /^https:\/\/github\.com\/.+\/releases\/download\/\{version\}\/\{asset\}$/);
    assert.deepEqual(Object.keys(assets).sort(), [...TARGETS].sort());
    for (const [target, { asset, sha256 }] of Object.entries(assets)) {
      assert.match(sha256, /^[a-f0-9]{64}$/, `${component} ${target} sha256`);
      assert.ok(asset && !asset.includes("/"), `${component} ${target} asset`);
    }
  }
  assert.equal(manifest.ytdlp.assets["x86_64-pc-windows-msvc"].asset, "yt-dlp.exe", "64-bit Windows build, not yt-dlp_x86.exe");
  assert.match(manifest.ffmpeg.url, new RegExp(`^https://ffmpeg\\.org/releases/ffmpeg-${manifest.ffmpeg.version.replaceAll(".", "\\.")}\\.tar\\.xz$`));
  assert.match(manifest.ffmpeg.sha256, /^[a-f0-9]{64}$/);
});

test("Tauri externalBin matches the queue- names the Rust sidecar module resolves", () => {
  const tauri = JSON.parse(read("src-tauri/tauri.conf.json"));
  assert.deepEqual(tauri.bundle.externalBin, [
    "binaries/queue-yt-dlp", "binaries/queue-ffmpeg", "binaries/queue-ffprobe", "binaries/queue-deno",
  ]);
  const rust = read("src-tauri/src/sidecar.rs");
  for (const name of ["queue-yt-dlp", "queue-ffmpeg", "queue-deno"]) assert.ok(rust.includes(`"${name}"`), name);
  // yt-dlp derives ffprobe's path by replacing "ffmpeg" in the --ffmpeg-location file name.
  assert.equal("queue-ffmpeg".replace("ffmpeg", "ffprobe"), "queue-ffprobe");
});

test("the ffmpeg build is LGPL-only with networking off", () => {
  const script = read("scripts/build-ffmpeg.sh");
  for (const flag of ["--disable-everything", "--disable-autodetect", "--disable-network", "--enable-protocol=file,pipe"]) {
    assert.ok(script.includes(flag), flag);
  }
  assert.doesNotMatch(script, /--enable-(?:gpl|nonfree|version3|libx26[45]|openssl|gnutls)/);
  assert.match(script, /grep -q '\^License: LGPL'/, "the build must fail if configure does not report LGPL");
});

test("CI installs and checks sidecars before tests and bundles", () => {
  const workflow = read(".github/workflows/build.yml");
  assert.match(workflow, /^  ffmpeg:$/m);
  assert.match(workflow, /^  test:\n    needs: ffmpeg$/m);
  assert.match(workflow, /node scripts\/fetch-sidecars\.mjs --target \$\{\{ matrix\.target \}\} --check$/m);
  assert.match(workflow, /scripts\/build-ffmpeg\.sh "\$t"/);
});

test("build-ffmpeg.sh rejects unknown or path-traversal targets before touching the filesystem", { skip: process.platform === "win32" && "build-ffmpeg.sh runs on macOS/Linux only" }, (t) => {
  const parent = mkdtempSync(join(tmpdir(), "queue-ffmpeg-target-qa-"));
  t.after(() => rmSync(parent, { recursive: true, force: true }));
  const checkout = join(parent, "checkout");
  mkdirSync(join(checkout, "scripts"), { recursive: true });
  copyFileSync(new URL("scripts/build-ffmpeg.sh", root), join(checkout, "scripts", "build-ffmpeg.sh"));
  copyFileSync(new URL("scripts/sidecars.json", root), join(checkout, "scripts", "sidecars.json"));
  writeFileSync(join(checkout, "MARKER"), "keep");
  writeFileSync(join(parent, "MARKER"), "keep");
  // An existing build dir, so the old order would have resolved and deleted the traversal path.
  mkdirSync(join(checkout, "src-tauri", "binaries", ".cache", "ffmpeg-build-aarch64-apple-darwin"), { recursive: true });
  for (const target of ["aarch64-apple-darwin/../../../..", "x86_64-apple-darwin/../../..", "riscv64-unknown-linux-gnu", ""]) {
    const run = spawnSync("bash", [join(checkout, "scripts", "build-ffmpeg.sh"), target], { encoding: "utf8", timeout: 10000 });
    assert.equal(run.status, target ? 2 : 1, `${JSON.stringify(target)}: ${run.stderr}`);
    assert.ok(existsSync(join(checkout, "MARKER")) && existsSync(join(parent, "MARKER")), `${JSON.stringify(target)} deleted files`);
  }
  assert.ok(!existsSync(join(checkout, "src-tauri", "binaries", ".cache", "ffmpeg-9.0.2.tar.xz")), "no download before validation");
});
