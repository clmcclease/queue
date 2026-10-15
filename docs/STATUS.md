# Status

**Updated:** 2026-10-14 · **Milestone:** M1 done · M2 (real queue) in progress · **Active:** #41 (native tests in CI + Linux box), PR #57

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

## Done: #5 (Q2) validate URLs, reject duplicates (squash-merged from PR #47)
- `src-tauri/src/link.rs` (pure) — **validation:**
  - only `http`/`https` with a host, starting with a literal `http(s)://`, so the text yt-dlp gets is exactly what was validated;
  - a bare `example.com/…` gets `https://` added;
  - rejected: inner whitespace (two links on one line; multi-line paste is #13), control characters, backslashes, and username/password links;
  - the row shows the typed text, not punycode.
- **Duplicate rule (`dedupe_key`):**
  - YouTube watch, youtu.be, shorts, live, embed and music links collapse to `youtube:<id>`;
  - every other link keeps its query **exactly**: parameter order kept, only `utm_*` removed, `+` literal, and byte-exact percent-decoding re-encoded canonically (RFC 3986 normalization only: escape case (`%3d` = `%3D`) and unreserved escapes (`%7e` = `~`) are equal; `+`, `%2B` and `%20` are distinct);
  - scheme, `www.`, default port, fragment and trailing slash are ignored.

  A missed duplicate is better than rejecting a distinct download.
- `Queue::add` returns `AddError` (`invalid` / `duplicate { id }`), serialized as `{kind, message, id}`. A failed row doesn't block re-adding its link.
- UI: invalid → inline alert, text kept; duplicate → 3 s toast, existing row highlighted, field cleared.
- Deps: `url` =2.5.8 (already in the tree via Tauri) as a direct dep; `serde_json` =1.0.151 as a dev-dep.
- Checks: `cargo test` all pass (24 lib, including 7 link tests and 5 new queue tests); Codex's Q1 tests updated for Q2; `npm test` 56/56 (+2 Q2 UI tests).
- `codex-qa test`: **FAIL**, then fixed (`qa/reports/20261010-225648-test-W3wi.md`):
  1. Encoded delimiters collided (`a=1%26b%3D2` vs `a=1&b=2`).
  2. `mailto:` was accepted as `https://mailto:…`.
  3. A literal NUL was accepted.
  Fixes: the URL parser decides whether there's a scheme (with a dotted host:port heuristic); control characters are rejected; query keys split on real `&`, then decode and re-encode each part canonically (`+`/`%20`/`%3d` compare equal, `%26` stays in the value). Codex's tests are merged: `src-tauri/tests/queue_q2.rs` (13) and 5 UI tests. All cargo tests pass; `npm test` 61/61.
- `codex-qa review`: **FAIL**, then fixed (`qa/reports/20261010-230240-review-EepI.md`): stripping `feature=` everywhere, sorting repeated parameters, and lossy UTF-8 decoding each merged distinct links. Unknown sites now keep parameter order and all non-`utm_*` parameters, with **byte-exact** canonical percent-encoding (`%FF` ≠ `%FE` ≠ `%EF%BF%BD`; `+` = `%20`, `%3d` = `%3D`). YouTube still collapses to the video id. Two of Codex's earlier test-run cases encoded the opposite (merge-everything) rule; they were updated to the safer rule. All cargo tests pass; `npm test` 61/61.
- `codex-qa review` round 2: **FAIL**, then fixed (`qa/reports/20261011-083115-review-o0rl.md`): repaired malformed authorities (`https:example.test`) were validated in one form and sent to yt-dlp in another, so a literal `http(s)://` prefix is now required and backslashes and userinfo are rejected; `+` was equated with space on unknown sites and is now literal; and a stale STATUS bullet was rewritten.
- `codex-qa review` round 3: **FAIL**, then fixed (`qa/reports/20261011-083711-review-fMtl.md`): `https:///host` still passed (a host must now follow `//` directly), and empty `&&` query pieces were dropped from the key (they're now kept). All cargo tests pass; `npm test` 61/61.
- `codex-qa review` round 4: **FAIL** (one P2), then fixed (`qa/reports/20261011-093728-review-Y8qS.md`): `+` collided with `%2B` because `canonical()` decoded every escape. It now does plain RFC 3986 normalization, decoding only escapes of unreserved characters and upper-casing the rest, so `+`, `%2B` and `%20` stay distinct while `%2b` = `%2B`. Tests were added for values and names. All cargo tests pass; `npm test` 61/61; clippy clean.
- **Review gate:** the owner accepted it after round 4's fix (2026-10-11) on the strength of the local checks; no fifth review was run.
- This branch also records D-015 (MP4 preset) and D-016 (provisional model) plus the PRD, BACKLOG and STATUS updates, per the owner's 2026-10-11 decisions.

## In progress: #41 (NFR) native tests on Linux, Windows and macOS, plus the Linux box (PR #57, D-023)
The owner asked for tests that actually run and pass on all three OSes, using hosted runners and their Ubuntu box. The owner's choices: keep Rust pinned (D-010); extend `build.yml` as #41 rather than add a separate `ci.yml`; set up the box as a runner, with OK for sudo and the runner registration.
- **CI (`build.yml`):** `check` is now a `test` matrix (`ubuntu-24.04`, `windows-2025`, `macos-26` arm64; `fail-fast: false`) with each OS's own sidecars. It runs `npm run build`, `npm test`, fmt, clippy `-D warnings` and `cargo test --locked`. Bundles need `test`. There's no xvfb (no test needs a display).
- **Linux box:** `chrisbeast` (Ubuntu 26.04, Ryzen, 16 threads, 192.168.4.83; KVM console at 192.168.4.69).
  - Packages and the `gh-runner` user (no sudo, rustup) were set up by a one-time root script the owner ran.
  - Actions runner v2.338.0 (SHA-256 checked) is a systemd service labelled `self-hosted, linux, ryzen`. GitHub shows it online.
  - The repo is private and runs no fork-PR workflows.
  - The opt-in `test-box` job (a manual run, or a PR labelled `ci:box`; never forks) passes on the box. It needed `~/.cargo/bin` on PATH, because the service has no login profile.
- **`scripts/test-linux.sh`:** rsyncs the working tree to the box and runs the same checks. It passes from the Mac. Section C of `docs/QA.md` covers the box.
- **Bugs the new runs found, all fixed:**
  1. Windows clippy: `proc.rs` imported Unix-only time types.
  2. The "Text file busy" flake (open problem 5). It reproduced about 20 times in 15 runs on the 16-thread box. `proc::spawn_tree` now retries ETXTBSY briefly on Unix; 0 failures in 75 runs after the fix.
  3. Windows: a test stepped 1 ns at the info-freshness cutoff, but Windows' SystemTime counts 100 ns. It now steps 1 µs.
- **Checks:** PR #57 is green on `test (linux)`, `test (windows)`, `test (macos)` and `test-box`. Local: `cargo test` 220, `npm test` 140/140, actionlint and shellcheck clean.
- **Spending limit hit (2026-10-14):** after the bundle run, GitHub refused to start jobs: "recent account payments have failed or your spending limit needs to be increased". The owner chose "Linux on the box, rest gated". A `plan` job (`scripts/ci-plan.sh`, tested by `ci.test.mjs`) now puts Linux on the box when it's online (or labelled `ci:box`), with hosted Linux otherwise. Windows and macOS, and the macOS ffmpeg build, run only on `main`, manual runs, and PRs labelled `ci:native` or `ci:bundle`. Detecting the box needs a `RUNNER_STATUS_TOKEN` secret, which the owner creates; without it, hosted Linux runs.
- **Not done in this PR:** #41's Windows fake-yt-dlp fixture. The Unix-only subprocess tests still skip on Windows.

## Done: #39 (D7) Open folder and Show in folder (squash-merged from PR #56; 4-OS bundles green)
- **Open folder** (next to "Saving to"): `open_download_folder` opens the current download folder with `reveal::open_folder` (see the review rounds below for each OS). A chosen folder that's gone gives D-020's "isn't available" reason.
- **Show in folder** on Done rows: `reveal_download(id)` looks up the row's file with `Queue::saved_file` (Done rows only), checks it still exists ("… isn't there anymore. It may have been moved or deleted."), then calls `reveal::reveal`, which selects the file. On Linux without `FileManager1`, it opens the folder and returns a notice that the UI shows as a toast.
- **Scope (D-022):** both commands take no path from the UI: the folder is the current download folder, and a reveal takes a row id. `tauri-plugin-opener` (=2.7.0) is a Linux-only dependency, used for one Rust function and never registered, so the webview gets no opener permission. All blocking work runs on worker threads.
- **Per-OS wording:** `src/platform.ts` (`detectOs`, `wordingFor`) gives "Show in Finder" / "Show in File Explorer" / "Show in folder" and the tooltip "Open the download folder in Finder / File Explorer / your file manager". It also holds the ⌘, / Ctrl+, shortcut from #36.
- **Checks:** `cargo test` 210 passed; clippy/fmt clean; `npm run build` OK; `npm test` 128/128 (macOS arm64). The file-manager behavior needs a manual check on each OS; CI builds all four with `ci:bundle`.
- `codex-qa test "#39"`: **PASS WITH NOTES**, no bugs (`qa/reports/20261013-201159-test-nrRC.md`). Its tests are added (D-021): `folder_d7_qa.rs` (6) and `folder-d7-39-qa.test.mjs` (9). `cargo test` 216 passed; `npm test` 137/137; clippy/fmt clean.
- `codex-qa review` round 1: **FAIL**, then fixed (`qa/reports/20261013-201902-review-J6cA.md`):
  1. Both commands ran on the main thread, so a slow network folder or a stuck Linux file manager could freeze the window. They're now async: filesystem checks, `open_path`, and the Windows/Linux reveal run in `spawn_blocking`. The macOS reveal goes to the main thread with `run_on_main_thread`, as AppKit requires, and returns at once.
  2. Linux: opener 2.7.0's portal fallback is broken (service and interface names swapped, and it passes a URI instead of a file descriptor), and 2.7.1 only changes iOS. If `FileManager1` can't select the file, Queue now opens the file's folder with `open_path` (xdg-open) instead of failing.
  The non-macOS branch can't be compiled locally (a Windows cross-check needs Windows resource tools). Linux `check` and the `ci:bundle` Windows build cover it. `cargo test` 216, `npm test` 137/137.
- `codex-qa review` round 2: **FAIL**, then fixed (`qa/reports/20261013-202339-review-kw59.md`):
  1. The macOS reveal still canonicalized the path on the main thread, inside opener.
  2. On Windows, opener 2.7.0 drops most `SHOpenFolderAndSelectItems` errors and reports success.
  The new `reveal.rs` does its own reveal on each OS, all on a worker thread:
  - macOS: `/usr/bin/open -R <path>`, args as an array, with failure from the exit status.
  - Windows: `SHOpenFolderAndSelectItems` through `windows-sys` (features `Win32_System_Com` and `Win32_UI_Shell_Common` added to the same pinned 0.61.2), with the HRESULT reported.
  - Linux: opener's `FileManager1` reveal, falling back to opening the folder.
  `reveal.rs` was type-checked and clippy-clean for `x86_64-pc-windows-msvc` in a scratch crate; Linux can't be cross-checked here (GTK system libraries), so CI `check` covers it. New macOS test: a failed reveal is reported. `cargo test` 217.
- `codex-qa review` round 3: **FAIL** (`qa/reports/20261013-202848-review-FCeh.md`):
  1. The Linux fallback opened the folder, without selecting the file, yet reported success.
  2. Opener's detached folder launchers report success before checking anything.
  The owner chose "fix both now". `reveal.rs` now owns every launch, and each reports its failure:
  - macOS: `open <dir>` and `open -R <file>`, checking the exit status.
  - Windows: `ShellExecuteExW` with the "explore" verb, `SEE_MASK_FLAG_NO_UI` and `SEE_MASK_NOASYNC`, plus `GetLastError`; reveal uses `SHOpenFolderAndSelectItems`.
  - Linux: `xdg-open <dir>`, checking its exit status (round 4 removed a 10 s cutoff). Reveal tries `FileManager1` and otherwise opens the folder through that checked path and returns a notice ("Opened the folder, but your file manager couldn't select …"), which the UI shows as a toast, not an error.
  The opener plugin is now a Linux/BSD-only dependency. More `windows-sys` features, same pinned version: `Win32_UI_Shell`, `Win32_System_Registry`. Cross-checks in a scratch crate: Windows clippy is clean, and the Linux branch is clippy-clean on the host with its cfg swapped. New tests cover launcher exit status, arguments passed byte for byte, macOS open/reveal failures, and the toast notice. `cargo test` 219; `npm test` 138/138.
- `codex-qa review` round 4: **FAIL**, then fixed (`qa/reports/20261013-212009-review-qS6R.md`):
  1. A launcher still running after 10 s counted as success and was never reaped. `launch` now waits for the real exit status, with no timeout. It runs on a worker, so the window never blocks, and the child is always reaped.
  2. Windows Open folder called the shell without COM. A `with_com` helper (STA, balanced `CoUninitialize`) now wraps both shell calls.
  New test: a slow launcher that fails is reported as a failure. `cargo test` 220; Windows and Linux branches clippy-clean in the scratch crate.
- `codex-qa review` round 5: **PASS WITH NOTES** (`qa/reports/20261013-213127-review-T6IQ.md`). Its P3 (STATUS and BACKLOG still described the old opener-based design) is fixed. Its suggested tests, none blocking:
  - backend-command tests (missing id, deleted file, unavailable folder, launcher failure);
  - Windows HRESULT, Unicode and UNC cases;
  - Linux `FileManager1` and fallback cases;
  - native happy-path checks on each OS.

## Done: #36 (PRD §6.4) settings screen (squash-merged from PR #55)
- **Panel, not a window:** the PRD asks for "a single window". A ⚙ button next to the link field opens Settings over the queue, as does ⌘, on macOS or Ctrl+, elsewhere; the tooltip shows the right one.
  - It's a `role="dialog"` with `aria-modal`, and the queue behind it is `inert`.
  - Focus moves to ✕ on open and returns to the opener on close. Escape or ✕ closes it.
- **First section, Downloads:** the download-folder control (D1). The folder state moved into `useDownloadFolder`, which the app calls once and shares between the main "Saving to" line and Settings, so both agree and the folder is read once. `FolderPicker` still works standalone, as Codex's D1 tests use it.
- Only working controls are shown; later items add sections (#11, #17, #25/#31/#32, #21, #20).
- **Owner check (dev app):** ⚙ opened Settings and the folder change worked, but Escape did nothing. It only worked while focus was inside the panel, and a mouse click on the panel background drops focus to the body. Fixed: a window-level Escape listener closes Settings, with or without ⌘/Ctrl, plus a new test. Retested by the owner: Escape closes Settings, and ⌘, opens it.
- `codex-qa test "#36"`: **FAIL**, then fixed (`qa/reports/20261013-193855-test-s3D5.md`). Under React StrictMode (used by `main.tsx`), the panel's mount effect ran twice. The replay recorded the ✕ button as the opener, so closing left focus on the body instead of the gear. Now `App` records the opener before the queue goes inert and restores focus after it isn't. Codex's tests (`settings-screen-36-qa.test.mjs`, 14 tests) are merged. From now on, QA-written tests are added by default, minus their STATUS edits (D-021; CLAUDE.md and docs/QA.md updated). `npm test` 124/124.
- `codex-qa review`: **PASS WITH NOTES**, with no findings (`qa/reports/20261013-194921-review-afj4.md`). Its suggested tests, none blocking: native Tab traversal while Settings is open, cancelling the native picker with Escape, and queue events arriving while Settings is open.
- **Checks:** `npm run build` OK; `npm test` 124/124. No Rust changes (macOS arm64).

## Done: #10 (D1) output folder picker, remembered (squash-merged from PR #54; 4-OS bundles green)
- **Settings (D-020):** new `settings.rs`, which loads and saves `settings.json` in the app config folder. Saves are atomic; a missing or damaged file means defaults. `download_dir` is `None` for the OS Downloads folder.
- **Commands:** `settings_download_folder` returns `{ path, error }`. `settings_choose_download_folder` (async) opens the native folder picker (`tauri-plugin-dialog` =2.8.1) at the current folder, saves the choice, and returns `null` when cancelled.
- **Downloads:** `begin_download` and "Download all" use the chosen folder. A chosen folder that's gone fails with "The download folder … isn't available. Choose another with Change…." It is never recreated. Downloads already running keep the folder they started with.
- **UI:** a "Saving to <folder> · Change…" line next to "Download all" (`FolderPicker.tsx`, `src/settings.ts`). Errors show inline as literal text, and Change… is disabled while the picker is open.
- **Checks:** `cargo test` 201 passed; clippy/fmt clean; `npm run build` OK; `npm test` 98/98 (macOS arm64). The native picker needs a manual check in the dev app on each OS; CI only builds it.
- `codex-qa test "#10"`: **FAIL**, then fixed (`qa/reports/20261013-065434-test-J2uQ.md`):
  1. A slow startup read could overwrite a folder just picked.
  2. The same read could show its error after a successful pick. FolderPicker now ignores the startup read once a folder is chosen.
  3. `download.rs` still did `create_dir_all(out_dir)`, so a folder that vanished mid-download came back. `run_in` no longer creates the output folder; `lib::download_dir` creates only the default Downloads folder, up front. The M1 test `d2_missing_output_folder_is_created` now asserts the opposite (`…fails_and_is_not_recreated`).
  Codex's tests (`settings_d1_qa.rs` with 7 tests, `folder-picker-d1-qa.test.mjs` with 6) pass against the fixes in its worktree. The owner chose to add them: `cargo test` 208 passed; `npm test` 104/104; clippy/fmt clean. The real-sidecar tests caught two YouTube tests in `retry_d2.rs` that relied on the old folder creation; they now create their out folder. Real tests: 12/12.
- `codex-qa review` round 1: **FAIL**, then fixed (`qa/reports/20261013-070558-review-5bRq.md`). P2: two Queue processes saving at once shared `settings.json.tmp`. Each save now claims its own temp file (pid, counter, time) with `create_new`, syncs it, and renames it over `settings.json`; a failed save removes only its own temp. New test: 16 threads × 20 saves leave one complete choice and no temp files. Codex's two temp-file tests were adapted to the unique names (a read-only folder now forces the failure; a crashed save's leftover is ignored). `cargo test` 209 passed.
- `codex-qa review` round 2: **PASS WITH NOTES** (`qa/reports/20261013-071834-review-50NW.md`). Its P3 is fixed: cancelling the picker cleared an unresolved startup error; now only a successful pick clears it (new UI test). `npm test` 105/105. This last change is UI-only and came after the review.

## Done: #9 (Q6) remove + retry with readable reasons (squash-merged from PR #53)
- **Readable reasons:** `download::failure_reason` drops yt-dlp's `[extractor] id: ` lead-in, so the row says "Failed: Video unavailable" rather than "[youtube] dQw4w9WgXcQ: Video unavailable". If nothing would be left, it keeps the whole line. 403 detection is unchanged; it still matches "HTTP Error 403". Metadata failures use the same rule.
- **Remove (✕):** `Queue::remove` / `queue_remove` takes any row except Downloading/Processing (cancel is #22).
  - Removing a Done row leaves its file on disk. The row's saved `-J` info is deleted.
  - A row removed while Fetching ignores the late result, and `run_fetch_job` deletes the info file that fetch saved.
  - Ids are never reused. The UI keeps removed ids, so `mergeItems` drops late events and snapshots for them.
- **Retry:** `Queue::retry` / `queue_retry` only accepts Failed rows.
  - A row whose info fetch failed (no meta) goes back to Fetching and is re-dispatched to the metadata worker. Otherwise it goes back to Queued, for Download or "Download all"; a running "Download all" picks it up.
  - Retry is refused while the same link was added again and is still active.
  - Attempt and progress reset. The saved info was already consumed, so the download extracts again.
- **UI:** a Failed row shows Retry and ✕; other rows show ✕, except while downloading or processing. Errors appear in the alert line as literal text.
- **Checks:** `cargo test` 187 passed; clippy/fmt clean; `npm run build` OK; `npm test` 90/90; real tests 12/12 (macOS arm64).
- `codex-qa test "#9"`: **FAIL**, then fixed (`qa/reports/20261012-202328-test-lWk2.md`). An `ERROR:` line with an empty body produced an empty reason (the row said just "Failed:"). This predates #9. `failure_reason` now skips empty `ERROR:` lines and falls back to the exit status. Codex's tests (`…-lWk2.tests.patch`: `queue_q6_qa.rs` with 11 tests, plus 4 UI tests) pass against the fix in its worktree: 11/11 and 94/94. The owner chose to add them; with them, `cargo test` 198 passed and `npm test` 94/94.
- `codex-qa review`: **PASS WITH NOTES**, with no findings (`qa/reports/20261012-203221-review-tQv0.md`). It suggested more tests, none blocking:
  - `queue_remove` itself deleting info while keeping a Done file;
  - retrying a fetch failure during "Download all";
  - removing a row the runner just picked;
  - Retry disabled when the listener failed.
  Real tests were rerun after the empty-ERROR fix: 12/12. Codex's real-sidecar run failed only because of its sandbox (`semctl: Operation not permitted`).

## Done: #8 (D4) live progress + Processing (squash-merged from PR #52)
- yt-dlp runs with `--progress --newline` and two templates:
  - `download:QUEUE-PROGRESS {json}` (status, downloaded, total, total_estimate, speed, eta, fragment, fragments; `|null` defaults);
  - `postprocess:QUEUE-POST {json}`.
  Both were verified against the real yt-dlp.
- `download::parse_line` (pure):
  - Progress `{percent (bytes, else fragments; clamped), downloaded, total, speed, eta, part}`; `part` counts finished streams (video = 1, audio = 2);
  - `Processing(name)` when a postprocessor other than MoveFiles starts;
  - bad values are dropped and other lines ignored.
- Downloads now stream stdout (bounded 64 KB lines) through `proc::spawn_tree` (process group or Job Object, like metadata); stderr keeps a 64 KB tail on its own thread. The Windows CREATE_NO_WINDOW flag comes from spawn_tree.
- `run_with_retries_events` adds an event callback; `run_with_retries` keeps its signature.
- Queue:
  - `Status::Processing`, `Item.progress`, `progressed`/`processing`;
  - finish and retry accept Processing; `is_busy` and the runner count it.
- lib: progress throttled to ~4/s per item (a new part or 100% always goes through; state changes always do).
- UI: a progress bar (`role=progressbar`), "Downloading… 42% · 3.2 MB/s · 0:35 left (part 2)", and "Processing…".
- Tests:
  - 5 parser/line unit tests and 1 queue flow test;
  - a fake-yt-dlp streaming test;
  - an opt-in **real** test (part 1 + part 2 progress, then Merger, in 3.5 s);
  - 1 UI test.
  `cargo test` 160 passed; `npm test` 82/82.
- `codex-qa test`: **FAIL**, then fixed (`qa/reports/20261012-063510-test-Mxcf.md`):
  1. A retry hid the live numbers. `progressText(progress, attempt)` now shows "Retrying (2/3)… 25% · …".
  2. A `total` of 0 suppressed a valid `total_estimate`; zero now counts as unknown.
  3. A flaky `session_dir` collision on a coarse clock; a per-process counter is added to the name.
  Codex's tests are merged (`progress_d4_qa.rs` 9, `progress_stream_d4_qa.rs` 3, 3 UI tests, and an opt-in real loopback progress test). `cargo test` 172 passed; `npm test` 85/85.
- Opt-in real tests (macOS arm64, pinned sidecars, network on): `download_real_d2` 8/8, `retry_d2` 2/2, `meta_q3` 2/2. The loopback fixture now paces its media (1 KB every 20 ms). An instant transfer left yt-dlp's speed and ETA null, which failed Codex's ETA assertion.
- `codex-qa review` round 1: **FAIL**, then fixed (`qa/reports/20261012-064826-review-tyGd.md`). If yt-dlp exited while a descendant kept stdout or stderr open, the download waited forever and held the slot. Both pipes are now read on threads. Once yt-dlp exits, the pipes get 5 s (`DRAIN_GRACE`) to close; then the tree is stopped. The progress throttle is now a tested `Throttle` type. New tests: orphan holding stdout, orphan holding stderr, a success with a lingering descendant, and throttle rules. `cargo test` 176 passed; clippy/fmt clean; `npm test` 85/85; real tests still 12/12 (macOS arm64). Windows streaming is only covered by CI's build (fake-subprocess tests are Unix-only, #41).
- `codex-qa review` round 2: **FAIL**, then fixed (`qa/reports/20261012-065552-review-4k73.md`):
  - P2: progress events were buffered without limit. The channel is now bounded at 64; when it's full, progress is dropped (later updates replace it), and Processing waits for room.
  - P3: a repeated 100% skipped the throttle. Now only each part's first 100% does.
  New tests: progress flood with a slow consumer, repeated 100%, and a row failing while Processing doesn't stop the next one. `cargo test` 178 passed; clippy/fmt clean; real tests rerun.
- `codex-qa review` round 3: **PASS WITH NOTES** (`qa/reports/20261012-072413-review-NXPT.md`). Its P3 is fixed: under backpressure the final 100% could be dropped. The reader now keeps the newest progress that didn't fit and sends it before the next Processing and at EOF. New test: the final progress survives backpressure; the flood test also checks that the newest value survives and that order holds. `cargo test` 179 passed.
- `codex-qa review` round 4: **PASS WITH NOTES** (`qa/reports/20261012-073039-review-OjrZ.md`). Its P3 is fixed: a held-back update could wait for more output while yt-dlp was quiet. The channel is replaced by a small `Mailbox`, a bounded queue where consecutive progress for the same part merges into the newest one, so the newest value is ready immediately. A part's 100% gives way only to another 100%; Processing is never dropped; the reader waits when 64 events queue, and is released if the caller returns. Two ordering tests now allow an intermediate update to be merged. New test: the newest progress arrives during a pause after a burst. `cargo test` 180 passed (streaming tests repeated 10×); real tests rerun.
- `codex-qa review` round 5: **PASS WITH NOTES** (`qa/reports/20261012-080911-review-R9XZ.md`). Its P3 is fixed: lib.rs's throttle discarded a quick update, so a pause left the UI on the older value. The throttle moved into `run_in`'s loop (`download::PROGRESS_EVERY`). A held update stays in the Mailbox, where newer ones replace it, and goes out once due, with no further output needed. New tests: a throttled update is delivered during a pause; a full mailbox keeps order, and a hang-up releases a blocked reader. `cargo test` 182 passed (streaming tests repeated 10×); real tests rerun.
- `codex-qa review` round 6: **PASS WITH NOTES** (`qa/reports/20261012-081645-review-Pl5d.md`). Its only P3 was about a test: the held-update test could merge 10% and 60% before the first callback. Fixed: 60% is now printed only after 10% is delivered, and the test asserts 60% arrives at least `PROGRESS_EVERY` later. It passed 10/10 repeats; `cargo test` 182 passed. This last change touches only the test; the reviewed app code is unchanged.

## Done: #7 (Q4, Q5) Download all, with #16 N2 (squash-merged from PR #51)
- `queue::run_all(queue, download, wait)` (Tauri-free): repeatedly downloads the **next Queued row (lowest id)**.
  - Rows still Fetching are waited for, but don't block rows that are ready.
  - A failure doesn't stop the run (Q6), and rows added mid-run are picked up.
  - If a manual Download holds the one slot, it waits; it stops when nothing is queued, fetching, or downloading.
- `lib.rs`: the download logic is split into `begin_download` (start + paths) and `complete_download` (blocking; retries, finish, cleanup, events). These are shared by the per-row `queue_download` (on a thread) and `queue_run_all`, which runs `run_all` on a thread guarded by `Runner(Mutex<bool>)`. `set_running` changes the flag and emits the event under that lock, so only one run happens at a time and runner events stay ordered. It emits `queue://runner {running}`; there's also `queue_runner_state`.
- UI: a **Download all** button, disabled without queued rows, before listening, or while running ("Downloading all…"). Per-row Download is disabled during a run.
- States: Naming and Processing from the issue's list arrive with #14/#25 and #8 (progress parsing). Today's Fetching/Queued are its FetchingInfo/Ready.
- Tests: 5 runner unit tests (order, continuing past failure, waiting for fetching, a ready row not held up, a manual-download slot, rows added mid-run) and 3 UI tests. The test mount helpers answer `queue_runner_state`. All cargo tests pass; `npm test` 74/74.
- **Owner test (2026-10-11):** Download all worked and picked up a link added mid-run, but the first row failed with "Not saved: … already exists". The owner asked for "keep both", so **N2 / #16 is pulled forward into this change** (D-018): `download::numbered` + `place_unique` place the file as `Title (2).ext`, `(3)`, … (≤ 999), each candidate claimed atomically. All collision tests (fake and real yt-dlp) were updated to expect ` (2)` with the original untouched.
- `codex-qa test "#7"`: **FAIL**, then fixed (`qa/reports/20261011-192723-test-IeHf.md`), three UI races:
  1. A late `queue_run_all` reply revived a finished run. "Running" now comes only from `queue://runner` events.
  2. A stale startup snapshot overrode a newer event. The snapshot now applies only if no runner event has arrived.
  3. A runner-listener failure was cleared by a successful add. A failure to subscribe to either event stream is now the persistent "disconnected" state.

  Its strict-order note (ready rows overtake rows still fetching) is kept by design (D-019). Codex's tests are merged (`queue_q4_qa.rs` 7 and 7 UI tests; one updated for D-018). `cargo test` 146 passed; `npm test` 81/81; the real-sidecar collision tests pass with ` (2)`.
- `codex-qa test "#16"`: **FAIL**, then fixed (`qa/reports/20261011-193704-test-fL5J.md`): a name at the 255-byte limit couldn't take " (2)" ("File name too long"). `numbered` now shortens the stem at a character boundary to fit 255 bytes. Its real-yt-dlp test also showed that a **250-char title failed even on the first download** (yt-dlp's `.part` suffix went over the limit), so downloads now pass `--trim-filenames 200`. Codex's tests are merged (`collision_n2_qa.rs` 7, plus 2 real tests). `cargo test` 153 passed; all 9 opt-in real-sidecar download tests pass; `npm test` 81/81.
- `codex-qa review`: **FAIL**, then fixed (`qa/reports/20261011-194556-review-ZuR9.md`):
  - P2: a lasting start error (no Downloads or cache folder) made Download all retry one row every 250 ms forever. Now `queue_run_all` preflights the folders and yt-dlp and returns a readable error, and a row that can't start while the slot is free is **failed** (`Queue::fail_queued`) so the run moves on. Only real contention (`Queue::is_busy`) waits.
  - P2: a stale "finished" runner event could override a newer run. The runner is now a `Mutex<bool>`, and each change and its event are emitted under that lock (`set_running`). The run thread is wrapped in `catch_unwind`, so the flag always resets.
  A new runner test covers the failed row and the run continuing.
- **Owner test (2026-10-11):** a playlist-only link timed out after 90 s, because with `--no-playlist` yt-dlp still extracted every video. The `-J` fetch now passes `--flat-playlist`: a real 96-video playlist returns in 2.7 s and is refused with "Playlists aren't supported yet" (#27). A `watch?v=…&list=…` link still yields the single video (checked).
- `codex-qa` review round 2: **PASS WITH NOTES** (`qa/reports/20261011-195758-review-gSoc.md`); its two doc P3s (BACKLOG #7 lifecycle wording, this stale bullet) are fixed.
- `cargo test` 154 passed; the opt-in real tests pass (one `_MEI` count was skewed by the owner's dev app running yt-dlp at the same time; it passed on rerun); `npm test` 81/81.

## Done: #48 (D-017) auto-retry 403s and reuse fetched info (squash-merged from PR #50)
- The metadata worker saves the raw `-J` to `<app cache>/info/session-<pid>-<nanos>/item-<id>.json` (`meta::fetch_raw` + `save_info`, per-run namespace, `create_new`), and the queue records it with its fetch time (`set_info`/`fresh_info`/`take_info`, internal only).
- `download::run_with_retries`:
  - attempt 1 uses `--load-info-json <file>` when the saved info is under **5 h** old (`INFO_FRESH_FOR`), and `-- <url>` otherwise;
  - on a yt-dlp-reported `HTTP Error 403` it waits 10 s, then 30 s, and retries with a **fresh URL extraction**, up to 3 attempts in total;
  - all attempts share **one** fresh per-download work dir, so finished streams are reused (D-014 still holds); other errors aren't retried.
- `Item.attempt` (1, then 2–3; 0 when idle) is emitted on each retry, and the row shows "Retrying (2/3)…". The info file is deleted when the item finishes.
- Tests:
  - unit tests for freshness and expiry (including a clock going backwards and a missing file), attempt reset, the `--load-info-json` argv, and 403 detection;
  - `tests/retry_d2.rs`, 5 fake-yt-dlp tests: 403 then success, 3×403 giving up, no retry for other errors, info first then URL on retry, and an existing target not retried or overwritten;
  - an opt-in **real download from saved info** (6 s).
  `npm test` 69/69.
- `codex-qa test`: **FAIL**, then fixed (`qa/reports/20261011-153801-test-XLyJ.md`):
  1. A local collision on a file named "HTTP Error 403.webm" triggered retries. Only a 403 **reported by yt-dlp** is retryable now (an internal `Failure { reason, retryable }`).
  2. Finished parts were thrown away between retries. All attempts now share **one** fresh work dir (still per download, removed at the end).

  Sharing a dir means leftovers can't be allowed to confuse the result: yt-dlp now writes the final path with `--print-to-file after_move:filepath queue-result.txt` (UTF-8, in the work dir; confirmed in the pinned source), and Queue uses it only if it's a regular file inside the work dir. Otherwise it applies the strict rule, which now ignores `.part`/`.ytdl`/`.part-Frag*`. The 5 h wording is reconciled in BACKLOG and the issue. Codex's tests are merged (`retry_d2_qa.rs` 14, a real saved-info test in `download_real_d2.rs`, 2 UI tests). All cargo tests pass, plus **all 8 opt-in real-sidecar tests**; `npm test` 71/71.
- `codex-qa review`: **FAIL**, then fixed (`qa/reports/20261011-154757-review-Pkpr.md`):
  - P2: two app runs (e.g. dev + installed, which share the app identifier and cache) both wrote `info/item-1.json`, so one could download the other's video. The saved info now lives in a **per-run session dir** (`meta::session_dir`: `info/session-<pid>-<nanos>/`), written with `create_new` (`meta::save_info`), with a test. Stale session dirs are cleaned by #45.
  - P3: a stale STATUS bullet was fixed.
  All cargo tests pass (42 lib).
- `codex-qa review` round 2: **PASS WITH NOTES** (`qa/reports/20261011-161726-review-n6m6.md`). Its one P3 (saved info attached in a separate lock *after* the row became Queued, so a Download could start without it) is fixed after the review: `run_fetch_job` now attaches the info and marks the row Queued under one lock, with a test. All cargo tests pass (43 lib); `npm test` 71/71.

## Done: #6 (Q3) fetch metadata on add (squash-merged from PR #49; Windows `proc_tree` passed in CI)
- New rows start as **Fetching** (`Status::Fetching`). One worker thread (`fetch_worker`, started in Tauri's setup hook and fed through a channel) runs `yt-dlp <base args> --no-playlist -J -- <url>` per row, in order, with a 90 s limit (the process is killed when it's hit). It then emits `queue://item`.
- `meta.rs`:
  - pure `parse` → `Meta { title, channel (else uploader), duration, thumbnail (http(s) only), site }`; text is control-char-filtered and capped at 500 chars;
  - playlists are refused until #27;
  - `fetch` drains both pipes on threads, so large `-J` output can't deadlock.
- `Queue::fetched`: Fetching → Queued (with meta) or Failed (with the reason). Downloads still require Queued.
- UI: "Fetching info…", then a 16:9 thumbnail (lazy, `no-referrer`, loaded from the site's CDN), the title, channel · `h:mm:ss`, and the URL small underneath.
- `serde_json` moved from a dev-dependency to a dependency (same pin).
- Checks:
  - `cargo test` all pass: 31 lib, `meta_q3` 5 fake-yt-dlp tests (success, error reason, timeout kill, 1.5 MB output, playlist), and the opt-in **real YouTube `-J`** (title, channel, duration, thumbnail OK in 4.4 s);
  - Codex's earlier tests updated for the fetch step;
  - `npm test` 64/64 (+3 Q3 UI tests).
- `codex-qa test`: **FAIL**, then fixed (`qa/reports/20261011-102131-test-JLZ4.md`):
  1. Thumbnail URLs were truncated at 500 chars, breaking signed CDN links. URLs are now never truncated, only dropped if over 8 KB or containing control characters or whitespace.
  2. The 90 s limit didn't cover reading the pipes, so a descendant holding them could stall the worker.

  Fix for 2: yt-dlp now runs in its own **process group** (Unix). The pipes are read under the same deadline (via channels), and a timeout kills the whole group with `libc::kill(-pgid)`, since the onefile launcher spawns the real Python process. On Windows only the direct child is killed; tree cleanup goes with #22. `libc` =0.2.190 is pinned (Unix only; already locked via Tauri).
  Codex's tests are merged: `src-tauri/tests/meta_q3_qa.rs` (18) and 4 UI tests. A new test proves a descendant dies with the group. All cargo tests pass (the real YouTube `-J` too); `npm test` 68/68.
- `codex-qa review`: **FAIL**, then fixed (`qa/reports/20261011-102855-review-LofR.md`):
  - P1: unbounded pipe reads could exhaust memory. Now stdout is capped at 32 MB (overflow stops the tree early with a readable error), and stderr keeps a 64 KB tail.
  - P2: the Windows timeout only killed the direct child. New `src-tauri/src/proc.rs`: `spawn_tree` uses a Unix process group, or a **Windows Job Object** (KILL_ON_JOB_CLOSE); `terminate` is SIGTERM to the group, then SIGKILL after 3 s (no `_MEI*` leak), or `TerminateJobObject`.
  - P2: a dead worker left rows Fetching forever. A dispatch failure now fails the row; each job runs under `catch_unwind`; reader threads use `thread::Builder` (recoverable).
  Pinned: `windows-sys` =0.61.2 (Windows only; already locked via Tauri). New tests: overflow, stderr tail, recovery after a bad fetch, and an opt-in **real onefile timeout leaving no processes or `_MEI` dir**. The Windows Job Object code compiles only in CI (this PR uses `ci:bundle`).
- `codex-qa` re-review: **FAIL**, then fixed (`qa/reports/20261011-121202-review-KOJY.md`):
  - P2: on Windows the child ran before being put in the Job Object, so its children could escape. It's now spawned **CREATE_SUSPENDED**, assigned to the job, then resumed via a Toolhelp thread snapshot (`proc::windows::resume`).
  - P2: `catch_unwind` was a no-op under the template's release `panic = "abort"`. The release profile is now **`panic = "unwind"`**.
  - The worker logic was extracted into `run_fetch_job` / `dispatch_fetch`, with tests: a panicking job fails its row and the next works; a closed channel fails the row.
  - New `tests/proc_tree.rs` (cross-platform: the tree runs, and terminate closes the pipe only if descendants died). It runs locally on macOS and **in CI on the Windows runner** (a new bundle-job step).
  All cargo tests pass; `npm test` 68/68; actionlint clean.
- `codex-qa` review round 3: **FAIL** (one P2), then fixed (`qa/reports/20261011-121639-review-AjFX.md`): after the leader exited, a stdout overflow returned without stopping the group, and could wait on stderr first. Every post-exit error now terminates the tree, and the overflow is handled before stderr. A regression test covers a descendant flooding stdout after the parent exits (overflow reason in under 20 s, no survivor). All cargo tests pass.
- **Review gate:** the owner accepted it after round 3's fix (2026-10-11), with Windows verified in CI (PR labelled `ci:bundle`, which runs `proc_tree` on the Windows runner).
- Owner test (2026-10-11): many links in the dev app; metadata and thumbnails load and the list scrolls. **Some downloads fail with "HTTP Error 403: Forbidden".** This is the download path (#4), not #6. The verbose log shows `PO Token Providers: none` and `Detected experiment to bind GVS PO Token to video ID for web client`, so YouTube's PO-token enforcement is the likely cause. Cause found (`visionos` link rejection; yt-dlp doesn't retry 4xx). Decided as D-017 and filed as **#48** (auto-retry + `--load-info-json`), next after #6.

## Checks (latest)
- **macOS arm64 locally, at the #39 merge:**
  - `cargo test` 220, plus 12 opt-in real-sidecar tests (`QUEUE_QA_REAL_SIDECARS="$PWD/binaries" [QUEUE_QA_NETWORK=1] cargo test --locked --test download_real_d2 --test retry_d2 --test meta_q3 -- --ignored --test-threads=1`, from `src-tauri`; last run at #10), including a real YouTube DASH merge with live progress;
  - `npm test` 138/138; clippy, fmt and build clean.
- **Hosted CI:** green for every M1 and M2 merge; 4-OS bundles green at the #10 and #39 merges.
- **Not verified natively:** downloads, the folder picker and Open folder / Show in folder on Windows and Linux (CI builds them and runs the tests; the Rust download tests run on Linux only), and the native webviews on Windows and Linux.

## Open questions and problems
1. Default local model: **provisionally Qwen3-4B-Instruct-2507** (2.5 GB), with Qwen3.5-2B as the small option (D-016); N9 (#33) confirms it.
2. Windows code signing: buy a certificate, or accept SmartScreen warnings? (#24)
3. ~~MP4 preset?~~ **Decided (D-015):** an optional "Most compatible (MP4)" preset in #11, strict H.264 with an honest fallback notice, plus the VP9 decoder and AVI demuxer in ffmpeg.
4. M1 audit follow-ups, filed with the owner's OK:
   - **#41** native macOS/Windows download tests in CI (M2, P1);
   - **#42** D2 failure-path tests (M2);
   - **#45** staging-folder cleanup (M2);
   - **#43** Content Security Policy (M5, P1);
   - **#44** yt-dlp GPG signature checks (M5).
5. ~~Flaky Linux CI (`Text file busy`)~~ Fixed in #41: `spawn_tree` retries ETXTBSY.
6. `scripts/codex-qa` can leave its watchdog `sleep` running after Codex exits, holding the output pipe open, so callers waiting for EOF hang (seen in the #10 review). Not fixed yet.

## Owner checks in the dev app (macOS)
- 2026-10-13: live progress, "Processing…" and Retry worked on real downloads.
- 2026-10-13: #10's Change… opened the native picker. A download landed in the chosen (iCloud Drive) folder, and the choice was remembered after a relaunch.
- 2026-10-14: #39's Open folder opened the download folder in Finder, and Show in Finder selected the saved file (macOS).

## Next bounded action
#41 / PR #57: the owner fixes Actions billing and optionally adds `RUNNER_STATUS_TOKEN`. Then one `ci:bundle` run, `codex-qa test "#41"` and `review`, then ask for the squash-merge. Then decide whether #41 stays open for the Windows fake-yt-dlp fixture or gets a follow-up issue. Next item: #11 (D2 format presets, D-015).
