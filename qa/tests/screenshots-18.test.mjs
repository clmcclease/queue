// #18 (D-045): the webdriver job's screenshots. The pure parts of qa/webdriver/screenshots-18.mjs,
// the seed's refusal outside GitHub Actions, the fixture, and the workflow contract. The real
// captures run only in CI (Linux and Windows).
import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { EventEmitter } from "node:events";
import {
  FRAME_TEXT,
  IDENTIFIER,
  MIN_PNG_BYTES,
  exited,
  frameName,
  missingDarkLine,
  shoot,
  parseArgs,
  pngProblems,
  queueFile,
  schemeOf,
  seedQueue,
  themeVerdict,
} from "../webdriver/screenshots-18.mjs";

const root = new URL("../../", import.meta.url);

test("#18: arguments need an app, a light or dark theme and an output folder", () => {
  const opts = parseArgs(["--app", "a", "--theme", "dark", "--out", "o", "--require-dark", "--native-driver", "d"]);
  assert.deepEqual(opts, { app: "a", theme: "dark", out: "o", requireDark: true, nativeDriver: "d", tauriDriver: "tauri-driver" });
  assert.equal(parseArgs(["--app", "a", "--theme", "light", "--out", "o"]).requireDark, false);
  for (const bad of [
    ["--theme", "light", "--out", "o"],
    ["--app", "a", "--out", "o"],
    ["--app", "a", "--theme", "sepia", "--out", "o"],
    ["--app", "a", "--theme", "light"],
    ["--app", "a", "--theme", "light", "--out"],
    ["--app", "a", "--theme", "light", "--out", "o", "--other", "x"],
  ]) {
    assert.throws(() => parseArgs(bad), undefined, bad.join(" "));
  }
});

test("#18: queue.json is found where Tauri keeps the app's data on each OS", () => {
  const home = "/home/runner";
  assert.equal(queueFile("linux", {}, home), join(home, ".local", "share", IDENTIFIER, "queue.json"));
  assert.equal(queueFile("linux", { XDG_DATA_HOME: "/xdg" }, home), join("/xdg", IDENTIFIER, "queue.json"));
  assert.equal(queueFile("win32", { APPDATA: "C:\\Users\\r\\AppData\\Roaming" }, home), join("C:\\Users\\r\\AppData\\Roaming", IDENTIFIER, "queue.json"));
  assert.throws(() => queueFile("win32", {}, home), /APPDATA/);
  assert.equal(queueFile("darwin", {}, "/Users/me"), join("/Users/me", "Library", "Application Support", IDENTIFIER, "queue.json"));
  const conf = JSON.parse(readFileSync(new URL("src-tauri/tauri.conf.json", root), "utf8"));
  assert.equal(IDENTIFIER, conf.identifier, "the identifier is the app's own");
});

test("#18: seeding refuses to write unless it's GitHub Actions, so a real queue is never replaced", () => {
  const dir = mkdtempSync(join(tmpdir(), "queue-18-seed-"));
  try {
    const file = join(dir, "data", "queue.json");
    for (const env of [{}, { GITHUB_ACTIONS: "false" }, { GITHUB_ACTIONS: "1" }, { CI: "true" }]) {
      assert.throws(() => seedQueue(file, "{}", env), /refusing to write/, JSON.stringify(env));
      assert.equal(existsSync(file), false, `${JSON.stringify(env)}: nothing written`);
    }
    seedQueue(file, '{"version":1}', { GITHUB_ACTIONS: "true" });
    assert.equal(readFileSync(file, "utf8"), '{"version":1}');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("#18: the fixture has a Queued row with a notice, a Done row and a Failed row, and no thumbnail URL", () => {
  const fixture = JSON.parse(readFileSync(new URL("qa/webdriver/queue-18.json", root), "utf8"));
  assert.equal(fixture.version, 1);
  assert.deepEqual(fixture.items.map((i) => i.status.state), ["queued", "done", "failed"]);
  assert.ok(fixture.items[0].notice, "the Queued row carries a notice");
  assert.ok(!fixture.items.some((i) => i.status.state === "fetching"), "no Fetching row");
  assert.ok(!JSON.stringify(fixture).includes("http://") && fixture.items.every((i) => !i.meta?.thumbnail), "no CDN thumbnail");
  assert.equal(fixture.items[0].meta.title, "A queued video", "the text the rows frame waits for");
});

test("#18: files are named by OS, frame and the measured scheme", () => {
  assert.equal(frameName("linux", "empty", "dark"), "linux-empty-dark.png");
  assert.equal(frameName("win32", "rows", "light"), "windows-rows-light.png");
  assert.equal(schemeOf({ dark: true }), "dark");
  assert.equal(schemeOf({ dark: false }), "light");
  assert.equal(schemeOf(null), "light");
});

test("#18: dark is measured, never assumed: light must show, dark is skipped unless required", () => {
  assert.equal(themeVerdict("light", "light", false), "capture");
  assert.equal(themeVerdict("dark", "dark", false), "capture");
  assert.equal(themeVerdict("light", "dark", false), "fail", "a light run that isn't light fails");
  assert.equal(themeVerdict("dark", "light", false), "skip", "dark not proven yet: not saved, summary says so");
  assert.equal(themeVerdict("dark", "light", true), "fail", "dark required and missing fails");
});

test("#18: a screenshot must be a real PNG of a plausible size", () => {
  const png = Buffer.alloc(MIN_PNG_BYTES);
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]).copy(png);
  assert.deepEqual(pngProblems(png), []);
  assert.deepEqual(pngProblems(png.subarray(0, MIN_PNG_BYTES - 1)).length, 1, "too small");
  const notPng = Buffer.from(png);
  notPng[1] = 0;
  assert.match(pngProblems(notPng).join(), /not a PNG/);
  assert.equal(pngProblems(Buffer.alloc(0)).length, 2);
  assert.equal(pngProblems(null).length, 2);
});

test("#18: the webdriver job takes light and dark screenshots after the probe and always uploads them", () => {
  const workflow = readFileSync(new URL(".github/workflows/build.yml", root), "utf8").replaceAll("\r\n", "\n");
  const start = workflow.indexOf("\n  webdriver:\n");
  const job = workflow.slice(start, workflow.indexOf("\n  bundle:\n"));
  const at = (needle) => {
    const i = job.indexOf(needle);
    assert.ok(i >= 0, `missing: ${needle}`);
    return i;
  };
  const probe = at("csp-probe-133.mjs --app src-tauri/target/debug/queue.exe");
  const light = at("--theme light");
  const darkSetLinux = at("gtk-application-prefer-dark-theme=1");
  const darkSetWindows = at("AppsUseLightTheme /t REG_DWORD /d 0");
  const dark = at("--theme dark");
  const upload = at("- name: Upload the screenshots");
  assert.ok(probe < light && light < darkSetLinux && darkSetLinux < dark && darkSetWindows < dark && dark < upload, "probe, light, dark theme, dark, upload");
  assert.equal(job.match(/screenshots-18\.mjs --app src-tauri\/target\/debug\/queue --theme (light|dark) --out "\$RUNNER_TEMP\/screenshots"/g)?.length, 2, "Linux light and dark");
  assert.equal(job.match(/screenshots-18\.mjs --app src-tauri\/target\/debug\/queue\.exe --native-driver "\$RUNNER_TEMP\/msedgedriver\/msedgedriver\.exe" --theme (light|dark) --out "\$RUNNER_TEMP\/screenshots"/g)?.length, 2, "Windows light and dark");
  assert.equal(job.match(/xvfb-run -a node qa\/webdriver\/screenshots-18\.mjs/g)?.length, 2, "Linux runs under xvfb");
  assert.match(job, /GTK_THEME: Adwaita:dark/);
  const uploadStep = job.slice(upload, job.indexOf("\n      - ", upload + 1) === -1 ? undefined : job.indexOf("\n      - ", upload + 1));
  assert.match(uploadStep, /if: always\(\)/);
  assert.match(uploadStep, /actions\/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9 # v7\.0\.2/, "the already-pinned SHA");
  assert.match(uploadStep, /name: screenshots-\$\{\{ matrix\.os \}\}/);
  assert.doesNotMatch(job, /continue-on-error/, "a failed light capture fails the job");
  assert.doesNotMatch(job, /emulat|prefers-color-scheme/i, "the theme is forced only through the OS");
  // Windows' screenshot steps opt out of msedgedriver's diagnostics too (D-038).
  for (const m of job.matchAll(/- name: Screenshots, (light|dark)\n {8}if: runner\.os == 'Windows'\n[\s\S]*?run: /g)) {
    assert.match(m[0], /MSEDGEDRIVER_TELEMETRY_OPTOUT: "1"/);
  }
});

// #18 PR B review round 1: one driver awaited on exit, and every skipped dark frame reported.

test("#18 review: a stopped driver is awaited until it exits, within a limit", async () => {
  const child = Object.assign(new EventEmitter(), { exitCode: null, signalCode: null });
  let done = false;
  const waiting = exited(child, 1_000).then(() => (done = true));
  await new Promise((r) => setTimeout(r, 50));
  assert.equal(done, false, "still waiting while the driver is up");
  child.emit("exit", 0, null);
  await waiting;
  assert.equal(done, true);
  await exited(Object.assign(new EventEmitter(), { exitCode: 0, signalCode: null }), 10);
  const stuck = Object.assign(new EventEmitter(), { exitCode: null, signalCode: null });
  await assert.rejects(exited(stuck, 30), /didn't exit within 30 ms/);
});

/** `shoot` with fakes: `dark` says which frames the page reports dark for. */
async function fakeRun(dark, { requireDark = false, theme = "dark" } = {}) {
  const calls = [];
  const reported = [];
  let seeded = false;
  let cleaned = false;
  const missing = await shoot({ theme, requireDark }, {
    platform: "linux",
    withSession: async (fn) => {
      calls.push("session");
      return fn("s");
    },
    capture: async (_s, frame, texts) => {
      calls.push(`${frame}:${texts.join("|")}`);
      const scheme = dark.includes(frame) ? "dark" : "light";
      if (scheme === theme) return true;
      if (theme === "light" || requireDark) throw new Error(`${frame}: asked for ${theme}, the page reports ${scheme}`);
      return false;
    },
    openSettings: async () => calls.push("open settings"),
    seed: () => {
      seeded = true;
      return () => (cleaned = true);
    },
    report: (line) => reported.push(line),
  });
  return { missing, reported, calls, seeded, cleaned };
}

test("#18 review: a partly dark run lists every frame that wasn't captured", async () => {
  const all = await fakeRun(["empty", "settings", "rows"]);
  assert.deepEqual(all.missing, []);
  assert.deepEqual(all.reported, [], "nothing missing, nothing reported");
  for (const [dark, missing] of [
    [["empty", "rows"], ["settings"]],
    [["empty", "settings"], ["rows"]],
    [[], ["empty", "settings", "rows"]],
  ]) {
    const run = await fakeRun(dark);
    assert.deepEqual(run.missing, missing, `dark for ${dark.join(",") || "none"}`);
    assert.equal(run.reported.length, 1);
    assert.match(run.reported[0], new RegExp(`^DARK NOT CAPTURED on linux for ${missing.join(", ")}:`));
  }
  assert.equal(missingDarkLine("win32", []), null);
});

test("#18 review: with --require-dark a light frame fails the run, and the seed is still removed", async () => {
  await assert.rejects(fakeRun(["empty", "settings"], { requireDark: true }), /rows: asked for dark, the page reports light/);
  let cleaned = false;
  await assert.rejects(
    shoot({ theme: "dark", requireDark: true }, {
      platform: "linux",
      withSession: async (fn) => fn("s"),
      capture: async (_s, frame) => {
        if (frame === "rows") throw new Error("rows failed");
        return true;
      },
      openSettings: async () => {},
      seed: () => () => (cleaned = true),
      report: () => {},
    }),
    /rows failed/,
  );
  assert.equal(cleaned, true, "the seeded queue.json is removed even when rows fails");
});

test("#18 review: two sessions, settings opened between frames, and the rows frame waits for all three rows", async () => {
  const run = await fakeRun(["empty", "settings", "rows"]);
  assert.deepEqual(run.calls, [
    "session",
    `empty:${FRAME_TEXT.empty.join("|")}`,
    "open settings",
    `settings:${FRAME_TEXT.settings.join("|")}`,
    "session",
    `rows:${FRAME_TEXT.rows.join("|")}`,
  ]);
  assert.equal(run.seeded && run.cleaned, true);
  const fixture = JSON.parse(readFileSync(new URL("qa/webdriver/queue-18.json", root), "utf8"));
  assert.ok(FRAME_TEXT.rows.includes(fixture.items[0].meta.title));
  assert.ok(FRAME_TEXT.rows.some((t) => fixture.items[0].notice.startsWith(t)), "the queued row's notice");
  assert.ok(FRAME_TEXT.rows.includes(fixture.items[1].meta.title), "the finished row");
  assert.ok(FRAME_TEXT.rows.includes(fixture.items[2].status.reason), "the failure reason");
});

