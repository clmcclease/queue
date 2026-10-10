// #2 / D6 / D-012: manifest loading must work in ordinary checkout paths.
// Stop at a stub curl before any network, extraction, or compiler invocation.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

const project = new URL("../../", import.meta.url);
for (const dirname of ["Queue 日本語 & spaces", "Queue's checkout"]) {
  test(`ffmpeg build loads the pinned manifest under ${JSON.stringify(dirname)}`, { skip: process.platform === "win32" && "ffmpeg build script runs on macOS/Linux only" }, (t) => {
    const parent = mkdtempSync(join(tmpdir(), "queue-ffmpeg-path-qa-"));
    t.after(() => rmSync(parent, { recursive: true, force: true }));
    const root = join(parent, dirname);
    const scripts = join(root, "scripts");
    const tools = join(parent, "tools");
    mkdirSync(scripts, { recursive: true }); mkdirSync(tools);
    copyFileSync(new URL("scripts/build-ffmpeg.sh", project), join(scripts, "build-ffmpeg.sh"));
    copyFileSync(new URL("scripts/sidecars.json", project), join(scripts, "sidecars.json"));
    writeFileSync(join(tools, "curl"), "#!/bin/sh\necho QA_REACHED_DOWNLOAD >&2\nexit 42\n", { mode: 0o755 });
    const run = spawnSync("bash", [join(scripts, "build-ffmpeg.sh"), "aarch64-apple-darwin"], {
      env: { ...process.env, PATH: `${tools}:${process.env.PATH}` }, encoding: "utf8", timeout: 10000,
    });
    assert.equal(run.status, 42, `manifest loading failed before the download stub:\n${run.stderr}`);
    assert.match(run.stderr, /QA_REACHED_DOWNLOAD/);
  });
}
