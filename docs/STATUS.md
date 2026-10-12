# Status

**Updated:** 2026-10-10 · **Milestone:** M1 done · M2 (real queue) in progress · **Active:** #7 + #16 done → #8 (live progress) next

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
- **macOS arm64 locally, at the #4 merge:**
  - `cargo test` 41, plus 4 opt-in real-sidecar tests (`QUEUE_QA_REAL_SIDECARS=src-tauri/binaries [QUEUE_QA_NETWORK=1] cargo test --test download_real_d2 -- --ignored`), including a real YouTube DASH merge;
  - `npm test` 54/54; clippy, fmt and build clean.
- **Hosted CI:** green for every M1 merge on all 4 targets.
- **Not verified natively:** downloads on Windows and Linux (CI builds and runs the tests there; the Rust download tests run on Linux only), and the native webviews on Windows and Linux.

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

## Next bounded action
Start **#8** (D4): live progress (percent, speed, ETA) from yt-dlp's `--newline --progress-template` (JSON with `|null` defaults, per the ytdlp-expert notes in D-015's research), streamed as item updates. Also add the **Processing** state (post-download merge/extract), completing #7's lifecycle except Naming (#14/#25).
