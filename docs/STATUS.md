# Status

**Updated:** 2026-10-10 · **Milestone:** M1 (walking skeleton) in progress · **Active:** M1 items all done (#1, #29, #2, #3, #4) → M1 audit next

## Current state
- M0 is complete: PRD v2, backlog, decisions D-001–D-009, and Codex QA (see docs/QA.md).
- **#1 is done** (commit 9bd2762, pushed; issue closed). The Tauri v2 + React/TS/Tailwind app opens an empty "Queue" window.
  - Toolchains are installed on the owner's Mac (`brew install mise rustup`) and pinned in the repo (D-010): Node 24.21.0 in `mise.toml`, Rust 1.99.0 in `rust-toolchain.toml`.
  - Exact pins: Tauri CLI 2.12.1 / API 2.12.2, `tauri` =2.12.2, `tauri-build` =2.7.1, React 19.3.0, Vite 8.3.4, Tailwind 4.3.3 (Vite plugin), TypeScript 6.0.3. `package-lock.json` and `Cargo.lock` are committed.
  - Generated from create-tauri-app 4.7.5 (react-ts). Removed: the greet demo, `tauri-plugin-opener`, serde, and the template SVGs. The app identifier is `dev.mcclease.queue`.
  - The icons are the Tauri template defaults; real icons come in #18.
  - README has a Development section with setup steps.
- **#29 is done.** It was squash-merged from PR #34 after a green 4-OS hosted run. `.github/workflows/build.yml` (D-011):
  - The `check` job runs on ubuntu-24.04 for every PR and every push to `main`: `npm ci`, build, `npm test`, `cargo fmt`, clippy, `cargo test`.
  - The `bundle` matrix runs tauri-action v1.0.0 for macOS arm64, macOS x64 (cross-compiled on macos-26), Windows x64, and Linux x64, and uploads unsigned workflow artifacts. It runs on pushes to `main` and on PRs labelled `ci:bundle`.
  - Actions are SHA-pinned and runner images are pinned.
  - The sidecar fetch step is deferred to #2, which doesn't exist yet. BACKLOG has been updated.
  - New `npm test` script (`node --test "qa/tests/*.test.mjs"`). The directory form `node --test qa/tests/` fails, which is why the glob is used.

- **#2 is done** (D-012). It was squash-merged from PR #35 after a green 4-OS hosted run.
  - `scripts/sidecars.json` pins yt-dlp 2026.08.19 (onefile), deno v2.9.7, and the ffmpeg 9.0.2 source, each with a SHA-256.
  - `scripts/fetch-sidecars.mjs` (Node, no dependencies, with a built-in zip reader) downloads and verifies yt-dlp and deno. `--check` confirms that yt-dlp reports our ffmpeg, ffprobe, and deno.
  - `scripts/build-ffmpeg.sh` builds a minimal LGPL ffmpeg/ffprobe with no networking: native on macOS (x64 cross-compiled) and Linux, and Windows via mingw on Linux.
  - Tauri `externalBin` uses the `queue-` names. `src-tauri/src/sidecar.rs` resolves the paths and builds the base yt-dlp argv.
  - CI: a new `ffmpeg` job (cached) feeds `check` and `bundle`, which install the sidecars and run `--check` on each OS.
  - `THIRD_PARTY_NOTICES.md`; the AGENTS.md wording is updated per D-012.
  - New `qa/tests/sidecars.test.mjs` (4 tests).

- **#3 is done** (Q1). It was squash-merged from PR #38 after green checks.
  - The Rust `queue` module holds the queue (`Queue::add` trims and rejects blank input; no validation until #5). It's exposed through the `queue_add` and `queue_list` commands held in Tauri state.
  - `src/queue.ts` is the client. `UrlBar` uses a text input with `inputMode=url` (not `type=url`, which would trigger webview validation) and a **+** that's disabled while the field is empty; focus returns to the field after each add. `QueueList` shows the raw URL per row.
  - Tailwind is limited to scanning `src/` (`source("../src")`): it had been scanning the test files, so the CSS test passed by itself. The scaffold render test now uses Vite SSR.
- New requirement **D7** (owner, 2026-10-10): an Open folder button plus per-row Show in folder → **#39** (M2, P0). PRD §6.2 and BACKLOG updated; #20 reuses it.
- Backlog additions at the owner's request (2026-10-10): **#36** Settings screen (M2, with #10) and **#37** separate in-progress/completed folders (Later; one folder by default).

- **#4 is done** (D2). It was squash-merged from PR #40 after green checks and the owner's in-app test.
  - Items carry `status` (queued, downloading, done with a path, failed with a reason) and a `rev` the UI uses to drop stale replies and events (`mergeItems`: highest rev wins).
  - The `queue_download` command marks the item Downloading and runs `download::run` on a thread. The outcome arrives as a `queue://item` event.
  - **Staged download (redesign after review round 2):** yt-dlp runs with `-P <fresh work dir>` under the app cache (`downloads/item-<id>-<nanos>`), so its partial, merge-temp, and `.fNNN` files never sit next to the user's files. Then `download::finished_file` requires exactly one regular file there, and `download::place` moves it into the output folder **without ever replacing anything**: a hard link (atomic, no-clobber), falling back to a `create_new` copy across volumes. The filesystem itself rejects an existing name, including case- and Unicode-equivalent names on APFS. The work dir is always removed. No stdout path parsing, so Windows console encoding can't break it.
  - `download::args`: base sidecar args + `--no-playlist --no-overwrites --windows-filenames --no-mtime --no-progress -P <work> -o '%(title)s.%(ext)s' -- <url>`. No `-f` (D-007); the untrusted URL goes after `--`. On Windows the process is spawned with `CREATE_NO_WINDOW`.
  - Failure reason = the last `ERROR:` line. An existing target shows "Not saved: a file named … already exists (not overwritten)". #16 will pick a free name instead.
  - One download at a time (`Queue::start`); the UI disables the other Download buttons.
  - The UI registers the event listener before taking the snapshot; a registration failure shows an alert.
  - The fixed folder is the OS Downloads dir (Tauri path API) until #10.

## Checks performed (2026-10-10, macOS arm64 only)
- #4 (macOS arm64):
  - `cargo test` 21/21, clippy and fmt clean, `npm test` 47/47 (3 new D2 UI tests with mocked events: progress → file name, a stale reply can't revert a finished row, Windows path display plus error).
  - Real YouTube download ("Me at the zoo", jNQXAC9IVRw) with the exact argv: AV1 video + Opus audio merged into `.webm` by our minimal ffmpeg, 19.02 s. deno handled YouTube's JS.
  - In the app: ✓ with the file name in `~/Downloads`; `not a url` → "Failed: [generic] 'not a url' is not a valid URL"; re-downloading an existing file → "Not downloaded … already exists", and the file stayed untouched.
  - Fresh-file mtime confirmed via CLI (the run started 18:35:00, file 18:35:03). Test files were removed from `~/Downloads`.
  - Incident: one automated keystroke sequence ran while the Queue window wasn't frontmost and may have typed a URL + Enter into another app. UI automation is stopped; the owner was told.
- #4 `codex-qa test`: **FAIL**, then fixed (`qa/reports/20261010-183537-test-VXzc.md`). Two bugs:
  1. A file created just before the run was reported as done: the mtime heuristic's 2 s slack.
  2. A directory at the reported path counted as a download.
  The mtime heuristic is replaced by a pre-run name snapshot plus an `is_file` check. Codex's tests are merged: `src-tauri/tests/download_d2.rs` (14 tests, using a fixture executable), `download_real_d2.rs` (3 opt-in tests against the real sidecars over loopback HTTP; run them with `QUEUE_QA_REAL_SIDECARS=src-tauri/binaries cargo test --test download_real_d2 -- --ignored`), and 2 UI tests. All pass locally, including the 3 real-sidecar tests that Codex's sandbox blocked. `cargo test` 35/35 (+3 opt-in), `npm test` 49/49.
- #4 `codex-qa review`: **FAIL**, then fixed (`qa/reports/20261010-184319-review-pWmI.md`):
  - P1: two concurrent same-title downloads could merge onto one path. `Queue::start` now allows **one download at a time**, and the other Download buttons are disabled while one runs; parallel downloads (#28) will need reserved names.
  - P2: a finish event could be lost between the snapshot and listener registration. The app now registers the listener first, then takes the snapshot; a registration failure shows an alert.
  - P2: an unlistable folder became an empty snapshot, so a skipped file could show as a false Done. Snapshot errors now fail the item; a missing folder is still fine.
  Regression tests: 1 queue test, 1 snapshot test, 3 UI tests (the listener-order test was mutation-checked). `cargo test` 37/37 (+3 real-sidecar opt-in tests passing), `npm test` 52/52.
- #4 `codex-qa` re-review: **FAIL** (`qa/reports/20261010-184857-review-McuG.md`):
  - P1: yt-dlp's merge temp files in the user's folder could clobber `Foo.temp.webm`.
  - P2: a Windows non-UTF-8 stdout path could break success detection.
  - P2: NFC/NFD-equivalent names slipped past the name snapshot.
  All three came from yt-dlp writing straight into the user's folder, so the design changed to the staged download above; the name snapshot and stdout parsing are gone. Fake-yt-dlp tests were rewritten for the staged flow (14, including lookalike `Foo.temp.webm`/`Foo.f251.webm` untouched, a macOS case/NFD collision refused by the OS, a missing out dir, and a stale work dir refused). New opt-in real YouTube test (`QUEUE_QA_NETWORK=1`). `cargo test` all pass (+4 real-sidecar opt-in tests, including the YouTube merge leaving only the finished file), `npm test` 52/52.
- #4 `codex-qa` re-review on the staged design: **FAIL**, then fixed (`qa/reports/20261010-185524-review-bqWF.md`). P2: a failed listener registration left downloads enabled, so a result could never arrive and the row stayed Downloading, blocking the rest. Download now stays disabled until the listener is up, and a registration failure shows a persistent "Restart Queue" warning that a later successful add doesn't clear. The copy fallback is split into `copy_no_clobber`, with direct tests (copy, refuse existing, no partial file on failure). +2 UI tests; `npm test` 54/54, `cargo test` 41 (+4 opt-in).
- #4 owner test (2026-10-10): the owner downloaded a real video link in the dev app on macOS; it worked.
- #4 final `codex-qa` re-review: **PASS WITH NOTES**, with no actionable findings (`qa/reports/20261010-185811-review-IzBd.md`). Suggested follow-up tests (not blocking): Windows executable fixtures, a forced cross-volume `place`, injected copy/sync failures, StrictMode listener cleanup. Codex's sandbox blocked its Rust and 17 Node tests; the full suites ran locally.
- #3 (macOS arm64): `cargo test` 8/8 (3 new queue tests), clippy and fmt clean, `npm run build`, `npm test` 33/33. In the real app (`tauri dev`, driven with System Events and checked by screenshots): Enter adds a row and clears the field; **+** adds trimmed input; **+** is disabled when the field is empty; Enter on an empty field does nothing; typing right after **+** lands in the field.
- #3 `codex-qa test`: **FAIL**, then fixed (`qa/reports/20261010-173134-test-MZBN.md`). Two UI races, both reproduced with real React DOM and delayed mock IPC:
  1. A slow `queue_add` reply cleared a link typed meanwhile. The field now clears only if it still holds the submitted text.
  2. A late initial `queue_list` snapshot replaced a row added before it arrived. Rows are now merged by id.
  Codex's tests are merged with the owner's approval: `qa/tests/queue-ui.test.mjs` (7 jsdom interaction tests) and `src-tauri/tests/queue_q1.rs` (6). **jsdom 30.1.2** is a new, exact-pinned devDependency (test-only, never shipped; owner approved). `npm test` 40/40, `cargo test` 14/14.
- #3 `codex-qa review`: **FAIL**, then fixed (`qa/reports/20261010-174447-review-cdVo.md`). Same class of race:
  - P2: a pending add reply could duplicate a row already in the snapshot.
  - P2: out-of-order replies rendered out of order.
  - P3: focus waited on IPC.
  Fixed by a single pure `mergeItems` (`src/queue.ts`) used for every reply (one row per id, ascending id), and by focus returning before the await. Three regression tests were added. Mutation-checked on a scratch copy: each fails against the old code. They also resolve their pending fake replies, so a failure can't hang. `npm test` now has `--test-timeout=30000`, after two hung runs during the mutation check had to be killed (no leftovers).
- #3 `codex-qa` re-review: **PASS WITH NOTES** (`qa/reports/20261010-175901-review-Tz8C.md`), with no actionable findings. It didn't explicitly cover a failed add racing a successful one, so I added a test for that case: the good row stays, the error shows, newer text survives, and later adds work and clear the error. `npm test` 44/44, `cargo test` 14/14, clippy and fmt clean.
- #2 (macOS arm64 host):
  - `build-ffmpeg.sh` for aarch64 (30 s) and x86_64 (cross-compiled): LGPL, about 3.5–3.9 MB per binary, linking only system libraries (libz, CoreFoundation/Video/Media). The tarball SHA-256 was verified.
  - `fetch-sidecars.mjs --check` passes for both macOS targets (x64 via Rosetta): `yt-dlp stable@2026.08.19`, `ffmpeg 9.0.2, ffprobe 9.0.2`, `deno-2.9.7`.
  - `cargo clippy -D warnings` clean; `cargo test` 2/2; `npm test` 18/18; actionlint clean.
  - Release `tauri build --bundles app,dmg`: all 4 sidecars are in `Contents/MacOS` and pass the check from there. The `.dmg` is 80.8 MB and the app 123 MB installed (deno 81 MB), well within the new 256 MB budget (D-013).
  - Not verified: a real download, merge, `-x`, or thumbnail run with this ffmpeg (that's #4).
- #2 hosted run on PR #35 (38087616380): **all green on the first run**.
  - ffmpeg 9.0.2 LGPL built for all 4 targets, including Windows via mingw. `--check` passed in the check job and in every bundle job: macOS arm64, macOS x64 (Rosetta), Windows x64, Linux x64.
  - Installer sizes, all under the 256 MB budget (D-013): dmg 77 MB (arm64) / 81 MB (x64), NSIS 49 MB, MSI 62 MB, deb/rpm 84 MB, **AppImage 155 MB** (bundles WebKitGTK; the least headroom for llama-server in #30).
- #2 `codex-qa test`: **FAIL**, then fixed (`qa/reports/20261010-152603-test-GeC9.md`). Two bugs:
  1. `build-ffmpeg.sh` put the manifest path inside JS source, so a checkout path containing an apostrophe broke it. The path is now passed as an argument.
  2. `fetch-sidecars --check` ignored a timeout or spawn error when the header had already printed. It now requires yt-dlp's no-URL exit status 2, with no error or signal.
  Codex's tests (12 behavior tests with stubbed network and spawn, 2 path tests, 3 Rust path tests) reproduce both bugs and pass with the fixes. They're in the working tree pending the owner's decision. `npm test` 32/32, `cargo test` 5/5, real `--check` still OK.
- #2 `codex-qa review`: **FAIL**, then fixed (`qa/reports/20261010-153213-review-dROg.md`). P1: `build-ffmpeg.sh` ran `rm -rf` on the build dir before validating the target, so `aarch64-apple-darwin/../../../..` would have deleted the checkout. The target allowlist now runs first, and a regression test with marker files covers it. `npm test` 33/33; a real arm64 build and `--check` still pass. The re-review was **PASS WITH NOTES** (`qa/reports/20261010-153638-review-6yFv.md`); its only finding, a trailing blank line in STATUS, is fixed.
- #29: actionlint 1.7.12 is clean and `npm test` passes 6/6 locally. The workflow hasn't run on GitHub yet; the first run will be on the draft PR.
- #29 first hosted run (PR #34): checks and the macOS arm64/x64 and Linux bundles passed, and all 9 artifacts uploaded (Windows msi/nsis included). The Windows job then failed in `npm test`: git's CRLF checkout broke the line-based config tests. Fixed with `.gitattributes` (`eol=lf`) plus CRLF normalization in the tests. The failure was reproduced locally on a CRLF copy (2 fail), and the fix passes it (14/14).
- #29 second hosted run (0c928b5): **all green**. check 80s; bundles: macos-arm64 114s, macos-x64 124s, linux-x64 219s, windows-x64 729s; 9 artifacts uploaded. That's about 80 billed minutes warm and about 110 cold; D-011 is updated to match.
- #29 final `codex-qa review --base main`: **PASS WITH NOTES**, no findings (`qa/reports/20261010-113126-review-WrJi.md`). Separately confirmed: `.gitattributes` leaves the icons alone (git classifies them as `-text`), and the downloaded macOS artifacts are `x86_64` and `arm64` Mach-O respectively. Nobody has installed or launched the Windows, Linux, or Intel builds yet.
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
Run the M1 `codex-qa audit` (CLAUDE.md gate 3) to close M1, then fix or log its findings. After that, start M2, beginning with #36 (Settings screen) + #10 (folder picker) + #39 (D7: Open folder / Show in folder). Open owner question: an optional "most compatible (MP4/H.264)" preset (possibly with #11), since D-007's default often yields `.webm`, which QuickTime and Finder preview can't play.
