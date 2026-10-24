// Independent NFR/#86(a), D-035: prove the committed primary public key matches
// the architect's fingerprint without gpg. This is not signature verification.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import test from "node:test";

test("NFR/#86 QA: FFmpeg's committed v4 primary key matches the exact approved pin", () => {
  const root = new URL("../../", import.meta.url);
  const { signing } = JSON.parse(readFileSync(new URL("scripts/sidecars.json", root))).ffmpeg;
  assert.equal(signing.key, "scripts/keys/ffmpeg-release.asc");
  assert.equal(signing.fingerprint, "FCF986EA15E6E293A5644F10B4322F04D67658D8");
  assert.equal(signing.source, "https://ffmpeg.org/download.html");
  const armor = readFileSync(new URL(signing.key, root), "utf8").replaceAll("\r\n", "\n");
  assert.match(armor, /^-----BEGIN PGP PUBLIC KEY BLOCK-----\n\n[\s\S]+\n-----END PGP PUBLIC KEY BLOCK-----\n?$/);
  const packets = Buffer.from(armor.split("\n").filter(line => line && !line.startsWith("-----") && !line.startsWith("=")).join(""), "base64");
  // The committed key uses an old-format tag-6 packet with a two-byte length.
  assert.equal(packets[0], 0x99);
  assert.equal(packets[3], 4);
  const end = 3 + packets.readUInt16BE(1);
  assert.ok(end <= packets.length);
  const fingerprint = createHash("sha1").update(packets.subarray(0, end)).digest("hex").toUpperCase();
  assert.equal(fingerprint, signing.fingerprint);
});
