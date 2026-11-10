#!/usr/bin/env node
// #18 (D-045): screenshots of the real Queue app under WebDriver, the rendered evidence for the
// visual design pass on Linux (WebKitGTK) and Windows (WebView2). macOS has no WebDriver for
// WKWebView; there the look is the owner's hand check.
//
//   node qa/webdriver/screenshots-18.mjs --app <built app> --theme light|dark --out <dir>
//        [--require-dark] [--native-driver <path>] [--tauri-driver <path>]
//
// The OS theme is set by the CI step before this runs (GTK on Linux, the registry on Windows);
// nothing in Queue or here overrides it. Each capture first measures the scheme the page sees
// (`prefers-color-scheme`) and names the file by it, so dark is measured, never assumed. A dark
// frame that sees light isn't saved: with --require-dark that fails the step; otherwise every
// such frame is listed in a "DARK NOT CAPTURED" line in the step summary, and the step exits 0.
// One tauri-driver serves both app sessions and is awaited when it's stopped.
//
// Frames: `empty` and `settings` from a fresh queue, then `rows` from qa/webdriver/queue-18.json
// (one Queued row with a notice, one Done, one Failed). The seed writes the app's real
// queue.json, so it refuses to run anywhere but GitHub Actions.
import { spawn } from "node:child_process";
import { appendFileSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { homedir } from "node:os";
import { pathToFileURL } from "node:url";
import { until, wd } from "./csp-probe-133.mjs";

/** The app's identifier (tauri.conf.json), which names its data folder. */
export const IDENTIFIER = "dev.mcclease.queue";
const PNG_MAGIC = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
/** A real window screenshot is far larger; anything smaller is blank or broken. */
export const MIN_PNG_BYTES = 10_000;

export function parseArgs(argv) {
  const opts = { tauriDriver: "tauri-driver", requireDark: false };
  const keys = { "--app": "app", "--theme": "theme", "--out": "out", "--native-driver": "nativeDriver", "--tauri-driver": "tauriDriver" };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--require-dark") {
      opts.requireDark = true;
      continue;
    }
    const key = keys[argv[i]];
    if (!key || i + 1 >= argv.length) throw new Error(`usage: --app <path> --theme light|dark --out <dir> [--require-dark] [--native-driver <path>] [--tauri-driver <path>] (bad argument ${argv[i]})`);
    opts[key] = argv[++i];
  }
  if (!opts.app) throw new Error("--app <path to the built app> is required");
  if (opts.theme !== "light" && opts.theme !== "dark") throw new Error("--theme must be light or dark");
  if (!opts.out) throw new Error("--out <dir> is required");
  return opts;
}

/** Where the app keeps queue.json on `platform`, from Tauri's app data folder rules. */
export function queueFile(platform, env = process.env, home = homedir()) {
  if (platform === "win32") {
    if (!env.APPDATA) throw new Error("APPDATA isn't set");
    return join(env.APPDATA, IDENTIFIER, "queue.json");
  }
  if (platform === "darwin") return join(home, "Library", "Application Support", IDENTIFIER, "queue.json");
  return join(env.XDG_DATA_HOME || join(home, ".local", "share"), IDENTIFIER, "queue.json");
}

/** Writes the fixture as the app's queue.json. Never outside GitHub Actions: it replaces a real queue. */
export function seedQueue(file, fixture, env = process.env) {
  if (env.GITHUB_ACTIONS !== "true") {
    throw new Error(`refusing to write ${file}: seeding replaces the app's real queue, so it only runs on GitHub Actions`);
  }
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, fixture);
}

/** The scheme the page reports. */
export const schemeOf = (measured) => (measured?.dark ? "dark" : "light");

/** `<os>-<frame>-<scheme>.png`. */
export function frameName(platform, frame, scheme) {
  const os = { win32: "windows", linux: "linux", darwin: "macos" }[platform] ?? platform;
  return `${os}-${frame}-${scheme}.png`;
}

/** Problems with a screenshot; empty means it's a real PNG of a plausible size. */
export function pngProblems(bytes) {
  const problems = [];
  if (!bytes || bytes.length < PNG_MAGIC.length || PNG_MAGIC.some((b, i) => bytes[i] !== b)) problems.push("not a PNG");
  if (!bytes || bytes.length < MIN_PNG_BYTES) problems.push(`only ${bytes?.length ?? 0} bytes (expected at least ${MIN_PNG_BYTES})`);
  return problems;
}

/**
 * What to do when the page reports `measured` for a run asked to capture `wanted`: "capture",
 * "fail" (light didn't show, or dark is required and didn't), or "skip" (dark isn't proven on
 * this OS yet; the frames aren't saved and the summary says so).
 */
export function themeVerdict(wanted, measured, requireDark) {
  if (wanted === measured) return "capture";
  if (wanted === "light" || requireDark) return "fail";
  return "skip";
}

function summary(line) {
  console.log(line);
  if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${line}\n`);
}

const MEASURE = "return { dark: matchMedia('(prefers-color-scheme: dark)').matches, background: getComputedStyle(document.body).backgroundColor };";

/** How long a killed tauri-driver may take to exit before the run gives up on it. */
export const DRIVER_EXIT_MS = 10_000;

/**
 * Resolves when `child` has exited, or rejects after `ms`. A driver is stopped and awaited before
 * the script ends, so it never outlives the run holding ports 4444/4445 (#18 review).
 */
export function exited(child, ms = DRIVER_EXIT_MS) {
  if (child.exitCode !== null || child.signalCode !== null) return Promise.resolve();
  return new Promise((resolveExit, reject) => {
    const timer = setTimeout(() => reject(new Error(`tauri-driver didn't exit within ${ms} ms`)), ms);
    child.once("exit", () => {
      clearTimeout(timer);
      resolveExit();
    });
  });
}

/** Starts tauri-driver once for the whole run; `stop()` kills it and waits for it to exit. */
async function startDriver(opts) {
  const driverArgs = opts.nativeDriver ? ["--native-driver", resolve(opts.nativeDriver)] : [];
  const driver = spawn(opts.tauriDriver, driverArgs, { stdio: ["ignore", "inherit", "inherit"] });
  let driverExit;
  driver.on("exit", (code, signal) => { driverExit = `tauri-driver exited (${code ?? signal})`; });
  driver.on("error", (e) => { driverExit = `tauri-driver failed to start: ${e.message}`; });
  const stop = async () => {
    driver.kill();
    await exited(driver);
  };
  try {
    await until("tauri-driver", 30_000, async (left) => {
      if (driverExit) throw Object.assign(new Error(driverExit), { fatal: true });
      return (await wd("GET", "/status", undefined, left)) !== undefined;
    });
  } catch (e) {
    await stop().catch(() => {});
    throw e;
  }
  return { stop };
}

/** One app session on the running driver: runs `fn(session)` and always ends the session. */
async function withSession(opts, fn) {
  let session;
  try {
    const created = await wd("POST", "/session", {
      capabilities: { alwaysMatch: { "tauri:options": { application: resolve(opts.app) } } },
    }, 120_000);
    session = created.sessionId;
    await until("the app's page", 60_000, (left) =>
      wd("POST", `/session/${session}/execute/sync`, { script: "return document.readyState === 'complete' && !!document.querySelector('main')", args: [] }, left));
    return await fn(session);
  } finally {
    if (session) await wd("DELETE", `/session/${session}`, undefined, 10_000).catch((e) => console.error(`ending the session: ${e.message}`));
  }
}

/** Waits until the page shows every one of `texts`. */
const showing = (session, texts) =>
  until(`${texts.map((t) => `"${t}"`).join(", ")} on the page`, 30_000, (left) =>
    wd("POST", `/session/${session}/execute/sync`, {
      script: "const text = document.body.innerText; return arguments[0].every((t) => text.includes(t));",
      args: [texts],
    }, left));

/**
 * Measures the scheme, then (if it's the one wanted) checks every expected text is on the page,
 * takes the screenshot, checks it and saves it. Returns false when a dark frame is skipped.
 */
async function capture(session, opts, frame, texts) {
  const measured = await wd("POST", `/session/${session}/execute/sync`, { script: MEASURE, args: [] });
  const scheme = schemeOf(measured);
  console.log(`${frame}: prefers-color-scheme ${scheme}, body background ${measured?.background}`);
  const verdict = themeVerdict(opts.theme, scheme, opts.requireDark);
  if (verdict === "fail") throw new Error(`${frame}: asked for ${opts.theme}, the page reports ${scheme} (body background ${measured?.background})`);
  if (verdict === "skip") return false;
  await showing(session, texts);
  const png = Buffer.from(await wd("GET", `/session/${session}/screenshot`), "base64");
  const problems = pngProblems(png);
  if (problems.length) throw new Error(`${frame}: ${problems.join("; ")}`);
  writeFileSync(join(opts.out, frameName(process.platform, frame, scheme)), png);
  summary(`- ${frameName(process.platform, frame, scheme)}: ${png.length} bytes, body background ${measured?.background}`);
  return true;
}

/** What each frame must show before it's captured. The rows frame shows all three seeded rows. */
export const FRAME_TEXT = {
  empty: ["Queue is empty."],
  settings: ["Settings"],
  rows: ["A queued video", "Interrupted when Queue closed", "A finished video", "Unsupported URL"],
};

/** The summary line for dark frames that weren't captured, or null when none are missing. */
export function missingDarkLine(platform, missing) {
  if (!missing.length) return null;
  return `DARK NOT CAPTURED on ${platform} for ${missing.join(", ")}: the OS theme was set to dark, but the page reported light. Those frames go to #71's hand checks.`;
}

/**
 * The run: `empty` and `settings` in one session on a fresh queue, then `rows` in a second session
 * on the seeded queue, all on one driver. Every frame that `capture` skips is listed in the
 * summary, so a partly dark run can't pass silently (#18 review). Returns the missing frames.
 * `deps` lets tests run it without a driver or an app.
 */
export async function shoot(opts, deps) {
  const { withSession: session, capture: take, seed, report, platform } = deps;
  const missing = [];
  const take1 = async (s, frame) => {
    if (!(await take(s, frame, FRAME_TEXT[frame]))) missing.push(frame);
  };
  await session(async (s) => {
    await take1(s, "empty");
    await deps.openSettings(s);
    await take1(s, "settings");
  });
  const cleanup = seed();
  try {
    await session((s) => take1(s, "rows"));
  } finally {
    cleanup();
  }
  const line = missingDarkLine(platform, missing);
  if (line) report(line);
  return missing;
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  mkdirSync(opts.out, { recursive: true });
  summary(`### Screenshots (${opts.theme}, ${process.platform})`);
  const driver = await startDriver(opts);
  try {
    await shoot(opts, {
      platform: process.platform,
      withSession: (fn) => withSession(opts, fn),
      capture: (session, frame, texts) => capture(session, opts, frame, texts),
      openSettings: (session) =>
        wd("POST", `/session/${session}/execute/sync`, { script: "document.querySelector('button[aria-label=\"Settings\"]').click(); return true;", args: [] }),
      seed: () => {
        const file = queueFile(process.platform);
        seedQueue(file, readFileSync(new URL("./queue-18.json", import.meta.url)));
        return () => rmSync(file, { force: true });
      },
      report: summary,
    });
  } finally {
    await driver.stop();
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((e) => {
    console.error(`SCREENSHOTS: ERROR\n  ${e.stack ?? e}`);
    process.exit(1);
  });
}
