// yt-dlp's signed checksums (#44, D-035): the parts of --verify-ytdlp-signature that don't need
// gpg, so they're tested on any machine.

/** `SHA2-256SUMS` as a map of file name → sha256 (lower-case). A `*` before a name (binary
 *  mode) is dropped; malformed lines are ignored. */
export function parseSums(text) {
  const sums = new Map();
  for (const line of text.split(/\r?\n/)) {
    const m = /^([0-9a-fA-F]{64}) [ *]?(\S.*)$/.exec(line.trim());
    if (m) sums.set(m[2].replace(/^\*/, "").trim(), m[1].toLowerCase());
  }
  return sums;
}

/** Every pinned asset whose name and sha256 aren't paired on one signed line, as messages. */
export function unsignedAssets(sums, assets) {
  const problems = [];
  const seen = new Set();
  for (const { asset, sha256 } of Object.values(assets)) {
    // One check per name and hash: two targets pinning the same file to different hashes are
    // both checked (#44 QA).
    const pin = `${asset} ${sha256.toLowerCase()}`;
    if (seen.has(pin)) continue;
    seen.add(pin);
    const signed = sums.get(asset);
    if (signed === undefined) problems.push(`${asset}: not in the signed sums`);
    else if (signed !== sha256.toLowerCase()) problems.push(`${asset}: signed sha256 ${signed}, pinned ${sha256}`);
  }
  return problems;
}

/** gpg's `--status-fd` output: the fingerprints it imported, and for each valid signature the
 *  primary key's fingerprint (the last field of VALIDSIG, so a signing subkey counts too). */
export function parseStatus(text) {
  const imported = new Set();
  const valid = [];
  for (const line of text.split(/\r?\n/)) {
    const fields = line.trim().split(/\s+/);
    if (fields[0] !== "[GNUPG:]") continue;
    if (fields[1] === "IMPORT_OK" && fields[3]) imported.add(fields[3].toUpperCase());
    if (fields[1] === "VALIDSIG" && fields.length >= 12) valid.push(fields[11].toUpperCase());
  }
  return { imported: [...imported], valid };
}
