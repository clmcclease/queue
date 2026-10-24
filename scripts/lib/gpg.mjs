// A detached signature checked with gpg (#44, #86, D-035), shared by every signed sidecar. The
// data and signature files are untrusted: they're only ever gpg's file arguments.
import { spawnSync } from "node:child_process";
import { parseStatus } from "./signature.mjs";

export const GPG_MISSING = "gpg is required for this mode and wasn't found";

/** Imports `keyFile` into the throwaway `home`, which must yield exactly `fingerprint`, and
 *  verifies `sigFile` over `dataFile`: gpg exits 0 and a VALIDSIG names that primary key.
 *  Throws with the reason otherwise; returns the fingerprint. */
export function verifyDetached({ home, keyFile, fingerprint, dataFile, sigFile }) {
  const want = fingerprint.toUpperCase();
  const gpg = (...argv) => {
    const run = spawnSync("gpg", ["--homedir", home, "--batch", "--no-tty", "--status-fd", "1", ...argv], { encoding: "utf8" });
    if (run.error?.code === "ENOENT") throw new Error(GPG_MISSING);
    if (run.error) throw run.error;
    return run;
  };
  const imported = parseStatus(gpg("--import", keyFile).stdout).imported;
  if (imported.length !== 1 || imported[0] !== want) {
    throw new Error(`the committed key imported as [${imported.join(", ")}], expected exactly ${want}`);
  }
  const verified = gpg("--verify", sigFile, dataFile);
  const valid = parseStatus(verified.stdout).valid;
  if (verified.status !== 0 || !valid.includes(want)) {
    throw new Error(`no valid signature by ${want} (gpg exit ${verified.status}; valid: [${valid.join(", ")}])\n${verified.stderr}`);
  }
  return want;
}
