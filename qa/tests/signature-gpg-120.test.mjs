// #120 (D-035): the shared signature check with real gpg, on committed fixtures for the pinned
// yt-dlp release: yt-dlp's own SHA2-256SUMS and its .sig, untouched (see .gitattributes). The
// gpg cases run wherever gpg is on PATH (Linux and macOS CI) and skip elsewhere (Windows CI, a
// Mac without gpg). A throwaway key is generated at test time; no private key is committed and
// nothing uses the network.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { verifyDetached } from "../../scripts/lib/gpg.mjs";
import { parseSums, unsignedAssets } from "../../scripts/lib/signature.mjs";

const root = new URL("../../", import.meta.url);
const manifest = JSON.parse(readFileSync(new URL("scripts/sidecars.json", root), "utf8"));
const { version, assets, signing } = manifest.ytdlp;
const fixtures = fileURLToPath(new URL(`qa/fixtures/signatures/yt-dlp-${version}/`, root));
const keyFile = fileURLToPath(new URL(signing.key, root));
const sums = join(fixtures, "SHA2-256SUMS");
const sig = join(fixtures, "SHA2-256SUMS.sig");

// Runnable means it starts and exits 0; a gpg that's present but broken skips too (#120 QA).
const probe = spawnSync("gpg", ["--version"]);
const hasGpg = !probe.error && probe.status === 0;
const skip = hasGpg ? false : "gpg isn't on PATH (CI runs these on Linux and macOS)";

// A fresh gpg home for one case, removed with its agent afterwards.
function home(t) {
  const dir = mkdtempSync(join(tmpdir(), "queue-gpg120-"));
  t.after(() => {
    spawnSync("gpgconf", ["--homedir", dir, "--kill", "gpg-agent"]);
    rmSync(dir, { recursive: true, force: true });
  });
  return dir;
}

// A copy of `file` in `dir` with one byte flipped at `at` (negative counts from the end).
function flipped(file, dir, at) {
  const bytes = readFileSync(file);
  const i = at < 0 ? bytes.length + at : at;
  bytes[i] ^= 0x01;
  const out = join(dir, `flipped-${file.split(/[\\/]/).pop()}`);
  writeFileSync(out, bytes);
  return out;
}

// A throwaway signing key in its own home: its fingerprint, its armored public key, and a
// detached signature it made over `data`.
function throwaway(t, data) {
  const dir = home(t);
  const gpg = (...argv) => spawnSync("gpg", ["--homedir", dir, "--batch", "--pinentry-mode", "loopback", "--passphrase", "", ...argv], { encoding: "utf8" });
  const made = gpg("--quick-generate-key", "Queue QA throwaway <qa@example.invalid>", "ed25519", "sign", "1d");
  assert.equal(made.status, 0, made.stderr);
  const listed = gpg("--with-colons", "--list-keys");
  const fingerprint = listed.stdout.split("\n").find((line) => line.startsWith("fpr:")).split(":")[9];
  const publicKey = join(dir, "throwaway.asc");
  writeFileSync(publicKey, gpg("--armor", "--export", fingerprint).stdout);
  const signature = join(dir, "throwaway.sig");
  const signed = gpg("--detach-sign", "--output", signature, data);
  assert.equal(signed.status, 0, signed.stderr);
  return { fingerprint, publicKey, signature };
}

test("#120: the committed fixtures are the pinned release's signed sums (no gpg needed)", () => {
  const problems = unsignedAssets(parseSums(readFileSync(sums, "utf8")), assets);
  assert.deepEqual(problems, [], "a pin bump must refresh qa/fixtures/signatures too");
  assert.ok(readFileSync(sig).length > 0);
});

test("#120: the real signature over the real sums passes with the pinned key", { skip }, (t) => {
  assert.equal(verifyDetached({ home: home(t), keyFile, fingerprint: signing.fingerprint, dataFile: sums, sigFile: sig }), signing.fingerprint);
});

test("#120: one flipped byte in the sums fails", { skip }, (t) => {
  const dir = home(t);
  const dataFile = flipped(sums, dir, 0);
  assert.throws(() => verifyDetached({ home: dir, keyFile, fingerprint: signing.fingerprint, dataFile, sigFile: sig }), /no valid signature/);
});

test("#120: one flipped byte in the signature fails", { skip }, (t) => {
  const dir = home(t);
  const sigFile = flipped(sig, dir, -2);
  assert.throws(() => verifyDetached({ home: dir, keyFile, fingerprint: signing.fingerprint, dataFile: sums, sigFile }), /no valid signature/);
});

test("#120: a signature by another key fails against the pinned key", { skip }, (t) => {
  const other = throwaway(t, sums);
  assert.throws(
    () => verifyDetached({ home: home(t), keyFile, fingerprint: signing.fingerprint, dataFile: sums, sigFile: other.signature }),
    /no valid signature/,
  );
});

test("#120: another key in the committed key's place fails at import", { skip }, (t) => {
  const other = throwaway(t, sums);
  const dir = home(t);
  const swapped = join(dir, "swapped.asc");
  copyFileSync(other.publicKey, swapped);
  assert.throws(
    () => verifyDetached({ home: dir, keyFile: swapped, fingerprint: signing.fingerprint, dataFile: sums, sigFile: other.signature }),
    /committed key imported as \[[0-9A-F]{40}\], expected exactly/,
  );
});

test("#120: the other key's own fingerprint doesn't make the real signature its own", { skip }, (t) => {
  const other = throwaway(t, sums);
  assert.throws(
    () => verifyDetached({ home: home(t), keyFile: other.publicKey, fingerprint: other.fingerprint, dataFile: sums, sigFile: sig }),
    /no valid signature/,
  );
});
