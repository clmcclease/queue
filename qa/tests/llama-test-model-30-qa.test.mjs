// N6/#30(b): run the production test-GGUF fetcher in disposable roots, offline.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";

const hash = (b) => createHash("sha256").update(b).digest("hex");
const bytes = Buffer.from("GGUF independent test fixture");
function fixture(t, { body = bytes, size = bytes.length, digest = hash(bytes), status = 200, cached } = {}) {
  const root = mkdtempSync(join(tmpdir(), "queue-30-test-gguf-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, "scripts"));
  copyFileSync(new URL("../../scripts/fetch-sidecars.mjs", import.meta.url), join(root, "scripts/fetch-sidecars.mjs"));
  mkdirSync(join(root, "scripts/lib"));
  copyFileSync(new URL("../../scripts/lib/signature.mjs", import.meta.url), join(root, "scripts/lib/signature.mjs"));
  const manifest = JSON.parse(readFileSync(new URL("../../scripts/sidecars.json", import.meta.url)));
  manifest.testModel = { url: "https://fixture.invalid/model.gguf", file: "test.gguf", size, sha256: digest };
  writeFileSync(join(root, "scripts/sidecars.json"), JSON.stringify(manifest));
  writeFileSync(join(root, "body"), body);
  const cache = join(root, "src-tauri/binaries/.cache");
  mkdirSync(cache, { recursive: true });
  if (cached) writeFileSync(join(cache, `test-model-${digest}`), cached);
  const preload = join(root, "offline.mjs");
  writeFileSync(preload, `
import { appendFileSync, readFileSync } from 'node:fs';
globalThis.fetch = async (url) => {
  appendFileSync(new URL('./requests', import.meta.url), url + '\\n');
  if (url !== 'https://fixture.invalid/model.gguf') throw new Error('unexpected network request');
  return { ok: ${status === 200}, status: ${status}, arrayBuffer: async () => readFileSync(new URL('./body', import.meta.url)) };
};`);
  const out = join(root, "test 日本語 & model");
  const result = spawnSync(process.execPath, ["--import", pathToFileURL(preload).href, join(root, "scripts/fetch-sidecars.mjs"), "--test-model", out], { encoding: "utf8", timeout: 10000 });
  return { root, cache, out, result };
}

test("N6/#30: test-model mode installs only checked GGUF bytes in a Unicode path", (t) => {
  const f = fixture(t);
  assert.equal(f.result.status, 0, f.result.stderr);
  assert.deepEqual(readFileSync(join(f.out, "test.gguf")), bytes);
  assert.deepEqual(readFileSync(join(f.cache, `test-model-${hash(bytes)}`)), bytes);
  assert.equal(existsSync(join(f.root, "src-tauri/binaries/llama")), false);
  assert.equal(readFileSync(join(f.root, "requests"), "utf8"), "https://fixture.invalid/model.gguf\n");
});

test("N6/#30: a valid cache avoids the network entirely", (t) => {
  const f = fixture(t, { cached: bytes, status: 503 });
  assert.equal(f.result.status, 0, f.result.stderr);
  assert.equal(existsSync(join(f.root, "requests")), false);
  assert.deepEqual(readFileSync(join(f.out, "test.gguf")), bytes);
});

test("N6/#30: corrupt cached bytes are fetched again and replaced after verification", (t) => {
  const f = fixture(t, { cached: Buffer.from("bad cached data") });
  assert.equal(f.result.status, 0, f.result.stderr);
  assert.deepEqual(readFileSync(join(f.cache, `test-model-${hash(bytes)}`)), bytes);
  assert.ok(existsSync(join(f.root, "requests")));
});

for (const [name, options, message] of [
  ["wrong hash", { digest: "0".repeat(64) }, /sha256 mismatch/],
  ["wrong size", { size: bytes.length + 1 }, /bytes, want/],
  ["non-GGUF payload despite a matching hash", { body: Buffer.from("HTML"), size: 4, digest: hash(Buffer.from("HTML")) }, /not a GGUF file/],
  ["HTTP failure", { status: 503 }, /HTTP 503/],
]) {
  test(`N6/#30: rejects ${name} without installing a model`, (t) => {
    const f = fixture(t, options);
    assert.equal(f.result.status, 1, f.result.stderr);
    assert.match(f.result.stderr, message);
    assert.equal(existsSync(f.out), false);
  });
}
