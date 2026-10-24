// #44: yt-dlp's signed checksums (D-035). The gpg run itself is CI's (Linux); these test the
// parts that don't need gpg, the manifest's signing entry, and the failure without gpg.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { parseStatus, parseSums, unsignedAssets } from "../../scripts/lib/signature.mjs";

const root = new URL("../../", import.meta.url);
const manifest = JSON.parse(readFileSync(new URL("scripts/sidecars.json", root), "utf8"));
const A = "a".repeat(64);
const B = "b".repeat(64);

test("#44: the sums parser reads name and hash, with or without the binary-mode star", () => {
  const sums = parseSums(`${A}  yt-dlp_linux\r\n${B} *yt-dlp.exe\nnot a line\n${"C".repeat(64)}  yt-dlp_macos\n`);
  assert.deepEqual([...sums], [["yt-dlp_linux", A], ["yt-dlp.exe", B], ["yt-dlp_macos", "c".repeat(64)]]);
});

test("#44: every pinned asset must be paired with its own hash on one signed line", () => {
  const sums = parseSums(`${A}  yt-dlp_linux\n${B}  yt-dlp_macos\n`);
  const assets = {
    one: { asset: "yt-dlp_linux", sha256: A },
    two: { asset: "yt-dlp_macos", sha256: A }, // the hash is in the file, but on another name
    three: { asset: "yt-dlp_macos", sha256: A }, // the same pin twice is reported once
    four: { asset: "yt-dlp.exe", sha256: B },
  };
  assert.deepEqual(unsignedAssets(sums, assets), [
    `yt-dlp_macos: signed sha256 ${B}, pinned ${A}`,
    "yt-dlp.exe: not in the signed sums",
  ]);
  assert.deepEqual(unsignedAssets(sums, { one: { asset: "yt-dlp_linux", sha256: A.toUpperCase() } }), []);
});

test("#44: gpg's status lines give the imported keys and the signing primary key", () => {
  const fpr = "AC0CBBE6848D6A873464AF4E57CF65933B5A7581";
  const sub = "1111111111111111111111111111111111111111";
  const status = [
    `[GNUPG:] IMPORT_OK 1 ${fpr}`,
    `[GNUPG:] IMPORT_OK 0 ${fpr}`,
    "[GNUPG:] IMPORT_RES 1 0 1 0 0 0 0 0 0 0 0 0 0 0 0",
    "[GNUPG:] GOODSIG 57CF65933B5A7581 yt-dlp",
    `[GNUPG:] VALIDSIG ${sub} 2026-08-19 1755600000 0 4 0 1 10 00 ${fpr}`,
    "gpg: Good signature from a forged comment line",
  ].join("\n");
  assert.deepEqual(parseStatus(status), { imported: [fpr], valid: [fpr] });
  assert.deepEqual(parseStatus("gpg: Good signature\n[GNUPG:] VALIDSIG short"), { imported: [], valid: [] });
});

test("#44: the manifest pins the committed key, its fingerprint and its source", () => {
  const { signing } = manifest.ytdlp;
  assert.equal(signing.key, "scripts/keys/yt-dlp.asc");
  assert.match(signing.fingerprint, /^[0-9A-F]{40}$/);
  assert.match(signing.source, /^https:\/\/github\.com\/yt-dlp\/yt-dlp\/blob\/[0-9a-f]{40}\/public\.key$/);
  const key = readFileSync(new URL(signing.key, root), "utf8");
  assert.match(key, /^-----BEGIN PGP PUBLIC KEY BLOCK-----\n[\s\S]+\n-----END PGP PUBLIC KEY BLOCK-----\n?$/);
});

test("#44: without gpg, the mode fails loudly before downloading anything", () => {
  const run = spawnSync(process.execPath, [fileURLToPath(new URL("scripts/fetch-sidecars.mjs", root)), "--verify-signatures"], {
    encoding: "utf8",
    env: { PATH: "/nonexistent" },
    timeout: 20000,
  });
  assert.equal(run.status, 1, run.stderr);
  assert.match(run.stderr, /gpg is required for this mode/);
  assert.doesNotMatch(run.stdout, /downloading/);
});
