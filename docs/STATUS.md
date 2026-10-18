# Status

**Updated:** 2026-10-17 · **Milestone:** M1 done · M2 (real queue) in progress · **Active:** M2 audit follow-ups: #68 (in review), then #69, #70, #72, #74

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

## M2 audit: YELLOW (`qa/reports/20261017-141115-audit-yQjF.md`)
- Owner chose to fix all four findings in M2 before closing it, one at a time: **#67** stop downloads on quit, **#68** save-warning listener, **#69** codex-qa watchdog + stale docs, **#70** real video+audio merging in CI. #71 (native UI smoke checks on Windows/Linux) is filed for M5. #72 came out of #67's review (below).

## In review: #68 (Q7, D-025) the save warning can't be lost (branch `ui/68-storage-listener`)
- **The bug:** `App.tsx` swallowed a failed `queue://storage` registration, so a later save failure (a full disk, permissions) never showed D-025's warning.
- **Fix:** when registration fails, the app reads `queue_storage_problem` every 2 s and sets or clears the warning; the timer stops on unmount. A reply is used only if it's newer than the last one applied, and an event counts as newer than every read started before it. So a late reply can't hide or bring back a warning, and replies slower than the interval still land.
- **Tests:** a #68 test in `queue-ui.test.mjs`. Codex's `storage-listener-q7-68-qa.test.mjs` (10 tests): Unicode text, rejected reads, event precedence, StrictMode, cleanup, out-of-order replies. Plus 2 for review round 1's cases. Fault tests: removing the polling or the cleanup, or restoring either earlier ordering rule, fails them.
- `codex-qa test "#68"`: **FAIL**, then fixed (`qa/reports/20261018-113843-test-Jq9b.md`): older replies overwrote newer ones. Codex's tests are added (D-021).
- `codex-qa review` round 1: **FAIL**, then fixed (`qa/reports/20261018-114333-review-ndOG.md`): the first ordering fix dropped every reply slower than the poll.
- Round 2: **PASS WITH NOTES** (`qa/reports/20261018-114616-review-2Yej.md`). Its P3 (this handoff) is done. Suggested tests, not added: a registration that succeeds after unmount, and a recovery event while an older warning read is pending.
- **Checks (macOS arm64):** `npm run build`, `npm test` 198/198. Frontend only; the Rust suites were not rerun.

## Done: #67 (Q7, D2) stop downloads and metadata fetches when Queue quits (squash-merged from PR #73; 4-OS bundles green)
- **The bug:** on exit Queue saved the queue but never stopped yt-dlp/ffmpeg. On macOS/Linux they run in their own process group, so a download kept running after Queue quit, and a relaunch's #45 cleanup deleted its staging folder. Windows' job object (`KILL_ON_JOB_CLOSE`) already killed them.
- **`proc`:**
  - A global shutdown state (`stopping`, live trees, running jobs) under one lock. `spawn_tree` refuses once stopping.
  - `shut_down()`, `wait_for_trees`, `wait_idle`, and `job()` guards held by each download/fetch until its result is recorded.
  - Quit-aware waits: `Tree::wait_unless_quitting`, `recv_until`, `sleep_unless_quitting` (the 403 retry pause).
  - `Tree::terminate` waits up to 1 s until the tree is really gone (Unix: the group is empty; Windows: the job has no active processes).
  - `Tree::finish` stops leftover descendants right after a run, while the group is certainly ours. Drop only stops an unfinished tree whose leader is unreaped, so a reused group ID is never signalled.
- **Downloads/fetches:** their wait loops stop their own tree on quit. A failure during a quit is not recorded: the row stays Downloading/Fetching, so it comes back interrupted or re-fetches after a restart. A success is still recorded. The cross-volume copy stops between 1 MiB chunks and removes its partial file. The Download all runner starts nothing more.
- **Exit:** `shut_down` → `wait_idle(5 s)` → final save.
- **Tests:**
  - `tests/quit_q7_67.rs`: a hanging download with a pipe-holding descendant plus a fetch, quit; everything exits within 5 s; new work refused; jobs hold up the save; the copy aborts; successes recorded.
  - Codex's `tests/quit_q7_67_qa.rs` (6, each in its own process): closed stdout, orphans holding stdout or stderr, SIGTERM ignored → SIGKILL, restored rows, failed-spawn accounting.
  - `native_a_descendant_left_after_the_run_is_stopped_with_its_tree`.
  - `tests/support/native.rs` (the shared fixture builder and exit check; macOS/Linux zombies count as exited).
- **Fault tests:** removing each stop check, the tree count, `finish()`, the drop cleanup or the copy's quit check fails its test.
- `codex-qa test "#67"`: **FAIL**, then fixed (`qa/reports/20261017-142907-test-OxV3.md`). A download whose stdout closed, and metadata reader waits after the leader exited, ignored the quit; fixed with the quit-aware waits. Codex's tests are added. Its instant exit checks were relaxed to a bounded `exits_soon` (≤1 s), because a SIGKILLed process stays visible for milliseconds on macOS (1 in 4 runs; gone 200 ms later); 10/10 passes after.
- `codex-qa review` round 1: **FAIL**, then fixed (`qa/reports/20261017-144350-review-bzNG.md`): descendants that let go of the pipes escaped (now `finish`/drop cleanup), and a success during a quit was discarded (now recorded; job guards; the copy aborts).
- Round 2: **FAIL** (`qa/reports/20261017-145426-review-kbdi.md`). (1) A reused process-group ID could be signalled on drop: fixed (`finish` + the drop rule). (2) A write blocking past the quit wait, or a crash mid-copy, can leave a cut-off file under the final name. **Owner accepted it for #67** (D-014); it's **#72**, next in M2.
- Round 3: **PASS WITH NOTES** (`qa/reports/20261017-154848-review-OzO5.md`). Only the accepted #72 limitation, plus this handoff (now updated). Suggested tests not added: a quit after a copy has written bytes, the exit handler with a delayed successful download, and a quit during a real 403 pause.
- **Checks (macOS arm64):** fmt, clippy, `cargo test` 296 (14 ignored), real-sidecar loopback tests, `npm test` 185/185. **CI (run 39077927179):** green on Linux, Windows and macOS, plus all 4 bundles. The first run failed on Windows only in the detached-descendant test, because a Windows child inherits its parent's pipe handles even with null stdio. That test is now Unix-only (the job object covers Windows). The Windows quit tests then passed. **Owner in the dev app (macOS, 2026-10-17):** quit (⌘Q) in the middle of a real download. Afterwards I found no yt-dlp/ffmpeg process left, an empty `downloads` staging folder, and `queue.json` saved with that row still `downloading` (so it restores as interrupted) and the earlier finished row kept as done.

## Done: #65 (NFR) harden #41's native tests (squash-merged from PR #66)
- **Windows flake:** the two metadata-timeout tests now allow 3 s (`START`) for a freshly copied fixture to start, instead of 300/500 ms, and require finishing within 10 s (`PROMPT`). The fixtures run 30 s, so that still proves the timeout stopped them.
- **Orphan exit:** the held-pipe descendant writes a heartbeat every 50 ms, and the test checks it stops. Mutation check: with the Unix group kill disabled, the heartbeat check failed while the old time bound still passed.
- **Remote guard:** `test-linux.test.mjs` runs the extracted `GUARD` heredoc against a scratch HOME. Mutation check: breaking its `case "$real"` line fails the test.
- `codex-qa test "#65"`: **PASS WITH NOTES**, no bugs (`qa/reports/20261017-120012-test-5fpu.md`). Codex's tests are added (D-021): slow-start scenarios with per-OS PID-exit checks (`kill(pid, 0)` plus Linux zombies; Windows `OpenProcess`/`GetExitCodeProcess`), and `qa/tests/test-linux-65-qa.test.mjs` (4 tests: the whole script with local `ssh`/`rsync` stubs, no host contacted).
- `codex-qa review`: **PASS WITH NOTES** (`qa/reports/20261017-120543-review-W54q.md`). Its P3 (this handoff was stale) is fixed. Optional tests, not done: a descendant that keeps only stderr open, and a Unix descendant that ignores SIGTERM.
- **Checks (macOS arm64):** fmt, clippy, `cargo test` 288 (14 ignored), `npm test` 185/185. **CI (run 39055679396):** green on Linux, Windows and macOS on the first run; on Windows both timeout tests and the PID-exit test passed.

## Done: #42 (D2) download failure-path regression tests (squash-merged from PR #64)
- `download::place` and `copy_no_clobber` behave as before. Internally the hard link, copy and sync are separate steps (a private `Steps` struct of fn pointers, `STEPS` in production), so a test can fail any one of them.
- **Unit tests:** cross-volume fallback (injected EXDEV), a taken name never copies, mid-copy and sync failures remove the partial file and keep the source (the sync test runs the real copy, so dropping `sync_all` fails it), and a real cross-volume move against `QUEUE_QA_OTHER_VOLUME` (a folder on another drive; must be valid when set) or Linux's `/dev/shm` (skips when absent or read-only).
- **UI:** `qa/tests/listeners-d2-42.test.mjs`. Under StrictMode with delayed registration, oldest or newest first, exactly one listener per event survives, delivers completion, and unmount removes everything. It keeps its own registry because `@tauri-apps/api` 2.12.2's event mock never removes a listener (`unlisten` sends `eventId`, the mock reads `id`).
- **Owner's exFAT SSD (2026-10-17):** a hard link within it fails with ENOTSUP and one from the internal disk with EXDEV. The real cross-volume move passed with `QUEUE_QA_OTHER_VOLUME` there. With `TMPDIR` on it, real downloads passed 9/10. All failures were macOS `._` AppleDouble files counted by the tests (a file count, and staging folders, which in the app live on the internal disk). No Queue bug.
- Mutation checks: removing App.tsx's cancelled-unlisten line fails both listener tests; removing the sync call fails the sync test.
- `codex-qa test "#42"`: **PASS WITH NOTES**, no bugs (`qa/reports/20261016-193552-test-szg6.md`). Codex's tests are added (D-021): `place_failure_d2_42_qa.rs` (6) and 4 more StrictMode tests.
- `codex-qa review`: **PASS WITH NOTES** twice (`qa/reports/20261016-195947-review-XN6W.md`, `qa/reports/20261016-200220-review-0jGv.md`). Both P3s are fixed: the sync test replaced the whole copy step, and the Linux test failed rather than skipped on a read-only `/dev/shm`. Suggested tests, not done: a Download all run where placement fails for the first item, and runner/storage event delivery after the StrictMode replay.

## Done: #45 (D2) clean up staging left by an interrupted download (squash-merged from PR #63; D-014)
- `download::stale_work_dirs(cache)` lists earlier runs' leftovers in the app cache: `downloads/item-*` work dirs (D-014) and `info/session-*` saved `-J` folders (D-017, never reused after a restart). Only real folders with those names are listed, never files or symlinks (`DirEntry::file_type` doesn't follow links).
- In `lib.rs` setup, only when this copy holds the queue lock (so another copy's running download is never touched), the list is taken before this run creates its own folders and deleted on a background thread.
- **Checks:** `cargo test` 268 (new test: matching folders, Unicode names, plain files, other folders, and a symlink whose target survives); clippy/fmt clean (macOS arm64). Live: after the dev app restarted with this code, `~/Library/Caches/dev.mcclease.queue/downloads` and `info` were empty, both modified at the restart. The earlier contents weren't captured.
- `codex-qa test "#45"`: **FAIL**, then fixed (`qa/reports/20261015-220139-test-OrFe.md`). If `<cache>/downloads` or `<cache>/info` was itself a symlink, `read_dir` followed it and the cleanup deleted matching folders elsewhere. Now each container must be a real folder (`symlink_metadata`), or it's skipped. Codex's 8 tests are added (D-021), `stale_staging_d2_45_qa.rs`, including both regressions. `cargo test` 276.
- `codex-qa review` round 1: **FAIL** (P2, a TOCTOU race; `qa/reports/20261015-220649-review-BWJW.md`): `downloads` could be swapped for a symlink between listing and deleting. **Owner's decision: re-check and accept.** `download::remove_stale` re-validates the folder and its parent as real folders right before `remove_dir_all`, and setup uses it. New test: a container swapped for a symlink after listing is refused, and the outside data survives. The remaining sub-millisecond window, needing a process that can already write the user's cache, is an accepted limitation (D-014). The full fix (per-OS handle-based no-follow deletion) was declined as disproportionate. `cargo test` 277.
- `codex-qa review` round 2: **PASS WITH NOTES** (`qa/reports/20261015-221003-review-Yop9.md`). Its P3 is fixed: the QA integration tests now delete with `download::remove_stale`, the production path. Suggested tests, none blocking: startup without the queue lock, a candidate itself swapped for a symlink, and Windows junctions.

## Done: #13 (Q8) paste several links at once (squash-merged from PR #62)
- **UrlBar:** an `onPaste` handler catches text with more than one non-blank line (any line ending). It stops the field from joining the lines into one bad link and calls `onAddMany(lines)`. A single-line paste is unchanged, and anything typed in the field stays.
- **App `addMany`:** adds each line in order through the normal `queue_add`, so each validates, checks duplicates and fetches info as usual. It then shows a summary toast, e.g. "Added 2 of 4 links: 1 already in the queue, 1 not a link.", and names the lines that weren't links in the alert, literally, up to 3, each shortened to 60 characters.
- **Pure helpers** in `queue.ts`: `splitLinks` and `addManySummary`. The field's placeholder now says links can be pasted one per line.
- **Checks:** `npm run build` OK; `npm test` 166/166 (4 new). No Rust changes (macOS arm64).
- `codex-qa test "#13"`: **PASS WITH NOTES**, no bugs (`qa/reports/20261015-211352-test-JyIx.md`). Its 8 tests are added (D-021): `paste-q8-13-qa.test.mjs` (Unicode, duplicates, rejected IPC, order, races, removal, a 100-link paste). `npm test` 174/174. Native clipboard behavior in each OS's webview is untested.
- `codex-qa review`: **PASS WITH NOTES**, with no findings (`qa/reports/20261015-211925-review-NIVp.md`). Suggested tests, none blocking: a native clipboard paste in each OS's webview, and a batch through real Tauri IPC.

## Done: #12 (Q7) the queue survives restarts (squash-merged from PR #61; D-025; 4-OS bundles green)
- **Owner's choices:** keep finished and failed rows. An interrupted download comes back Queued with a note and waits; nothing starts by itself.
- **`queue.rs`:**
  - `to_json` and `from_json` handle a `SavedQueue {version: 1, items}`, keeping id, link, status, video info and notice. `from_json` rebuilds each duplicate key from the link, turns Downloading or Processing into Queued with `INTERRUPTED`, drops invalid links and repeated ids, and keeps ids counting.
  - `load` moves a damaged file aside to `queue.json.bad`.
  - A `changes` counter goes up on every saved-worthy change except progress.
  - `fetching()` lists the rows to fetch again.
- **`settings.rs`:** the atomic writer is now the shared `write_atomic`.
- **`lib.rs`:** the queue is loaded in `setup` from `app_data_dir/queue.json`, and restored Fetching rows go to the metadata worker. A saver thread runs `save_queue` every 0.5 s when `changes` moved, building the JSON under the lock and writing outside it, plus a final save on `RunEvent::Exit`.
- **UI:** a queued row shows its notice, which clears when it starts.
- **Checks:** `cargo test` 251; clippy/fmt clean; `npm run build` OK; `npm test` 157/157 (macOS arm64).
- **Owner check (dev app, 2026-10-15):** quit mid-download, and `queue.json` held the row as downloading. After relaunch the finished video was still listed, as chosen.
- `codex-qa test "#12"`: **FAIL**, then fixed (`qa/reports/20261015-195325-test-mDTX.md`):
  1. If moving a damaged `queue.json` aside failed (e.g. `queue.json.bad` was a folder), the next save destroyed the only copy. Now `set_aside` uses `queue.json.bad`, or a unique `queue.json.bad-<time>` if that's taken. If even that fails, `load_checked` reports the file isn't safe to save over, and `lib.rs` disables queue saving for that session.
  2. A saved id of `u64::MAX` made the next add overflow and panic. Now ids use `checked_add` (adding fails with "The queue is full…"), and restored rows with id 0 or `u64::MAX` are dropped.
  Codex's tests are added (D-021): `persistence_q7_12_qa.rs` (12) and `persistence-q7-12-qa.test.mjs` (2). One check was loosened, so a second damaged file goes to a unique `.bad-` name instead of overwriting the first. `cargo test` 263, `npm test` 159/159.
- `codex-qa review` round 1: **FAIL**, then fixed (`qa/reports/20261015-200125-review-wzOV.md`). Any read error counted as "no file, safe to save", so an existing `queue.json` that couldn't be read (permissions, a disk error) would be replaced on the first change. Now only `NotFound` means empty and writable; any other read error starts empty with saving off. New unix test: an unreadable file is never saved over. Changes made during a save are covered by the change counter: the saver records the count it snapshotted, so later changes are written on the next 0.5 s tick and at exit. `cargo test` 264.
- `codex-qa review` round 2: **FAIL** (P1), then fixed (`qa/reports/20261015-200445-review-RE6K.md`). Two running copies, such as a dev build and an installed app sharing app data, could overwrite each other's `queue.json`. The owner chose "one copy only":
  - The official `tauri-plugin-single-instance` (=2.5.1, 16 days old; the 2.5.3 and 2.5.4 releases were too new) is registered first. Launching again brings the open window to the front.
  - As a backstop, `lock_queue` holds an exclusive `std::fs::File::try_lock` on `queue.lock` for the whole run, and without it the queue is never saved.
  Checked live on macOS: a second launch of the dev binary exited by itself with code 0, leaving one process. New test: `only_one_running_copy_can_hold_the_queue_lock`. `cargo test` 265.
- `codex-qa review` round 3: **FAIL**, then fixed (`qa/reports/20261015-201834-review-08cD.md`). Tauri releases the single-instance guard before the app's final save, so a relaunch in that moment got no `queue.lock` and silently ran a session that never saved. Now startup waits up to 10 s for the lock (`lock_queue_waiting`) before loading, so it reads the exiting copy's final save. If it still can't get the lock, or a damaged or unreadable file blocks saving, the window shows a red alert that this session won't be saved (`queue_storage_problem`, `data-role="unsaved"`). Never silent. New tests: the wait, giving up, and the alert shown literally or absent. `cargo test` 266, `npm test` 161/161.
- `codex-qa review` round 4: **FAIL**, then fixed (`qa/reports/20261015-202346-review-Z9y7.md`). A save that failed after startup (disk full, folder became read-only) was swallowed silently. Now `persist()` reports it: the storage problem can change during a session, and a failed write sets "Queue couldn't save the list (reason)… It keeps trying." and emits `queue://storage`. The saved count doesn't move, so the next 0.5 s tick retries with every pending change, and the next good write clears the warning (emitting `null`). The window listens and shows or clears the alert live. New tests: Rust `a_failed_save_is_reported_retried_and_cleared_once_saving_works`, with an injected writer; UI warning shown, then cleared. `cargo test` 267, `npm test` 162/162.
- `codex-qa review` round 5: **PASS WITH NOTES**, with no findings (`qa/reports/20261015-202727-review-WSZ5.md`). Suggested tests, none blocking: a change made during a paused save lands in the next save; quitting before the first 0.5 s tick keeps the change; a slow startup status read can't hide a newer warning.

## Done: #59 (Q2 bug) re-adding a link whose download finished (squash-merged from PR #60)
- **Owner report (2026-10-15):** downloaded a music video with Audio only (`.opus`), then re-added the link to get Most compatible audio (`.m4a`). It was refused as a duplicate, because a Done row still blocked its link (only Failed rows were exempt, since #5).
- **Owner's decision:** a finished row isn't "in the queue" (PRD Q2). `Item::is_active()` (Fetching, Queued, Downloading, Processing) now decides duplicates in `Queue::add` and the retry guard. A finished link starts a new row with the current Format; a name clash saves as `Title (2).ext` (N2), never overwriting.
- **Tests changed to the new rule:**
  - `queue_q2.rs`: `done_rows_still_block…` becomes `only_active_rows_block_duplicates…`, now also covering Processing;
  - `queue_q6_qa.rs`: a finished copy no longer blocks a retry.
  New unit tests: a finished row doesn't block its link (written another way) while an active copy does; and a retry is allowed beside a finished copy.
- **Checks:** `cargo test` 241; clippy/fmt clean; `npm test` 154/154 (macOS arm64).
- `codex-qa test "#59"`: **PASS WITH NOTES**, no bugs (`qa/reports/20261015-190439-test-Xfo0.md`). Its tests are added (D-021): `readd_q2_59_qa.rs` (5) and `readd-q2-59-qa.test.mjs` (2). `cargo test` 246, `npm test` 156/156.
- `codex-qa review`: **PASS WITH NOTES**, with no findings (`qa/reports/20261015-190957-review-CbX4.md`). Suggested tests, none blocking: a backend-level re-add that checks the second download gets the new format's flags (format capture is already tested in #11), and a failing re-added item during Download all.

## Done: #11 (D2) format presets (squash-merged from PR #58; D-015; 4-OS bundles green)
- **`download::Format`:** `Default` adds no flags. The others add exactly D-015's flags:
  - `CompatibleVideo`: `-S vcodec:h264,…,aext:m4a --merge-output-format mp4/webm/mkv`;
  - `Audio`: `-x`;
  - `CompatibleAudio`: `-f ba/b -S acodec:aac,aext:m4a,vext:mp4 -x`.
  A test checks that no format ever adds `-t`, `--remux-video`, `--recode-video`, `--audio-format` or a forced `mp4` merge. `run_with_retries_format` carries the format; the existing functions keep `Default`, so their callers are unchanged.
- **Notice:** `Format::notice` flags a "most compatible" download whose file isn't `.mp4`/`.m4v` (video) or `.m4a`/`.aac`/`.mp3` (audio). The wording is OS-neutral. `Queue::noticed` adds it to the Done row (`Item.notice`, cleared when the row starts again), and the UI shows it in amber.
- **Settings:** `format` lives in `settings.json`; files from before #11, and unknown values, mean Default. `settings_format` and `settings_set_format` read and save it. Settings has a "Format" radio group (`FormatPicker.tsx`) that's disabled until the saved value is known and reverts if a save fails. Downloads capture the format when they start.
- **ffmpeg:** `--enable-decoder=vp9` and `--enable-demuxer=avi` are added. macOS arm64 is about +1.2 MB (ffmpeg +15.5%, ffprobe +16.8%), and the build still reports LGPL 2.1+.
- **Checks (macOS arm64):** `cargo test` 230; clippy/fmt clean; `npm test` 148/148; `fetch-sidecars --check` OK. Real-sidecar tests pass with the new ffmpeg, network included: `download_real_d2` 9 (the new `d11_real_formats…`: Audio keeps the WAV bytes, and the compatible formats fall back with a notice), `retry_d2` 2, `meta_q3` 2. Use `ci:bundle` (the ffmpeg build changed).
- `codex-qa test "#11"`: **FAIL**, then fixed (`qa/reports/20261015-070419-test-lORt.md`). A `format` value from a newer version made the whole `settings.json` fail to load, which reset a chosen download folder to Downloads. My own test had encoded that. Each setting now falls back to its default on its own (`or_default`), keeping the rest of the file. Codex's tests are added (D-021): `format_d2_11_qa.rs` (6) and `format-d2-11-qa.test.mjs` (6). `cargo test` 236; `npm test` 154/154.
- `codex-qa review` round 1: **FAIL**, then fixed (`qa/reports/20261015-071403-review-JPtt.md`). The notice judged by extension alone, but an `.mp4` can hold AV1, and H.264 can come in `.mkv`. yt-dlp now also writes the chosen `vcodec|acodec` (`--print-to-file after_move:…`, to `queue-codecs.txt` in the work dir). `run_with_retries_saved` returns them, and `Format::notice_for` decides from the real codecs, named H.264, AV1, VP9, AAC, Opus and so on. It falls back to the extension when yt-dlp reports `NA`. New unit tests cover AV1 in MP4, Opus in MP4, H.264 in MKV, no-video sites and unknown codecs. A new real YouTube test checks that "Most compatible video" gives `.mp4` with `avc1` and `mp4a` reported and no notice. `cargo test` 238.
- `codex-qa review` round 2: **FAIL**, then fixed (`qa/reports/20261015-073254-review-1ZjY.md`). With one codec unknown (`av01|NA`, `NA|opus`), the known problem was ignored and the extension decided. Now each known codec is judged on its own; only when nothing known is wrong does an unknown codec fall back to the extension. New tests: the partial-codec cases, and a fake-yt-dlp retry test showing the successful attempt's `queue-codecs.txt` is used, never the failed attempt's, and is never taken for the download. `cargo test` 239.
- `codex-qa review` round 3: **PASS WITH NOTES** (`qa/reports/20261015-075336-review-XFNt.md`). Its P3 is fixed: the real format test now uses `run_with_retries_saved` and `notice_for`, like the app, and asserts yt-dlp's report for the WAV page (`vcodec=none`). Not added, from its suggestions: a site offering low-resolution H.264 alongside higher-resolution AV1 (live YouTube covers the H.264 choice), and changing the preset mid-download (the format is captured at start; covered by code inspection).
- **Installers at the #11 merge (CI artifacts):** dmg 78/81 MB (arm64/x64), NSIS 49 MB, MSI 62 MB, deb 85 MB, AppImage 156 MB. Under D-013's 256 MB.

## Done: #41 (NFR) native tests on Linux, Windows and macOS; the repo went public (squash-merged from PR #57; D-023, D-024)
The owner asked for tests that actually run and pass on all three OSes, using hosted runners and their Ubuntu box. The owner's choices: keep Rust pinned (D-010); extend `build.yml` as #41 rather than add a separate `ci.yml`.
- **CI (`build.yml`):** `check` is now a `test` matrix (`ubuntu-24.04`, `windows-2025`, `macos-26` arm64; `fail-fast: false`) with each OS's own sidecars. It runs `npm run build`, `npm test`, fmt, clippy `-D warnings`, `cargo test --locked` and the real-sidecar loopback tests. Bundles need `test`.
- **Green before billing stopped it:** `test (linux)`, `test (windows)`, `test (macos)` and a self-hosted box job on commit `434d58e`.
- **Bugs the new runs found, all fixed:**
  1. Windows clippy: `proc.rs` imported Unix-only time types.
  2. The "Text file busy" flake (open problem 5). It reproduced about 20 times in 15 runs on the 16-thread box. `proc::spawn_tree` now retries ETXTBSY briefly on Unix; 0 failures in 75 runs after the fix.
  3. Windows: a test stepped 1 ns at the info-freshness cutoff, but Windows' SystemTime counts 100 ns. It now steps 1 µs.
- `codex-qa test "#41"`: **FAIL**, then fixed (`qa/reports/20261014-201623-test-u22j.md`). Two parts of #41 were missing:
  1. CI never ran the real-sidecar loopback tests. They now run in `test` and in `scripts/test-linux.sh`.
  2. There was no Windows download fixture. Codex's tests are added (D-021): `native_download_41_qa.rs` builds a tiny Rust fake yt-dlp (`tests/support/native_sidecar_41.rs`) with `rustc`. Its 5 portable tests cover Unicode argv and paths, collisions, pipes, a 403 retry, timeouts and an orphaned descendant. It adds 2 Linux-only ETXTBSY tests, plus a CI check that the loopback step exists.
- **Actions minutes ran out:** about 3,100 billed minutes in 6 days against the private plan's 2,000. Mostly macOS bundles (about 48%) and the macOS ffmpeg job (about 16%), both at 10×. GitHub stopped starting jobs.
- **Going public (2026-10-14, the owner's decision; D-023):**
  - **History scan:** gitleaks over all 56 commits, including GitHub's pre-squash `refs/pull/*`, found no leaks. No binaries were ever committed, and the only email addresses are the owner's and noreply ones.
  - **Box runner removed:** unregistered on GitHub (0 runners). The owner runs `sudo bash ~/queue-box-runner-remove.sh` on the box to remove the service and the `gh-runner` user; the build packages stay for `scripts/test-linux.sh`.
  - **CI simplified:** the `test-box` job, the cost-gating `plan` job (`ci-plan.sh`) and `.github/actionlint.yaml` are removed; the full matrix runs on every PR. A config test asserts there's no self-hosted runner, no `pull_request_target` and no secrets.
  - **LAN details** removed from the current docs (they remain in history; they're private, unreachable addresses).
- **Linux box:** `chrisbeast` (Ubuntu 26.04, Ryzen, 16 threads) runs `scripts/test-linux.sh` over SSH; section C of `docs/QA.md` covers it. Passes, including the loopback and ETXTBSY tests.
- **Local checks:** `cargo test` 225, `npm test` 141/141, actionlint and shellcheck clean.
- **License and public-facing files (D-024):** MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`; `Cargo.toml`, `package.json` and the lockfile's root entry). Also added:
  - `THIRD_PARTY_NOTICES.md`: yt-dlp (Unlicense source; the shipped binary is GPLv3+), deno MIT, and the LGPL FFmpeg build with its exact source and build script. This rewrite of an existing file first dropped the GPLv3+ note and the FFmpeg trademark line; both are restored;
  - `SECURITY.md`, which asks for private vulnerability reports;
  - `CONTRIBUTING.md`: issues welcome, and an issue before any PR;
  - the README's responsible-use and License sections.
- **Public (2026-10-14):** the owner flipped it. Set: fork-PR workflows need approval for all outside contributors; private vulnerability reporting; secret scanning with push protection; Dependabot alerts and security updates (no open alerts). The box runner service and user were removed (`queue-box-runner-remove.sh`, run by the owner).
- **CI on free runners:** `test (windows)` first failed 2 of 8 real-sidecar tests with WSAECONNABORTED (10053). On Windows, a socket accepted from the fixture's non-blocking listener is non-blocking too, so a read could hit WouldBlock and drop the connection. Fixed by setting it blocking. Run on `99cb845`: green on all three test jobs and all 4 bundles.
- `codex-qa review` round 1: **FAIL**, then fixed (`qa/reports/20261014-215725-review-MExm.md`), both in `scripts/test-linux.sh`:
  1. The box never rebuilt a stale ffmpeg. It now stores a fingerprint of `build-ffmpeg.sh` and `sidecars.json` and rebuilds when it changes or a binary is missing.
  2. ssh split quoted arguments. Host, folder and xvfb values are now validated as plain (openrsync has no `--protect-args`), and the ssh arguments are quoted with `printf %q`. `qa/tests/test-linux.test.mjs` covers this. Verified on the box.
- `codex-qa review` round 2: **FAIL** (P1), then fixed (`qa/reports/20261014-220204-review-pUlL.md`). `QUEUE_LINUX_DIR=.` passed validation, which would have pointed `rsync --delete` at the box's whole home folder. Two layers now prevent it:
  1. Every path segment must start with a letter or digit.
  2. Before any sync, a box-side guard resolves the folder with `realpath`. It must be strictly inside the home, and if it already has files it must carry `.queue-test-checkout`, which the script creates.
  Tested on the box: a symlink to the home and an unrelated folder with files were both refused, with nothing touched. A normal run passes. `npm test` 144/144.
- `codex-qa review` round 3: **PASS WITH NOTES** (`qa/reports/20261014-220617-review-sZab.md`). Two P3s, both about test strength, are left as follow-ups:
  1. The orphan-timeout test doesn't assert that the descendant actually exited.
  2. `test-linux.test.mjs` checks the remote guard's text rather than running it (it was verified by hand on the box).
- **Final CI on `3293625`:** green on `test (linux/windows/macos)` and all 4 bundles.

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
- **macOS arm64 locally, at the #67 merge:** `cargo test` 296 (14 ignored), `npm test` 185/185, real-sidecar loopback tests pass (real-sidecar tests with network 14/14 at #11); clippy, fmt, actionlint and shellcheck clean.
- **Owner's Linux box (`scripts/test-linux.sh`):** all checks, including the loopback and ETXTBSY tests.
- **Hosted CI (public repo, free runners):** every PR runs the full checks natively on Linux, Windows and macOS arm64, plus the real-sidecar loopback tests. Bundles for all 4 targets run on `main` and `ci:bundle` PRs. All green at the #67 merge.
- **Not verified natively:** UI behavior (folder picker, Open folder / Show in folder, webviews) on Windows and Linux; CI only runs the automated tests there.

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
6. `scripts/codex-qa` can leave its watchdog `sleep` running after Codex exits, holding the output pipe open, so callers waiting for EOF hang (seen in the #10 review). Being fixed in #69.
7. Flaky on Windows CI: `native_metadata_timeout_stops_a_running_executable_promptly` timed out (300 ms) before the fake yt-dlp started, on 2 of 3 PR #64 runs (passed on re-run). Fixed in #65 (3 s to start); passed on Windows CI on the first run.
8. Dependabot alert 1 (moderate, GHSA-wrw7-89jp-8q8g): `glib` 0.18.5 `VariantStrIter` unsoundness, reached only through Tauri's Linux GTK stack (fixed in glib 0.20). Queue's code doesn't use glib, and no fix is possible until Tauri moves to gtk-rs ≥ 0.20. Proposed: dismiss as tolerable risk; waiting on the owner.

## Owner checks in the dev app (macOS)
- 2026-10-13: live progress, "Processing…" and Retry worked on real downloads.
- 2026-10-13: #10's Change… opened the native picker. A download landed in the chosen (iCloud Drive) folder, and the choice was remembered after a relaunch.
- 2026-10-14: #39's Open folder opened the download folder in Finder, and Show in Finder selected the saved file (macOS).
- 2026-10-15: #11's Format setting worked: Best quality gave `.webm`, Most compatible video gave `.mp4`, and Audio only gave `.opus`. Re-adding that link for Most compatible audio was refused, which is bug #59.
- 2026-10-15: with #59's fix, re-adding that link with Most compatible audio worked and gave an `.m4a`.

## Next bounded action
Commit #68 (owner approval), open its PR, and merge once CI is green. Then #69, #70, #72 and #74 (cancel a download in progress, owner request; row goes back to Queued, marked Cancelled), each through the QA gates. Then a fresh M2 `codex-qa audit` and closing M2.
