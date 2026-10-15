// #29, #41, PRD §7 cross-platform NFR, decisions D-010/D-011/D-023.
// Dependency-free configuration contracts, not an Actions runner simulation.
// These assertions use the workflow's block layout; actionlint separately
// validates YAML and GitHub expressions. No network or native builds here.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
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

test("D-011/D-023 checks run on every push to main and every PR, not on bare branch pushes", () => {
  const push = block("  push:");
  assert.match(push, /^    branches: \[main\]$/m, "only main pushes trigger; PR branches run via pull_request");
  assert.doesNotMatch(push, /^    (?:branches-ignore|paths|paths-ignore|tags|tags-ignore):/m);
  assert.doesNotMatch(test_, /^    if:/m);
});

test("D-023: the full checks and tests run natively on Linux, Windows and macOS, independently", () => {
  const rows = [...test_.matchAll(/          - os: (\S+)\n            runner: (\S+)\n            target: (\S+)/g)]
    .map(([, os, runner, target]) => [os, runner, target]);
  assert.deepEqual(rows, [
    ["linux", "ubuntu-24.04", "x86_64-unknown-linux-gnu"],
    ["windows", "windows-2025", "x86_64-pc-windows-msvc"],
    ["macos", "macos-26", "aarch64-apple-darwin"],
  ]);
  assert.match(test_, /^      fail-fast: false$/m, "one OS failing must not cancel the others");
  assert.match(test_, /^    runs-on: \$\{\{ matrix\.runner \}\}$/m);
  assert.match(test_, /node scripts\/fetch-sidecars\.mjs --target \$\{\{ matrix\.target \}\} --check$/m,
    "each OS installs and checks its own sidecars");
  assert.match(test_, /^          key: test-\$\{\{ matrix\.os \}\}$/m, "per-OS Rust cache");
});

test("the self-hosted box job is opt-in, never runs fork PRs, and runs the same checks", () => {
  const box = block("  test-box:");
  assert.match(box, /^    runs-on: \[self-hosted, linux, ryzen\]$/m);
  const condition = box.match(/^    if: >-\n((?:      .+\n?)+)/m)?.[1].replace(/\s+/g, " ").trim();
  assert.ok(condition, "the box job needs an opt-in guard");
  const expression = condition.replaceAll("github.event.pull_request.labels.*.name", "labelNames");
  for (const [eventName, head, labelNames, expected] of [
    ["workflow_dispatch", null, [], true],
    ["push", null, [], false],
    ["pull_request", "clmcclease/queue", [], false],
    ["pull_request", "clmcclease/queue", ["ci:box"], true],
    ["pull_request", "someone/fork", ["ci:box"], false],
  ]) {
    assert.equal(runInNewContext(expression, {
      github: { event_name: eventName, repository: "clmcclease/queue", event: { pull_request: { head: { repo: { full_name: head } } } } },
      labelNames,
      contains: (values, value) => values.includes(value),
    }, { timeout: 1000 }), expected, `${eventName} ${head} ${JSON.stringify(labelNames)}`);
  }
  for (const command of ["npm test", "cargo clippy --locked --all-targets -- -D warnings", "cargo test --locked"]) {
    assert.ok(steps(box).some((entry) => entry.split("\n")[0] === `run: ${command}`), `box job runs ${command}`);
  }
  assert.doesNotMatch(box, /sudo /, "the runner user has no sudo");
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

test("bundles depend on the tests and PR bundling is gated by ci:bundle", () => {
  assert.match(bundle, /^    needs: test$/m);
  const condition = bundle.match(/^    if: (.+)$/m)?.[1];
  assert.ok(condition, "bundling must have an event guard");
  // The guard uses JS-compatible boolean operators plus GitHub's label projection.
  // actionlint validates the original expression. Exercise its event/label cases
  // in an isolated context so equivalent guards can satisfy the contract.
  const expression = condition.replace(/^\$\{\{\s*|\s*\}\}$/g, "")
    .replaceAll("github.event.pull_request.labels.*.name", "labelNames");
  for (const [eventName, labelNames, expected] of [
    ["push", [], true],
    ["workflow_dispatch", [], true],
    ["pull_request", [], false],
    ["pull_request", ["bug", "ci:bundle-extra"], false],
    ["pull_request", ["ci:bundle"], true],
    ["pull_request", ["bug", "ci:bundle"], true],
  ]) {
    assert.equal(runInNewContext(expression, {
      github: { event_name: eventName, ref: "refs/heads/main", event: {} },
      labelNames,
      contains: (values, value) => values.includes(value),
    }, { timeout: 1000 }), expected, `${eventName} with ${JSON.stringify(labelNames)}`);
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
