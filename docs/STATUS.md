# Status

**Updated:** 2026-10-10 · **Milestone:** M1 done · M2 (real queue) in progress · **Active:** #5 (Q2), PR open

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

## In progress: #5 (Q2) validate URLs, reject duplicates (committed; PR open)
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
#5: the PR's checks, then the owner approves the squash-merge. Then #6 (fetch metadata on add). #36 (Settings screen), #10 (folder picker) and #39 (Open folder) follow later in M2.
