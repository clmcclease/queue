# Status

**Updated:** 2026-10-10 · **Milestone:** M1 (walking skeleton) in progress · **Active:** #29 (cross-platform CI), draft PR

## Current state
- M0 is complete: PRD v2, backlog, decisions D-001–D-009, and Codex QA (see docs/QA.md).
- **#1 is done** (commit 9bd2762, pushed; issue closed). The Tauri v2 + React/TS/Tailwind app opens an empty "Queue" window.
  - Toolchains are installed on the owner's Mac (`brew install mise rustup`) and pinned in the repo (D-010): Node 24.21.0 in `mise.toml`, Rust 1.99.0 in `rust-toolchain.toml`.
  - Exact pins: Tauri CLI 2.12.1 / API 2.12.2, `tauri` =2.12.2, `tauri-build` =2.7.1, React 19.3.0, Vite 8.3.4, Tailwind 4.3.3 (Vite plugin), TypeScript 6.0.3. `package-lock.json` and `Cargo.lock` are committed.
  - Generated from create-tauri-app 4.7.5 (react-ts). Removed: the greet demo, `tauri-plugin-opener`, serde, and the template SVGs. The app identifier is `dev.mcclease.queue`.
  - The icons are the Tauri template defaults; real icons come in #18.
  - README has a Development section with setup steps.
- **#29 is implemented** and committed on branch `ci/29-cross-platform`, with a draft PR labelled `ci:bundle`; it hasn't merged yet. `.github/workflows/build.yml` (D-011):
  - The `check` job runs on ubuntu-24.04 for every PR and every push to `main`: `npm ci`, build, `npm test`, `cargo fmt`, clippy, `cargo test`.
  - The `bundle` matrix runs tauri-action v1.0.0 for macOS arm64, macOS x64 (cross-compiled on macos-26), Windows x64, and Linux x64, and uploads unsigned workflow artifacts. It runs on pushes to `main` and on PRs labelled `ci:bundle`.
  - Actions are SHA-pinned and runner images are pinned.
  - The sidecar fetch step is deferred to #2, which doesn't exist yet. BACKLOG has been updated.
  - New `npm test` script (`node --test "qa/tests/*.test.mjs"`). The directory form `node --test qa/tests/` fails, which is why the glob is used.

## Checks performed (2026-10-10, macOS arm64 only)
- #29: actionlint 1.7.12 is clean and `npm test` passes 6/6 locally. The workflow hasn't run on GitHub yet; the first run will be on the draft PR.
- #29 first hosted run (PR #34): checks and the macOS arm64/x64 and Linux bundles passed, and all 9 artifacts uploaded (Windows msi/nsis included). The Windows job then failed in `npm test`: git's CRLF checkout broke the line-based config tests. Fixed with `.gitattributes` (`eol=lf`) plus CRLF normalization in the tests. The failure was reproduced locally on a CRLF copy (2 fail), and the fix passes it (14/14).
- #29 second hosted run (0c928b5): **all green**. check 80s; bundles: macos-arm64 114s, macos-x64 124s, linux-x64 219s, windows-x64 729s; 9 artifacts uploaded. That's about 80 billed minutes warm and about 110 cold; D-011 is updated to match.
- #29 `codex-qa test`: **FAIL**, then fixed (`qa/reports/20261010-103737-test-1s65.md`). P2: D-011's wording ("every push and PR") didn't match the triggers (main pushes and PRs). Resolved by fixing the wording rather than the trigger, which avoids running PR checks twice; the owner confirmed this policy. Codex's local checks passed, including a debug arm64 `tauri build`. The owner approved its 8 CI tests, which are merged as `qa/tests/ci.test.mjs`. The trigger test was rewritten for the confirmed policy, and a concurrency-group assertion was added. `npm test` passes 14/14.
- #29 `codex-qa review`: **FAIL**, then fixed (`qa/reports/20261010-104349-review-vCfW.md`). P2: a shared concurrency group let GitHub drop a pending push-to-main run and its artifacts. Fixed: every non-PR run now gets its own group (`github.run_id`). P3: the narrowed trigger was marked Confirmed while still pending. The owner has since confirmed it. Codex's `npm test` saw 2 tests blocked by sandbox EPERM; locally all 6 pass. The re-review was **PASS WITH NOTES** (`qa/reports/20261010-104621-review-Jfl3.md`); its only note, the decision status, is resolved.

### #1
- `npm run build` (tsc + vite): OK.
- `cargo fmt --check`: OK. `cargo clippy --all-targets -- -D warnings`: OK.
- `npm run tauri dev`: the app builds and opens a 900×640 window titled "Queue". Confirmed through the window server, System Events, and a screenshot showing "Queue is empty." in dark mode.
- Not run: `tauri build` (bundling), Windows, Linux. These are covered by the CI work in #29.
- `codex-qa test "#1"`: **PASS WITH NOTES** (`qa/reports/20261010-101935-test-TRiI.md`). Codex's checks passed, including `npm ci`, clippy, tsc, the build, `tauri build --debug --bundles app`, and its 6 new scaffold tests. It couldn't see the window itself (macOS screen access denied); Windows and Linux are untested. The owner approved its scaffold tests, which are merged as `qa/tests/scaffold.test.mjs` (run `node --test qa/tests/scaffold.test.mjs` after `npm run build`; 6/6 pass). The two empty-state tests need updating when #3 changes the screen. The patch's STATUS edit wasn't taken. An earlier run was interrupted by a terminal restart.
- `codex-qa review`: **PASS WITH NOTES** (`qa/reports/20261010-102443-review-9rpC.md`). P2: the README setup didn't put mise's Node or Homebrew rustup on PATH. Fixed, and confirmed in a clean shell (node 24.21.0, cargo 1.99.0).

## Open questions (PRD §13)
1. Which default local model? To be decided by the N9 eval.
2. Windows code signing: buy a certificate, or accept SmartScreen warnings? (#24)

## Next bounded action
For #29: watch the draft PR's CI and fix failures on the branch until checks and all 4 bundles are green, then ask the owner to approve the merge. After that, #2 (sidecars), including the CI fetch step.
