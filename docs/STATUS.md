# Status

**Updated:** 2026-10-10 · **Milestone:** M1 done · M2 (real queue) in progress · **Active:** #6 done → #48 (403 retry) next

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
Start **#48** (D-017): auto-retry an item that fails with `HTTP Error 403` up to 2× with backoff (~10 s, 30 s), and download from the saved `-J` result via `--load-info-json` when it's under ~6 h old (fall back to a fresh extraction). Keep the info JSON from #6's fetch (persist it in the app cache per item). Then #7 (sequential runner + Download all).
