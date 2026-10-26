// Independent NFR / #131 QA: run the actual CLI in disposable roots, never real gpg
// or upstream downloads. Fault injection includes partial files left by failed curl.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";

const project = new URL("../../", import.meta.url);
const pinned = JSON.parse(readFileSync(new URL("scripts/sidecars.json", project), "utf8"));
const names = ["SHA2-256SUMS", "SHA2-256SUMS.sig", pinned.ffmpeg.url.split("/").at(-1), `${pinned.ffmpeg.url.split("/").at(-1)}.asc`];

function fixture(t, options = {}, cliArgs = ["--verify-signatures"]) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "queue-131-QA 日本語 & ")));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  for (const dir of ["scripts/lib", "scripts/keys"]) mkdirSync(join(root, dir), { recursive: true });
  for (const file of ["scripts/fetch-sidecars.mjs", "scripts/lib/gpg.mjs", "scripts/lib/signature.mjs", pinned.ytdlp.signing.key, pinned.ffmpeg.signing.key]) {
    copyFileSync(new URL(file, project), join(root, file));
  }
  const manifest = structuredClone(pinned);
  const tarBytes = Buffer.from([0, 255, 128, 13, 10, 1, 0]);
  manifest.ffmpeg.sha256 = createHash("sha256").update(tarBytes).digest("hex");
  if (options.oddUrl) {
    // A shell must never interpret these URL characters, including a title-like Unicode value.
    manifest.ytdlp.url += "?title=日本語&literal=$(nope);`nope`";
  }
  writeFileSync(join(root, "scripts/sidecars.json"), JSON.stringify(manifest));
  writeFileSync(join(root, "options.json"), JSON.stringify(options));
  writeFileSync(join(root, "offline.mjs"), `
import cp from 'node:child_process';
import { syncBuiltinESMExports } from 'node:module';
import { appendFileSync, readFileSync, writeFileSync } from 'node:fs';
const root = new URL('./', import.meta.url);
const manifest = JSON.parse(readFileSync(new URL('scripts/sidecars.json', root)));
const options = JSON.parse(readFileSync(new URL('options.json', root)));
const log = event => appendFileSync(new URL('events.jsonl', root), JSON.stringify(event) + '\\n');
globalThis.fetch = async url => { log({ kind: 'node-fetch', url }); throw new Error('normal fetch stopped offline'); };
cp.spawnSync = (file, args, spawnOptions = {}) => {
  log({ file, args, spawnOptions });
  if (file === 'curl') {
    const src = args.at(-1);
    const name = new URL(src).pathname.split('/').at(-1);
    const dest = args[args.indexOf('--output') + 1];
    if (name === options.failAt) {
      writeFileSync(dest, 'partial, unauthenticated response');
      if (options.spawnCode) return { error: Object.assign(new Error('fixture spawn denied'), { code: options.spawnCode }) };
      return { status: options.status ?? 28, stderr: options.stderr ?? 'curl: (28) fixture TLS connect timeout', stdout: '' };
    }
    const body = name === 'SHA2-256SUMS'
      ? Object.values(manifest.ytdlp.assets).map(a => a.sha256 + '  ' + a.asset).join('\\n')
      : name.endsWith('.sig') || name.endsWith('.asc') ? Buffer.from([0, 255, 128])
      : Buffer.from([0, 255, 128, 13, 10, 1, 0]);
    writeFileSync(dest, body);
    return { status: 0, stdout: '', stderr: '' };
  }
  if (file !== 'gpg') throw new Error('unexpected executable ' + file);
  if (args[0] === '--version') return { status: 0, stdout: 'fixture', stderr: '' };
  const ffmpeg = args.at(-1).endsWith('ffmpeg-release.asc') || args.at(-1).endsWith('${names[2]}');
  const key = (ffmpeg ? manifest.ffmpeg : manifest.ytdlp).signing.fingerprint;
  if (args.includes('--import')) return { status: 0, stdout: '[GNUPG:] IMPORT_OK 1 ' + key + '\\n', stderr: '' };
  return { status: 0, stdout: '[GNUPG:] VALIDSIG ' + key + ' 2026-08-19 1787097600 0 4 0 1 10 00 ' + key + '\\n', stderr: '' };
};
syncBuiltinESMExports();
`);
  const result = spawnSync(process.execPath, ["--import", pathToFileURL(join(root, "offline.mjs")).href, join(root, "scripts/fetch-sidecars.mjs"), ...cliArgs], {
    encoding: "utf8", timeout: 10000,
    env: { ...process.env, TMPDIR: root, TMP: root, TEMP: root },
  });
  assert.equal(result.error, undefined, result.error?.message);
  const eventsPath = join(root, "events.jsonl");
  const events = existsSync(eventsPath) ? readFileSync(eventsPath, "utf8").trim().split("\n").map(JSON.parse) : [];
  assert.deepEqual(readdirSync(root).filter(n => n.startsWith("queue-gpg-")), [], "all temporary downloads and gpg homes are cleaned up");
  assert.equal(existsSync(join(root, "src-tauri/binaries")), false, "verification must not install or execute sidecars");
  return { result, events, manifest };
}

for (const [index, name] of names.entries()) {
  test(`NFR/#131 QA: timeout on ${name} discards partial bytes and stops immediately`, t => {
    const { result, events } = fixture(t, { failAt: name });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /curl: \(28\) fixture TLS connect timeout/);
    assert.ok(result.stderr.includes(name), "diagnostic identifies the failing URL");
    assert.equal(events.filter(e => e.file === "curl").length, index + 1, "no later requests after download failure");
    assert.equal(events.filter(e => e.file === "gpg" && e.args.includes("--verify")).length, index < 2 ? 0 : 1, "partial bytes never reach signature verification");
    assert.equal(events.some(e => e.kind === "node-fetch"), false);
  });
}

for (const [name, options, diagnostic] of [
  ["permission denied", { spawnCode: "EACCES" }, /curl: fixture spawn denied/],
  ["silent nonzero exit", { status: 7, stderr: "" }, /curl exited with 7/],
  ["certificate validation failure", { status: 60, stderr: "curl: (60) SSL certificate problem: fixture issuer" }, /curl: \(60\) SSL certificate problem: fixture issuer/],
  ["HTTP failure", { status: 22, stderr: "curl: (22) The requested URL returned error: 503" }, /curl: \(22\).*503/],
]) {
  test(`NFR/#131 QA: ${name} keeps curl's reason and cleans its partial file`, t => {
    const { result, events } = fixture(t, { failAt: names[2], ...options });
    assert.equal(result.status, 1);
    assert.match(result.stderr, diagnostic);
    assert.ok(result.stderr.includes(pinned.ffmpeg.url));
    assert.equal(events.filter(e => e.file === "curl").length, 3);
    assert.equal(events.filter(e => e.file === "gpg" && e.args.includes("--verify")).length, 1);
  });
}

test("NFR/#131 QA: shell metacharacters and Unicode URL data remain one curl argument", t => {
  const { result, events, manifest } = fixture(t, { oddUrl: true });
  assert.equal(result.status, 0, result.stderr);
  const downloads = events.filter(e => e.file === "curl");
  assert.equal(downloads.length, 4);
  for (const [index, { args, spawnOptions }] of downloads.entries()) {
    const expected = index < 2
      ? manifest.ytdlp.url.replace("{version}", manifest.ytdlp.version).replace("{asset}", names[index])
      : `${manifest.ffmpeg.url}${index === 3 ? ".asc" : ""}`;
    assert.equal(args.at(-1), expected);
    assert.ok(Array.isArray(args));
    assert.ok(!spawnOptions.shell);
  }
  assert.equal(events.some(e => e.kind === "node-fetch"), false);
});

for (const cliArgs of [[], ["--check"], ["--test-model", "model"], ["--check-llama", "llama"]]) {
  test(`NFR/#131 QA: ${cliArgs[0] ?? "normal fetch"} never invokes curl or gpg`, t => {
    const { result, events } = fixture(t, {}, cliArgs);
    assert.equal(result.status, 1, "fixture intentionally has no real sidecars/network");
    assert.equal(events.some(e => e.file === "curl" || e.file === "gpg"), false);
  });
}
