// Independent NFR/#44 / D-035 QA. Exercise the unchanged CLI in disposable roots.
// gpg and fetch are fault-injected, so these are control-flow tests, NOT cryptographic proof.
// The real gpg success path belongs to Linux CI.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";
import { parseStatus, parseSums, unsignedAssets } from "../../scripts/lib/signature.mjs";

const project = new URL("../../", import.meta.url);
const pinned = JSON.parse(readFileSync(new URL("scripts/sidecars.json", project), "utf8"));
const fingerprint = pinned.ytdlp.signing.fingerprint;
const other = "1".repeat(40);
const validStatus = (signer = other, primary = fingerprint) =>
  `[GNUPG:] VALIDSIG ${signer} 2026-08-19 1787097600 0 4 0 1 10 00 ${primary}\n`;
const ffmpegFingerprint = pinned.ffmpeg.signing.fingerprint;
const tarball = pinned.ffmpeg.url.split("/").pop();
const tarballBytes = "ffmpeg source tarball fixture";
const signedSums = Object.values(pinned.ytdlp.assets).map(({ asset, sha256 }) => `${sha256}  ${asset}`).join("\r\n");

test("NFR/#44 QA: committed v4 public-key packet hashes to the pinned primary fingerprint", () => {
  const armor = readFileSync(new URL(pinned.ytdlp.signing.key, project), "utf8");
  const packet = Buffer.from(armor.split(/\r?\n/).filter(line => line && !line.startsWith("-----") && !line.startsWith("=")).join(""), "base64");
  // This committed key uses an old-format tag-6 header with a two-byte length.
  // Its v4 fingerprint is SHA-1(0x99 || two-byte length || primary public-key body).
  assert.equal(packet[0], 0x99);
  assert.equal(packet[3], 4);
  const end = 3 + packet.readUInt16BE(1);
  assert.ok(end <= packet.length);
  assert.equal(createHash("sha1").update(packet.subarray(0, end)).digest("hex").toUpperCase(), fingerprint);
});

function runFixture(t, options = {}, args = ["--verify-signatures"]) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "queue-44-QA 日本語 & ")));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, "scripts/lib"), { recursive: true });
  mkdirSync(join(root, "scripts/keys"));
  for (const file of ["scripts/fetch-sidecars.mjs", "scripts/lib/signature.mjs", "scripts/lib/gpg.mjs", "scripts/keys/yt-dlp.asc", "scripts/keys/ffmpeg-release.asc"]) {
    copyFileSync(new URL(file, project), join(root, file));
  }
  const manifest = structuredClone(pinned);
  if (options.conflictingPin) manifest.ytdlp.assets["x86_64-apple-darwin"].sha256 = "0".repeat(64);
  if (options.missingSigning) delete manifest.ytdlp.signing;
  if (options.missingFfmpegSigning) delete manifest.ffmpeg.signing;
  // The fixture tarball is the pinned one unless a test says otherwise (#86).
  manifest.ffmpeg.sha256 = options.ffmpegPin ?? createHash("sha256").update(options.tarballBytes ?? tarballBytes).digest("hex");
  writeFileSync(join(root, "scripts/sidecars.json"), JSON.stringify(manifest));
  const config = {
    sums: signedSums, verifyStatus: 0, verifyOutput: validStatus(),
    importOutput: `[GNUPG:] IMPORT_OK 1 ${fingerprint}\n`,
    ffmpegImportOutput: `[GNUPG:] IMPORT_OK 1 ${ffmpegFingerprint}\n`,
    ffmpegVerifyStatus: 0, ffmpegVerifyOutput: validStatus("5F2EDE9A44501BB559871F8650EE8DF19C3345A2", ffmpegFingerprint),
    tarball, tarballBytes, ...options,
  };
  writeFileSync(join(root, "options.json"), JSON.stringify(config));
  const preload = join(root, "offline.mjs");
  writeFileSync(preload, `
import cp from 'node:child_process';
import { syncBuiltinESMExports } from 'node:module';
import { appendFileSync, readFileSync, existsSync } from 'node:fs';
const options = JSON.parse(readFileSync(new URL('./options.json', import.meta.url)));
const log = event => appendFileSync(new URL('./events', import.meta.url), JSON.stringify(event) + '\\n');
globalThis.fetch = async url => {
  log({kind: 'fetch', url});
  const name = new URL(url).pathname.split('/').at(-1);
  if (!['SHA2-256SUMS', 'SHA2-256SUMS.sig', options.tarball, options.tarball + '.asc'].includes(name)) throw new Error('offline stop before asset download');
  if (options.httpFailure === name) return {ok: false, status: 503};
  if (options.fetchTimeout === name) throw Object.assign(new Error('fixture download timeout'), {name: 'TimeoutError'});
  const body = name === options.tarball ? options.tarballBytes
    : name.endsWith('.sig') || name.endsWith('.asc') ? 'detached signature fixture' : options.sums;
  return {ok: true, arrayBuffer: async () => Buffer.from(body)};
};
cp.spawnSync = (file, args, spawnOptions = {}) => {
  if (file !== 'gpg') throw new Error('unexpected executable ' + file);
  const home = args[args.indexOf('--homedir') + 1];
  log({kind: 'gpg', file, args, spawnOptions, homeExists: existsSync(home)});
  if (options.missingGpg) return {error: Object.assign(new Error('not found'), {code: 'ENOENT'})};
  if (args[0] === '--version') return {status: 0, stdout: 'gpg fixture', stderr: ''};
  const ffmpeg = args.at(-1).endsWith('ffmpeg-release.asc') || args.at(-1).endsWith(options.tarball);
  if (args.includes('--import')) return {status: 0, stdout: ffmpeg ? options.ffmpegImportOutput : options.importOutput, stderr: ''};
  if (args.includes('--verify')) {
    if (options.verifySpawnError) return {error: Object.assign(new Error('fixture spawn error'), {code: 'EACCES'})};
    if (ffmpeg && options.ffmpegVerifyTimeout) return {status: null, error: Object.assign(new Error('fixture gpg timeout'), {code: 'ETIMEDOUT'})};
    if (ffmpeg) return {status: options.ffmpegVerifyStatus, stdout: options.ffmpegVerifyOutput, stderr: 'fixture gpg diagnostic'};
    return {status: options.verifyStatus, stdout: options.verifyOutput, stderr: 'fixture gpg diagnostic'};
  }
  throw new Error('unexpected gpg arguments');
};
syncBuiltinESMExports();
`);
  const result = spawnSync(process.execPath, ["--import", pathToFileURL(preload).href, join(root, "scripts/fetch-sidecars.mjs"), ...args], {
    encoding: "utf8", timeout: 10000,
    env: { ...process.env, TMPDIR: root, TMP: root, TEMP: root, GNUPGHOME: join(root, "must-not-use-user-home") },
  });
  assert.equal(result.error, undefined, result.error?.message);
  const eventsPath = join(root, "events");
  const events = existsSync(eventsPath) ? readFileSync(eventsPath, "utf8").trim().split("\n").map(JSON.parse) : [];
  const isolatedCalls = events.filter(e => e.kind === "gpg" && e.args.includes("--homedir"));
  for (const call of isolatedCalls) {
    const home = call.args[call.args.indexOf("--homedir") + 1];
    assert.equal(call.homeExists, true, "isolated home exists during gpg invocation");
    assert.equal(existsSync(home), false, "isolated home is removed on success and failure");
    assert.notEqual(home, join(root, "must-not-use-user-home"));
    assert.ok(call.args.includes("--batch") && call.args.includes("--no-tty"));
    assert.equal(call.spawnOptions.shell, undefined, "untrusted paths are argv, never shell input");
  }
  assert.deepEqual(readdirSync(root).filter(name => name.startsWith("queue-gpg-")), [],
    "both homes are removed, including failures before gpg starts");
  return { root, result, events, isolatedCalls };
}

test("NFR/#44 QA: mocked valid subkey signature checks all platform sums in an isolated home", t => {
  const f = runFixture(t);
  assert.equal(f.result.status, 0, f.result.stderr);
  assert.match(f.result.stdout, new RegExp(`VALIDSIG ${fingerprint}`));
  assert.equal(f.events[0].kind, "gpg");
  assert.match(f.result.stdout, new RegExp(`VALIDSIG ${ffmpegFingerprint}: ffmpeg`));
  assert.deepEqual(f.events.filter(e => e.kind === "fetch").map(e => e.url), [
    ...["SHA2-256SUMS", "SHA2-256SUMS.sig"].map(name => pinned.ytdlp.url.replace("{version}", pinned.ytdlp.version).replace("{asset}", name)),
    pinned.ffmpeg.url, `${pinned.ffmpeg.url}.asc`,
  ]);
  assert.equal(f.isolatedCalls.length, 4);
  const [imported, verified, ffImported, ffVerified] = f.isolatedCalls;
  const ffHome = ffImported.args[ffImported.args.indexOf("--homedir") + 1];
  assert.notEqual(ffHome, imported.args[imported.args.indexOf("--homedir") + 1], "each key uses a separate home");
  assert.equal(ffImported.args.at(-1), join(f.root, pinned.ffmpeg.signing.key));
  assert.deepEqual(ffVerified.args.slice(-3), ["--verify", join(ffHome, `${tarball}.asc`), join(ffHome, tarball)]);
  assert.equal(imported.args.at(-1), join(f.root, pinned.ytdlp.signing.key));
  const home = imported.args[imported.args.indexOf("--homedir") + 1];
  assert.equal(verified.args[verified.args.indexOf("--homedir") + 1], home);
  assert.deepEqual(verified.args.slice(-3), ["--verify", join(home, "SHA2-256SUMS.sig"), join(home, "SHA2-256SUMS")]);
  assert.equal(existsSync(join(f.root, "src-tauri/binaries")), false);
});

for (const [name, options, error] of [
  ["nonzero gpg exit despite matching VALIDSIG", { verifyStatus: 1 }, /no valid signature/],
  ["gpg killed after status output", { verifyStatus: null }, /no valid signature/],
  ["zero exit with no VALIDSIG", { verifyOutput: `[GNUPG:] GOODSIG ${fingerprint} yt-dlp\n` }, /no valid signature/],
  ["pinned signer but wrong primary fingerprint", { verifyOutput: validStatus(fingerprint, other) }, /no valid signature/],
  ["human-readable good-signature diagnostic", { verifyOutput: `gpg: Good signature from ${fingerprint}` }, /no valid signature/],
  ["short malformed VALIDSIG", { verifyOutput: `[GNUPG:] VALIDSIG ${fingerprint}` }, /no valid signature/],
  ["wrong imported key", { importOutput: `[GNUPG:] IMPORT_OK 1 ${other}\n` }, /committed key imported/],
  ["extra imported primary key", { importOutput: `[GNUPG:] IMPORT_OK 1 ${fingerprint}\n[GNUPG:] IMPORT_OK 1 ${other}\n` }, /committed key imported/],
  ["spawn failure at verification", { verifySpawnError: true }, /fixture spawn error/],
  ["checksum download HTTP error", { httpFailure: "SHA2-256SUMS" }, /HTTP 503/],
  ["signature download HTTP error", { httpFailure: "SHA2-256SUMS.sig" }, /HTTP 503/],
  ["unsigned asset list", { sums: "" }, /not in the signed sums/],
  ["all signed hashes paired with wrong names", { sums: signedSums.replaceAll("yt-dlp", "wrong-name") }, /not in the signed sums/],
  ["correct names but incorrect hashes", { sums: signedSums.replace(/[0-9a-f]{64}/g, "0".repeat(64)) }, /signed sha256/],
]) {
  test(`NFR/#44 QA: rejects ${name}`, t => {
    const f = runFixture(t, options);
    assert.equal(f.result.status, 1, f.result.stderr);
    assert.match(f.result.stderr, error);
    assert.doesNotMatch(f.result.stdout, /VALIDSIG/);
  });
}

test("NFR/#44 QA: missing gpg fails before either network request", t => {
  const f = runFixture(t, { missingGpg: true });
  assert.equal(f.result.status, 1);
  assert.match(f.result.stderr, /gpg is required/);
  assert.equal(f.events.length, 1);
  assert.deepEqual(f.events[0].args, ["--version"]);
});

test("NFR/#44 QA: missing signing configuration fails before download or gpg", t => {
  const f = runFixture(t, { missingSigning: true });
  assert.equal(f.result.status, 1);
  assert.match(f.result.stderr, /no ytdlp.signing/);
  assert.deepEqual(f.events, []);
});

for (const args of [[], ["--check"], ["--test-model", "model"], ["--check-llama", "llama"]]) {
  test(`NFR/#44 QA: ${args[0] ?? "normal fetch"} does not invoke gpg`, t => {
    const f = runFixture(t, { missingGpg: true }, args);
    // Deliberately stop at an unavailable fixture file or blocked download: no real binaries/network.
    assert.equal(f.result.status, 1);
    assert.equal(f.events.some(e => e.kind === "gpg"), false, f.result.stderr);
  });
}

test("NFR/#44 QA: malformed checksum and status lines cannot masquerade as pinned entries", () => {
  const hash = "a".repeat(64);
  const sums = parseSums(`junk ${hash}  yt-dlp_linux\n${hash.slice(1)}  yt-dlp_linux\n${hash}  yt-dlp_linux.exe\n${hash}  ../yt-dlp_linux\n`);
  assert.deepEqual(unsignedAssets(sums, { target: { asset: "yt-dlp_linux", sha256: hash } }), ["yt-dlp_linux: not in the signed sums"]);
  assert.deepEqual(parseStatus(`gpg: ${validStatus()}\n[GNUPG:] VALIDSIG short`), { imported: [], valid: [] });
});

test("NFR/#44 QA: shared asset must match every platform's pinned sha256", () => {
  const assets = {
    arm64: { asset: "yt-dlp_macos", sha256: "a".repeat(64) },
    x64: { asset: "yt-dlp_macos", sha256: "b".repeat(64) },
  };
  assert.equal(unsignedAssets(parseSums(`${"a".repeat(64)}  yt-dlp_macos`), assets).length, 1,
    "the x64 pin has no matching signed line and must be rejected");
});

test("NFR/#44 QA: CLI rejects a conflicting second platform pin for a shared asset", t => {
  const f = runFixture(t, { conflictingPin: true });
  assert.equal(f.result.status, 1, "every platform pin must be checked, including the second macOS entry");
  assert.match(f.result.stderr, /pinned assets don't match/);
});

for (const [name, options, error, gpgCalls] of [
  ["an ffmpeg tarball that isn't the pinned one, before gpg sees it", { ffmpegPin: "0".repeat(64) }, /sha256 mismatch/, 2],
  ["an ffmpeg signature gpg rejects", { ffmpegVerifyStatus: 1 }, /no valid signature by FCF986EA/, 4],
  ["an ffmpeg signature by another key", { ffmpegVerifyOutput: validStatus(other, other) }, /no valid signature by FCF986EA/, 4],
  ["a wrong ffmpeg key import", { ffmpegImportOutput: `[GNUPG:] IMPORT_OK 1 ${other}\n` }, /committed key imported/, 3],
  ["an ffmpeg .asc download error", { httpFailure: `${tarball}.asc` }, /HTTP 503/, 2],
  ["missing ffmpeg signing configuration", { missingFfmpegSigning: true }, /no ffmpeg.signing/, 0],
  ["an ffmpeg tarball HTTP error", { httpFailure: tarball }, /HTTP 503/, 2],
  ["an ffmpeg tarball download timeout", { fetchTimeout: tarball }, /fixture download timeout/, 2],
  ["an ffmpeg signature download timeout", { fetchTimeout: `${tarball}.asc` }, /fixture download timeout/, 2],
  ["an ffmpeg gpg subprocess timeout", { ffmpegVerifyTimeout: true }, /fixture gpg timeout/, 4],
  ["a missing ffmpeg import status", { ffmpegImportOutput: "" }, /committed key imported/, 3],
  ["two imported ffmpeg keys", { ffmpegImportOutput: `[GNUPG:] IMPORT_OK 1 ${ffmpegFingerprint}\n[GNUPG:] IMPORT_OK 1 ${other}\n` }, /committed key imported/, 3],
  ["an ffmpeg GOODSIG without VALIDSIG", { ffmpegVerifyOutput: `[GNUPG:] GOODSIG B4322F04D67658D8 FFmpeg\n` }, /no valid signature by FCF986EA/, 4],
  ["an ffmpeg malformed VALIDSIG", { ffmpegVerifyOutput: `[GNUPG:] VALIDSIG ${ffmpegFingerprint}\n` }, /no valid signature by FCF986EA/, 4],
  ["a forged ffmpeg diagnostic", { ffmpegVerifyOutput: `gpg: ${validStatus(other, ffmpegFingerprint)}` }, /no valid signature by FCF986EA/, 4],
  ["a matching ffmpeg signer with the wrong primary key", { ffmpegVerifyOutput: validStatus(ffmpegFingerprint, other) }, /no valid signature by FCF986EA/, 4],
]) {
  test(`NFR/#86 QA: rejects ${name}`, t => {
    const f = runFixture(t, options);
    assert.equal(f.result.status, 1, f.result.stderr);
    assert.match(f.result.stderr, error);
    assert.doesNotMatch(f.result.stdout, /VALIDSIG [0-9A-F]+: ffmpeg/);
    assert.equal(f.isolatedCalls.length, gpgCalls);
  });
}

test("NFR/#86 QA: a hash mismatch stops before downloading the detached signature", t => {
  const f = runFixture(t, { ffmpegPin: "0".repeat(64) });
  assert.equal(f.result.status, 1);
  assert.match(f.result.stderr, /sha256 mismatch/);
  assert.equal(f.events.some(e => e.kind === "fetch" && e.url === `${pinned.ffmpeg.url}.asc`), false);
});

test("NFR/#86 QA: yt-dlp failure stops before fetching any ffmpeg input", t => {
  const f = runFixture(t, { verifyStatus: 1 });
  assert.equal(f.result.status, 1);
  assert.match(f.result.stderr, new RegExp(`no valid signature by ${fingerprint}`));
  assert.equal(f.events.filter(e => e.kind === "fetch").length, 2);
  assert.equal(f.events.some(e => e.kind === "fetch" && e.url.startsWith(pinned.ffmpeg.url)), false);
});

test("NFR/#86 QA: hashes binary tarball bytes and accepts a signing subkey under the pinned primary", t => {
  const f = runFixture(t, { tarballBytes: "\u0000\u0001binary source\r\n日本語\u00ff\u0000" });
  assert.equal(f.result.status, 0, f.result.stderr);
  assert.match(f.result.stdout, new RegExp(`VALIDSIG ${ffmpegFingerprint}: ffmpeg`));
});
