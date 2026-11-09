#!/usr/bin/env node
// Fetch the msedgedriver that matches this Windows machine's WebView2 runtime (#133, D-037).
//
//   node scripts/fetch-msedgedriver.mjs <dir>
//
// msedgedriver must match the WebView2 runtime's version exactly, and the runtime updates on its
// own, so this is pinned by version rather than by checksum: it reads the installed runtime's
// version from the registry, downloads that exact build from Microsoft's official endpoint over
// HTTPS, logs the archive's sha256, and checks the driver reports the same version. Windows only;
// used by CI's `webdriver (windows)` job.
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

// WebView2 Runtime's EdgeUpdate client id (the Evergreen runtime), machine-wide then per-user.
const CLIENT = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
export const REGISTRY_KEYS = [
  `HKLM\\SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\${CLIENT}`,
  `HKLM\\SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\${CLIENT}`,
  `HKCU\\SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\${CLIENT}`,
];

// The `pv` value from `reg query <key> /v pv` output, or null. Only a plain four-part version
// is accepted, since it becomes part of a download URL.
export function parseRegVersion(output) {
  const m = /^\s*pv\s+REG_SZ\s+(\S+)\s*$/m.exec(String(output));
  return m && isVersion(m[1]) ? m[1] : null;
}

export const isVersion = (v) => typeof v === "string" && /^\d{1,5}(\.\d{1,5}){3}$/.test(v) && !/^0\.0\.0\.0$/.test(v);

export function driverUrl(version) {
  if (!isVersion(version)) throw new Error(`not a WebView2 version: ${JSON.stringify(version)}`);
  return `https://msedgedriver.microsoft.com/${version}/edgedriver_win64.zip`;
}

function webview2Version() {
  for (const key of REGISTRY_KEYS) {
    const r = spawnSync("reg", ["query", key, "/v", "pv"], { encoding: "utf8" });
    const v = r.status === 0 ? parseRegVersion(r.stdout) : null;
    if (v) return { version: v, key };
  }
  throw new Error(`no WebView2 runtime version found under:\n  ${REGISTRY_KEYS.join("\n  ")}`);
}

async function main() {
  const [dirArg] = process.argv.slice(2);
  if (!dirArg || process.argv.length > 3) throw new Error("usage: node scripts/fetch-msedgedriver.mjs <dir>");
  if (process.platform !== "win32") throw new Error("msedgedriver is only fetched on Windows");
  const dir = resolve(dirArg);
  const { version, key } = webview2Version();
  console.log(`WebView2 runtime ${version} (${key})`);
  const url = driverUrl(version);
  console.log(`downloading ${url}`);
  const res = await fetch(url);
  if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
  const zip = Buffer.from(await res.arrayBuffer());
  console.log(`edgedriver_win64.zip ${zip.length} bytes, sha256 ${createHash("sha256").update(zip).digest("hex")}`);
  mkdirSync(dir, { recursive: true });
  const zipPath = join(dir, "edgedriver_win64.zip");
  writeFileSync(zipPath, zip);
  // Windows' own bsdtar reads zip; a Git-for-Windows GNU tar earlier on PATH wouldn't.
  const tar = join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe");
  const x = spawnSync(tar, ["-xf", zipPath, "-C", dir, "msedgedriver.exe"], { encoding: "utf8" });
  if (x.status !== 0) throw new Error(`extracting msedgedriver.exe: ${x.stderr || x.error?.message}`);
  const exe = join(dir, "msedgedriver.exe");
  if (!existsSync(exe)) throw new Error(`${exe} missing after extraction`);
  const v = spawnSync(exe, ["--version"], { encoding: "utf8" });
  const reported = `${v.stdout ?? ""}`.trim();
  console.log(reported);
  if (v.status !== 0 || !reported.split(/\s+/).includes(version)) {
    throw new Error(`msedgedriver reports "${reported}", expected version ${version}`);
  }
  console.log(exe);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((e) => {
    console.error(e.message ?? e);
    process.exit(1);
  });
}
