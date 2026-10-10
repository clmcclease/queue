# Status

**Updated:** 2026-10-10 · **Milestone:** M1 (walking skeleton) in progress · **Active:** #1 (scaffold) done → #29 next

## Current state
- M0 is complete: PRD v2, backlog, decisions D-001–D-009, and Codex QA (see docs/QA.md).
- **#1 is done.** The Tauri v2 + React/TS/Tailwind app opens an empty "Queue" window.
  - Toolchains are installed on the owner's Mac (`brew install mise rustup`) and pinned in the repo (D-010): Node 24.21.0 in `mise.toml`, Rust 1.99.0 in `rust-toolchain.toml`.
  - Exact pins: Tauri CLI 2.12.1 / API 2.12.2, `tauri` =2.12.2, `tauri-build` =2.7.1, React 19.3.0, Vite 8.3.4, Tailwind 4.3.3 (Vite plugin), TypeScript 6.0.3. `package-lock.json` and `Cargo.lock` are committed.
  - Generated from create-tauri-app 4.7.5 (react-ts). Removed: the greet demo, `tauri-plugin-opener`, serde, and the template SVGs. The app identifier is `dev.mcclease.queue`.
  - The icons are the Tauri template defaults; real icons come in #18.
  - README has a Development section with setup steps.

## Checks performed (2026-10-10, macOS arm64 only)
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
#1 is committed. Next, do **#29** (cross-platform CI with tauri-action) before #2, so Windows and Linux build problems show up early. CI should read `rust-toolchain.toml` and the Node version in `mise.toml`, and run the scaffold tests after the frontend build.
