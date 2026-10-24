// Independent NFR/#120 / D-035 QA. Offline checks of the submitted test runner and real gpg.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { verifyDetached } from "../../scripts/lib/gpg.mjs";

const root = new URL("../../", import.meta.url);
const { version, signing } = JSON.parse(readFileSync(new URL("scripts/sidecars.json", root))).ytdlp;
const keyFile = fileURLToPath(new URL(signing.key, root));
const fixtures = new URL(`qa/fixtures/signatures/yt-dlp-${version}/`, root);
const dataFile = fileURLToPath(new URL("SHA2-256SUMS", fixtures));
const sigFile = fileURLToPath(new URL("SHA2-256SUMS.sig", fixtures));
const probe = spawnSync("gpg", ["--version"], { timeout: 5000 });
const skip = probe.error || probe.status !== 0 ? "gpg is not runnable" : false;

function temp(t) {
  const dir = mkdtempSync(join(tmpdir(), "q120qa-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

function home(t) {
  const dir = mkdtempSync(join(tmpdir(), "q120qa-"));
  t.after(() => {
    spawnSync("gpgconf", ["--homedir", dir, "--kill", "gpg-agent"], { timeout: 5000 });
    rmSync(dir, { recursive: true, force: true });
  });
  return dir;
}

function gpg(dir, ...args) {
  const run = spawnSync("gpg", ["--homedir", dir, "--batch", "--no-tty", "--pinentry-mode", "loopback", "--passphrase", "", ...args], {
    encoding: "utf8", timeout: 10000,
  });
  assert.equal(run.error, undefined, run.error?.message);
  assert.equal(run.status, 0, run.stderr);
  return run.stdout;
}

// Probe failures must skip the six cryptographic cases, while the fixture/pin case still runs.
// Preloading only mocks process availability; it never substitutes cryptographic results.
for (const failure of ["ENOENT", "EACCES", "nonzero exit"]) {
  test(`NFR/#120 QA: the submitted suite skips gpg cases on ${failure}`, (t) => {
    const dir = temp(t);
    const preload = join(dir, "probe.mjs");
    writeFileSync(preload, `
import cp from "node:child_process";
import { syncBuiltinESMExports } from "node:module";
cp.spawnSync = (file, args) => {
  if (file !== "gpg" && file !== "gpgconf") throw new Error("unexpected executable " + file);
  if (file === "gpg" && args.length === 1 && args[0] === "--version") {
    return ${failure === "nonzero exit"
      ? '{ status: 127, stdout: "", stderr: "gpg: dependent library missing" }'
      : `{ error: Object.assign(new Error("unavailable"), { code: ${JSON.stringify(failure)} }), status: null }`};
  }
  throw new Error("unrunnable gpg must never be used after the probe");
};
syncBuiltinESMExports();
`);
    const env = { ...process.env };
    delete env.NODE_TEST_CONTEXT;
    const run = spawnSync(process.execPath, ["--import", pathToFileURL(preload).href, "--test", "--test-reporter=tap", "--test-timeout=10000",
      fileURLToPath(new URL("qa/tests/signature-gpg-120.test.mjs", root))], {
      encoding: "utf8", timeout: 15000, env,
    });
    assert.equal(run.error, undefined, run.error?.message);
    assert.equal(run.status, 0, `expected the gpg-free case to pass and all six gpg cases to skip\n${run.stdout}\n${run.stderr}`);
    assert.match(run.stdout, /# pass 1\b/);
    assert.match(run.stdout, /# fail 0\b/);
    assert.match(run.stdout, /# skipped 6\b/);
  });
}

test("NFR/#120 QA: real gpg accepts Unicode and shell metacharacters in input paths", { skip }, (t) => {
  const dir = temp(t);
  const paths = ["公钥 & $(no-command).asc", "校验 sums ; 'quoted'", "签名 & data.sig"].map(name => join(dir, name));
  [keyFile, dataFile, sigFile].forEach((file, i) => copyFileSync(file, paths[i]));
  assert.equal(verifyDetached({ home: home(t), keyFile: paths[0], dataFile: paths[1], sigFile: paths[2],
    fingerprint: signing.fingerprint.toLowerCase() }), signing.fingerprint);
});

for (const [name, field, bytes] of [
  ["empty sums", "dataFile", () => Buffer.alloc(0)],
  ["CRLF-converted sums", "dataFile", () => Buffer.from(readFileSync(dataFile, "utf8").replace(/\r?\n/g, "\r\n"))],
  ["empty signature", "sigFile", () => Buffer.alloc(0)],
  ["truncated signature", "sigFile", () => readFileSync(sigFile).subarray(0, 40)],
]) {
  test(`NFR/#120 QA: real gpg rejects ${name}`, { skip }, (t) => {
    const replacement = join(temp(t), "replacement");
    writeFileSync(replacement, bytes());
    assert.throws(() => verifyDetached({ home: home(t), keyFile, fingerprint: signing.fingerprint,
      dataFile, sigFile, [field]: replacement }), /no valid signature/);
  });
}

test("NFR/#120 QA: a valid signature from an already-known foreign key is rejected", { skip }, (t) => {
  const signer = home(t);
  gpg(signer, "--quick-generate-key", "Queue QA <qa@example.invalid>", "ed25519", "sign", "1d");
  const publicKey = join(signer, "public.asc");
  writeFileSync(publicKey, gpg(signer, "--armor", "--export"));
  const signature = join(signer, "foreign.sig");
  gpg(signer, "--detach-sign", "--output", signature, dataFile);
  const verifier = home(t);
  gpg(verifier, "--import", publicKey);
  // Prove gpg exits zero and emits a real VALIDSIG; rejection must check the primary key.
  assert.match(gpg(verifier, "--status-fd", "1", "--verify", signature, dataFile), /\[GNUPG:\] VALIDSIG /);
  assert.throws(() => verifyDetached({ home: verifier, keyFile, fingerprint: signing.fingerprint,
    dataFile, sigFile: signature }), /no valid signature.*gpg exit 0/);
});

test("NFR/#120 QA: a key file containing the pinned and a foreign key is rejected", { skip }, (t) => {
  const signer = home(t);
  gpg(signer, "--quick-generate-key", "Queue QA <qa@example.invalid>", "ed25519", "sign", "1d");
  const combined = join(signer, "combined.asc");
  writeFileSync(combined, readFileSync(keyFile, "utf8") + "\n" + gpg(signer, "--armor", "--export"));
  assert.throws(() => verifyDetached({ home: home(t), keyFile: combined, fingerprint: signing.fingerprint,
    dataFile, sigFile }), /committed key imported as \[.*\], expected exactly/);
});
