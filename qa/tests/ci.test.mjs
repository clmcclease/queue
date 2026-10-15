// #29, #41, PRD §7 cross-platform NFR, decisions D-010/D-011/D-023.
// Dependency-free configuration contracts, not an Actions runner simulation.
// These assertions use the workflow's block layout; actionlint separately
// validates YAML and GitHub expressions. No network or native builds here.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { runInNewContext } from "node:vm";

const root = new URL("../../", import.meta.url);
// Normalize CRLF so a Windows checkout made without .gitattributes still parses.
const read = (path) => readFileSync(new URL(path, root), "utf8").replaceAll("\r\n", "\n");
const workflow = read(".github/workflows/build.yml");
const lines = workflow.split("\n");

function block(header) {
  const start = lines.indexOf(header);
  assert.ok(start >= 0, `missing workflow block ${header.trim()}`);
  const indent = header.length - header.trimStart().length;
  let end = start + 1;
  while (end < lines.length) {
    const line = lines[end];
    if (line.trim() && !line.trimStart().startsWith("#") &&
        line.length - line.trimStart().length <= indent) break;
    end++;
  }
  return lines.slice(start + 1, end).join("\n");
}

const test_ = block("  test:");
const bundle = block("  bundle:");
const steps = (job) => job.split(/^      - /m).slice(1);

test("D-011/D-023 a run happens on every push to main and every PR, not on bare branch pushes", () => {
  const push = block("  push:");
  assert.match(push, /^    branches: \[main\]$/m, "only main pushes trigger; PR branches run via pull_request");
  assert.doesNotMatch(push, /^    (?:branches-ignore|paths|paths-ignore|tags|tags-ignore):/m);
  assert.doesNotMatch(block("  plan:"), /^    if:/m, "the plan always runs");
});

// Runs scripts/ci-plan.sh (bash; it also has to work with macOS's bash 3.2) and parses its outputs.
function plan(env) {
  const run = spawnSync("bash", [fileURLToPath(new URL("scripts/ci-plan.sh", root))], {
    encoding: "utf8", timeout: 10000,
    env: { PATH: process.env.PATH, GITHUB_OUTPUT: "", ...env },
  });
  assert.equal(run.status, 0, run.stderr);
  const out = Object.fromEntries(run.stdout.trim().split("\n").map((line) => line.split(/=(.*)/s).slice(0, 2)));
  return { box: out.box, native: out.native, tests: JSON.parse(out.tests).map((r) => r.os), ffmpeg: JSON.parse(out.ffmpeg).map((r) => r.key), rows: JSON.parse(out.tests) };
}

test("D-023: the plan tests Windows/macOS only on main, manual runs and ci:native/ci:bundle PRs", { skip: process.platform === "win32" && "ci-plan.sh is bash" }, () => {
  for (const [event, labels, tests, ffmpeg] of [
    ["pull_request", [], ["linux"], ["linux-windows"]],
    ["pull_request", ["bug"], ["linux"], ["linux-windows"]],
    ["pull_request", ["ci:native"], ["linux", "windows", "macos"], ["macos", "linux-windows"]],
    ["pull_request", ["ci:bundle"], ["linux", "windows", "macos"], ["macos", "linux-windows"]],
    ["pull_request", ["ci:bundle-extra"], ["linux"], ["linux-windows"]],
    ["push", [], ["linux", "windows", "macos"], ["macos", "linux-windows"]],
    ["workflow_dispatch", [], ["linux", "windows", "macos"], ["macos", "linux-windows"]],
  ]) {
    const p = plan({ EVENT: event, LABELS: JSON.stringify(labels) });
    assert.deepEqual([p.tests, p.ffmpeg, p.box], [tests, ffmpeg, "false"], `${event} ${JSON.stringify(labels)}`);
  }
  assert.deepEqual(plan({ EVENT: "push" }).rows, [
    { os: "linux", runner: "ubuntu-24.04", target: "x86_64-unknown-linux-gnu" },
    { os: "windows", runner: "windows-2025", target: "x86_64-pc-windows-msvc" },
    { os: "macos", runner: "macos-26", target: "aarch64-apple-darwin" },
  ], "pinned runners and each OS's own target");
});

test("D-023: Linux moves to the box only when it's known to be up, and never for forks", { skip: process.platform === "win32" && "ci-plan.sh is bash" }, () => {
  const labelled = plan({ EVENT: "pull_request", LABELS: '["ci:box"]' });
  assert.deepEqual([labelled.box, labelled.tests], ["true", []], "ci:box puts Linux on the box");
  const fork = plan({ EVENT: "pull_request", LABELS: '["ci:box","ci:native"]', FORK: "true" });
  assert.deepEqual([fork.box, fork.tests], ["false", ["linux", "windows", "macos"]], "fork PRs never reach the box");
  // A token but no reachable API: counts as offline, so hosted Linux runs (nothing is left pending).
  const unreachable = plan({ EVENT: "pull_request", RUNNER_TOKEN: "x", REPO: "o/r", API: "http://127.0.0.1:9" });
  assert.deepEqual([unreachable.box, unreachable.tests], ["false", ["linux"]]);
});

test("the workflow takes its matrices from the plan, and the box job runs the same checks", () => {
  assert.match(block("  plan:"), /^          RUNNER_TOKEN: \$\{\{ secrets\.RUNNER_STATUS_TOKEN \}\}$/m);
  assert.match(block("  ffmpeg:"), /^        include: \$\{\{ fromJSON\(needs\.plan\.outputs\.ffmpeg\) \}\}$/m);
  assert.match(test_, /^        include: \$\{\{ fromJSON\(needs\.plan\.outputs\.tests\) \}\}$/m);
  assert.match(test_, /^    if: needs\.plan\.outputs\.tests != '\[\]'$/m, "no empty matrix");
  assert.match(test_, /^      fail-fast: false$/m, "one OS failing must not cancel the others");
  assert.match(test_, /^    runs-on: \$\{\{ matrix\.runner \}\}$/m);
  assert.match(test_, /node scripts\/fetch-sidecars\.mjs --target \$\{\{ matrix\.target \}\} --check$/m,
    "each OS installs and checks its own sidecars");
  assert.match(test_, /^          key: test-\$\{\{ matrix\.os \}\}$/m, "per-OS Rust cache");
  const box = block("  test-box:");
  assert.match(box, /^    runs-on: \[self-hosted, linux, ryzen\]$/m);
  assert.match(box, /^    if: needs\.plan\.outputs\.box == 'true'$/m);
  for (const command of ["npm test", "cargo clippy --locked --all-targets -- -D warnings", "cargo test --locked"]) {
    assert.ok(steps(box).some((entry) => entry.split("\n")[0] === `run: ${command}`), `box job runs ${command}`);
  }
  assert.doesNotMatch(box, /sudo /, "the runner user has no sudo");
  assert.match(box, /echo "\$HOME\/\.cargo\/bin" >> "\$GITHUB_PATH"/, "the service has no login PATH; cargo must be added");
});

test("PR checks run on open, synchronize, reopen, and label events", () => {
  const pr = block("  pull_request:");
  for (const event of ["opened", "synchronize", "reopened", "labeled"]) {
    assert.match(pr, new RegExp(`\\b${event}\\b`));
  }
  assert.match(workflow, /^  workflow_dispatch:/m);
});

test("all four desktop targets use pinned compatible runners and independent matrix jobs", () => {
  const targets = [...bundle.matchAll(/          - name: (\S+)\n            runner: (\S+)\n            target: (\S+)/g)]
    .map(([, name, runner, target]) => [name, runner, target]);
  assert.deepEqual(targets, [
    ["macos-arm64", "macos-26", "aarch64-apple-darwin"],
    ["macos-x64", "macos-26", "x86_64-apple-darwin"],
    ["windows-x64", "windows-2025", "x86_64-pc-windows-msvc"],
    ["linux-x64", "ubuntu-24.04", "x86_64-unknown-linux-gnu"],
  ]);
  assert.match(bundle, /^      fail-fast: false$/m);
  assert.match(bundle, /^    runs-on: \$\{\{ matrix\.runner \}\}$/m);
});

test("bundles need every planned test to pass and PR bundling is gated by ci:bundle", () => {
  assert.match(bundle, /^    needs: \[plan, test, test-box\]$/m);
  const condition = bundle.match(/^    if: >-\n((?:      .+\n?)+)/m)?.[1].replace(/\s+/g, " ").trim();
  assert.ok(condition, "bundling must have a guard");
  const expression = condition
    .replaceAll("github.event.pull_request.labels.*.name", "labelNames")
    .replaceAll("needs.*.result", "results")
    .replaceAll("!cancelled()", "!runCancelled");
  for (const [eventName, labelNames, results, expected] of [
    ["push", [], ["success", "success", "skipped"], true],
    ["push", [], ["success", "skipped", "success"], true],
    ["workflow_dispatch", [], ["success", "success", "skipped"], true],
    ["push", [], ["success", "failure", "skipped"], false],
    ["push", [], ["success", "success", "cancelled"], false],
    ["pull_request", [], ["success", "success", "skipped"], false],
    ["pull_request", ["bug", "ci:bundle-extra"], ["success", "success", "skipped"], false],
    ["pull_request", ["ci:bundle"], ["success", "success", "skipped"], true],
    ["pull_request", ["bug", "ci:bundle"], ["success", "failure", "skipped"], false],
  ]) {
    assert.equal(runInNewContext(expression, {
      github: { event_name: eventName, event: {} },
      labelNames, results, runCancelled: false,
      contains: (values, value) => values.includes(value),
    }, { timeout: 1000 }), expected, `${eventName} ${JSON.stringify(labelNames)} ${results}`);
  }
  assert.match(workflow, /^  cancel-in-progress: \$\{\{ github\.event_name == 'pull_request' \}\}$/m,
    "main push bundles must not be canceled by a subsequent push");
  // GitHub keeps one pending run per group, so non-PR runs need a unique group or queued main pushes are dropped.
  assert.match(workflow, /^  group: \$\{\{ github\.workflow \}\}-\$\{\{ github\.event_name == 'pull_request' && github\.ref \|\| github\.run_id \}\}$/m);
});

test("CI runs the frontend build before tests and all required Rust checks", () => {
  const entries = steps(test_);
  const position = (command) => entries.findIndex((entry) => entry.split("\n")[0] === `run: ${command}`);
  const install = position("npm ci");
  const build = position("npm run build");
  const tests = position("npm test");
  assert.ok(install >= 0 && build > install && tests > build);
  for (const command of ["cargo fmt --check", "cargo clippy --locked --all-targets -- -D warnings", "cargo test --locked"]) {
    const index = position(command);
    assert.ok(index >= 0, `missing ${command}`);
    assert.match(entries[index], /^        working-directory: src-tauri$/m);
  }
});

test("each job reads the pinned Node and Rust configuration and Linux dependencies", () => {
  const pkg = JSON.parse(read("package.json"));
  assert.match(read("mise.toml"), new RegExp(`^node = "${pkg.engines.node.replaceAll(".", "\\.")}"$`, "m"));
  for (const job of [test_, bundle]) {
    assert.match(job, /^          node-version-file: package\.json$/m);
    assert.doesNotMatch(job, /^          node-version:/m);
    assert.match(job, /run: (?:\|\n          )?rustup toolchain install(?:\n|$)/);
    assert.doesNotMatch(job, /rustup (?:default|update)\s/);
    for (const dependency of ["libwebkit2gtk-4.1-dev", "libssl-dev", "libayatana-appindicator3-dev", "librsvg2-dev"]) {
      assert.ok(job.includes(dependency), `missing Linux prerequisite ${dependency}`);
    }
  }
  for (const job of [test_, bundle]) assert.match(job, /^        if: runner\.os == 'Linux'$/m);
  assert.match(bundle, /^          rustup target add \$\{\{ matrix\.target \}\}$/m);
});

test("bundles upload workflow artifacts without creating releases or enabling signing", () => {
  const action = steps(bundle).find((entry) => entry.startsWith("uses: tauri-apps/tauri-action@"));
  assert.ok(action, "use the official tauri-action");
  assert.match(action, /^          args: --target \$\{\{ matrix\.target \}\}$/m);
  assert.match(action, /^          uploadWorkflowArtifacts: true$/m);
  assert.doesNotMatch(action, /^\s+(?:releaseId|tagName|releaseName):/m);
  assert.match(block("permissions:"), /^  contents: read$/m);
  assert.doesNotMatch(workflow, /(?:APPLE_CERTIFICATE|APPLE_SIGNING_IDENTITY|TAURI_SIGNING_PRIVATE_KEY|WINDOWS_CERTIFICATE)/);
  const tauri = JSON.parse(read("src-tauri/tauri.conf.json"));
  assert.equal(tauri.bundle.active, true);
  assert.equal(tauri.bundle.targets, "all", "include Windows installers and Linux AppImage/deb");
});

test("all Actions are SHA-pinned and every bundle runs frontend tests after building", () => {
  const actions = [...workflow.matchAll(/^      - uses: (\S+)/gm)];
  assert.ok(actions.length >= 7);
  for (const [, action] of actions) assert.match(action, /^[\w.-]+\/[\w.-]+@[a-f0-9]{40}$/);
  const entries = steps(bundle);
  const build = entries.findIndex((entry) => entry.startsWith("uses: tauri-apps/tauri-action@"));
  const tests = entries.findIndex((entry) => entry.split("\n")[0] === "run: npm test");
  assert.ok(tests > build && build >= 0);
  assert.equal(JSON.parse(read("src-tauri/tauri.conf.json")).build.beforeBuildCommand, "npm run build");
});
