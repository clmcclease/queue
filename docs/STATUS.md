# Status

**Updated:** 2026-10-10 · **Milestone:** M1 (walking skeleton) **done** → M2 (real queue) next

This is a current-state handoff. Per-item history lives in git, the PRs (#34, #35, #38, #40), and `qa/reports/`.

## Current state
- **M1 is complete on `main` (87b0d2d).** All five issues are closed:

  | Issue | Item | Commit |
  |---|---|---|
  | #1 | Scaffold | `9bd2762` |
  | #29 | CI | `9f08a5e` |
  | #2 | Sidecars | `d4277f5` |
  | #3 | URL bar and list | `6d81623` |
  | #4 | Download one item | `87b0d2d` |

- **What works:** paste a link → a row appears → Download → yt-dlp downloads it into the OS Downloads folder under its title. One download runs at a time. Failures show yt-dlp's reason, and an existing file is never overwritten ("Not saved: … already exists").
- **Toolchain (D-010):** Node 24.21.0 (mise) and Rust 1.99.0 (rust-toolchain.toml); exact npm/crate pins with lockfiles. jsdom 30.1.2 is a test-only devDependency.
- **Sidecars (D-012):**
  - yt-dlp 2026.08.19 (onefile) and deno v2.9.7, fetched and checksum-verified by `scripts/fetch-sidecars.mjs`;
  - minimal LGPL ffmpeg/ffprobe 9.0.2 built from the signed source by `scripts/build-ffmpeg.sh`;
  - Tauri `externalBin` uses `queue-*` names.
- **Installer budget (D-013):** under 256 MB. Measured: dmg 77–81 MB, NSIS 49 MB, deb 84 MB, AppImage 155 MB.
- **Downloads are staged (D-014):** yt-dlp runs in a fresh app-cache folder, and the finished file is moved into place with a no-clobber hard link or copy.
- **CI (D-011):**
  - Linux checks on every PR and every push to `main`;
  - 4-OS bundles plus a per-OS sidecar `--check` on `main` and on PRs labelled `ci:bundle`;
  - a cached ffmpeg build job.
- **QA:**
  - every item passed `codex-qa test` and `review` (several rounds fixed real bugs);
  - the owner tested a real download in the app on macOS;
  - the M1 `codex-qa audit` was **YELLOW**, with no M2 blocker (`qa/reports/20261010-220217-audit-fBu7.md`). Its doc findings are fixed in this change; its test/CI findings are filed as #41–#45. The M1 milestone is closed on GitHub.
- **Backlog added this session at the owner's request:** #36 Settings screen (M2), #37 separate in-progress/completed folders (Later), and #39 D7 Open folder / Show in folder (M2).

## Checks (latest)
- **macOS arm64 locally, at the #4 merge:**
  - `cargo test` 41, plus 4 opt-in real-sidecar tests (`QUEUE_QA_REAL_SIDECARS=src-tauri/binaries [QUEUE_QA_NETWORK=1] cargo test --test download_real_d2 -- --ignored`), including a real YouTube DASH merge;
  - `npm test` 54/54; clippy, fmt and build clean.
- **Hosted CI:** green for every M1 merge on all 4 targets.
- **Not verified natively:** downloads on Windows and Linux (CI builds and runs the tests there; the Rust download tests run on Linux only), and the native webviews on Windows and Linux.

## Open questions and problems
1. Default local model: to be decided by the N9 eval (PRD §13).
2. Windows code signing: buy a certificate, or accept SmartScreen warnings? (#24)
3. Should there be an optional "most compatible (MP4/H.264)" preset, maybe with #11? D-007's default often yields `.webm` (AV1/VP9 + Opus), which QuickTime and Finder preview can't play.
4. M1 audit follow-ups, filed with the owner's OK:
   - **#41** native macOS/Windows download tests in CI (M2, P1);
   - **#42** D2 failure-path tests (M2);
   - **#45** staging-folder cleanup (M2);
   - **#43** Content Security Policy (M5, P1);
   - **#44** yt-dlp GPG signature checks (M5).

## Next bounded action
Start M2 in BACKLOG order with **#5** (Q2): validate URLs inline and reject duplicates with a toast instead of a second row. #36 (Settings screen), #10 (folder picker) and #39 (Open folder) follow later in M2.
