# Backlog

This file is the source of truth for scope and order. Each item has a matching GitHub issue (see [D-004](DECISIONS.md)).
Requirement IDs refer to [PRD-001](prds/PRD-001-queue-v1.md). `N-T1` = §6.3 Tier 1 naming; `6.5` = History; `NFR` = §7.

## M1 – Walking skeleton

| Issue | Req | Pri | Area | Item |
|---|---|---|---|---|
| #1 | — | P0 | infra | **Scaffold Tauri v2 + React/TS/Tailwind app** — Create the app skeleton with pinned toolchain versions. `npm run tauri dev` opens an empty window. |
| #2 | D6 | P0 | infra | **Bundle yt-dlp + ffmpeg sidecars** — Fetch pinned yt-dlp and deno and build minimal LGPL ffmpeg/ffprobe from source for each target triple (macOS arm64/x64, Windows x64, Linux x64) into src-tauri/binaries (git-ignored), wired up as Tauri sidecars (D-012). Also adds the per-target sidecar steps to CI (`build.yml`). |
| #3 | Q1 | P0 | queue | **URL bar with + button and basic queue list** — Single-line input; Enter or + adds a row showing the raw URL. |
| #4 | D2 | P0 | download | **Download one item to a fixed folder named {title}** — Run the yt-dlp sidecar with an argv array and -o '%(title)s.%(ext)s'. No -f, so yt-dlp picks the format. Item shows done/failed. |
| #29 | NFR | P0 | infra | **Cross-platform CI builds (macOS, Windows, Linux)** — GitHub Actions matrix using tauri-action. Unsigned artifacts are uploaded on every push to main (D-011). The sidecar fetch step comes with #2. |

## M2 – Real queue

| Issue | Req | Pri | Area | Item |
|---|---|---|---|---|
| #5 | Q2 | P0 | queue | **Validate URLs and reject duplicates** — Inline error for invalid URLs; a duplicate shows a toast instead of a second row. |
| #6 | Q3 | P0 | queue | **Fetch metadata on add (yt-dlp -J)** — Row fills in thumbnail, title, channel, and duration before any download. |
| #48 | D2 | P0 | download | **Auto-retry YouTube 403s and reuse fetched info** — Retry a 403 up to 2× with backoff (~10 s/30 s); download from the saved `-J` via `--load-info-json` when it's under 5 h old, a margin before YouTube's ~6 h link expiry (D-017). |
| #7 | Q4,Q5 | P0 | queue | **Sequential queue runner with item states** — Download all processes ready items one at a time, in order by id; rows still fetching info don't block ready rows (D-019), and a failure doesn't stop the run. Lifecycle: Fetching → Queued → Downloading → Processing (#8) → Done/Failed; the proposed name is set as a row becomes Queued (#15). |
| #8 | D4 | P0 | download | **Live progress (percent, speed, ETA)** — Parse --newline JSON progress-template output and stream events to the UI. |
| #9 | Q6 | P0 | queue | **Remove and retry items with readable failure reasons** — A failed item shows a short reason (last stderr line) and Retry; the queue continues. |
| #10 | D1 | P0 | download | **Output folder picker, remembered** — Native folder dialog; persisted in settings. |
| #59 | Q2 | P0 | queue | **Re-add a link whose download finished** — Owner-reported bug: only active rows (fetching, queued, downloading, processing) make a link a duplicate, so a finished link can be downloaded again (e.g. in another format). |
| #36 | 6.4 | P0 | ui | **Settings screen** — One screen for all settings, persisted in the app's config folder (D-020) and built with #10 (its first control). #11, #17, #25/#31/#32, #20, #21, and #37 add their controls here. |
| #39 | D7 | P0 | ui | **Open downloads folder and Show in folder** — An Open folder button (Finder / File Explorer / file manager, per OS) and Show in folder on finished rows, through two of Queue's own commands that take no path from the UI (the folder, or a row id), with per-OS launchers that report failures (D-022). |
| #11 | D2 | P0 | download | **Format presets: Default (yt-dlp decides) / Audio only / Most compatible video (MP4) / Most compatible audio (M4A)** — Default passes no -f and forces no container. Audio only uses -x and keeps the best native audio with no re-encode. Most compatible uses D-015's `-S` flags (strict H.264/AAC, honest fallback notice), and the ffmpeg build adds the VP9 decoder + AVI demuxer. Default preset is Default. |
| #12 | Q7 | P1 | queue | **Persist queue and settings across restarts** — `queue.json` in the app-data directory and `settings.json` in the app-config directory (D-020, D-025); an interrupted download comes back Queued. |
| #13 | Q8 | P1 | queue | **Paste multiple URLs at once** — Newline-separated paste creates one row per URL. |
| #41 | NFR | P1 | infra | **Native macOS/Windows download tests in CI** — Run the download tests (and opt-in real-sidecar loopback tests) in the per-OS bundle jobs, and add a Windows executable fixture. M1 audit. |
| #42 | D2 | P2 | download | **Download failure-path regression tests** — Forced cross-volume `place`, mid-copy and sync failures, and StrictMode listener cleanup. M1 audit. |
| #45 | D2 | P2 | download | **Clean up staging folders left by an interrupted download** — Remove stale `item-*` work dirs on startup (D-014). |
| #65 | NFR | P2 | infra | **Harden #41's native tests** — Give the timeout fixtures time to start (flaky on Windows CI), check the orphaned descendant actually exits, and run `test-linux.sh`'s remote guard instead of matching its text. #41 review and PR #64 CI. |
| #67 | Q7,D2 | P1 | queue | **Stop downloads and metadata fetches when Queue quits** — On quit, refuse new yt-dlp runs, stop the running ones (whole trees) and wait up to 5 s, then save; rows cut off come back as interrupted. M2 audit. |
| #68 | Q7 | P2 | ui | **Don't fail silently when the save-warning listener can't register** — Show the failure and/or poll the save status, so D-025's warning can't be lost. M2 audit. |
| #69 | NFR | P2 | infra | **Fix the codex-qa watchdog and refresh stale docs** — The watchdog `sleep` can hold the caller's pipe for the whole timeout; README, STATUS, ARCHITECTURE and the PRD's format list are out of date. M2 audit. |
| #70 | D2,D4 | P2 | download | **Exercise real video+audio merging in CI** — Local video and audio fixtures over loopback, so every OS runs a real ffmpeg merge, Processing events and the format notices. M2 audit. |
| #72 | D2 | P2 | download | **Never leave a partial file under the final name** — Copy across volumes into a temporary name, then publish it without overwriting (per OS); startup removes Queue's leftover temp files. #67 review; accepted for #67 (D-014). Also fix the flaky #67 macOS test `native_a_descendant_left_after_the_run_is_stopped_with_its_tree` (owner, 2026-10-18). |
| #77 | D2 | P2 | download | **Long non-ASCII titles exceed Linux's 255-byte filename limit** — `--trim-filenames 200` counts characters, so a long CJK title gives a ~600-byte name; make names fit 255 bytes on every OS, leaving room for yt-dlp's temp suffixes. Found while researching #70. |
| #74 | Q6 | P1 | queue | **Cancel a download in progress** — A Cancel button on downloading/processing rows stops that download's whole tree (reusing #67) and removes its staging and partial files; the row goes back to Queued, marked Cancelled (owner's choice). Owner request; split from #22. |

## M3 – Smart naming (rules)

| Issue | Req | Pri | Area | Item |
|---|---|---|---|---|
| #14 | N-T1 | P0 | naming | **Tier-1 naming engine with cleanup rules** — Pure function: metadata + template → safe filename. Strip noise, emoji, and characters illegal on any OS (including Windows reserved names); cap length; prefer artist/track and series/episode fields. Table-driven unit tests with real titles. Also serves as the sanitizer and fallback for AI names. |
| #15 | N1 | P0 | naming | **Show proposed filename in row; inline edit** — Click the name to edit before download; the edited name is passed via -o. |
| #16 | N2 | P0 | naming | **Collision handling: append (n), never overwrite** — Done early with #7 (D-018): the finished file takes the first free `Title (n).ext`, claimed atomically at placement. |
| #17 | N3 | P1 | naming | **Naming template picker with live preview** — Presets {title}, {channel} - {title}, {upload_date} {title}; preview uses a sample item. |

## M4 – AI naming

| Issue | Req | Pri | Area | Item |
|---|---|---|---|---|
| #25 | N4 | P0 | naming | **AI naming provider toggle: Local (default) / API / Off** — NamingProvider trait with Local, Api, and Rules implementations, plus the settings UI. Any error, timeout (~10s), or invalid output falls back to Tier 1, and the row is marked "rules". |
| #30 | N6 | P0 | naming | **Local model runtime: bundled llama.cpp (llama-server) sidecar** — Official per-platform builds (Metal / CUDA or Vulkan / CPU fallback). Started on demand on a localhost port, unloaded after ~2 min idle. |
| #31 | N7 | P0 | naming | **Model manager: first-run model download** — Consented, resumable, checksum-verified GGUF download into app-data, with progress. Shows size and location; delete or switch model. Tier 1 is used until the model is ready. |
| #32 | N8 | P1 | naming | **API providers: Anthropic + OpenAI-compatible** — Base URL, model, and key (stored via the keyring crate in the OS credential store), plus a Test connection button. Covers OpenAI, OpenRouter, Ollama, and LM Studio. Split (D-029): **(a)**, #32: the OpenAI-compatible provider, with the key optional and kept in the OS credential store under service `dev.mcclease.queue`, account `api-key`. **(b)**, #91: the native Anthropic Messages API, which gates M4 completion. |
| #91 | N8 | P1 | naming | **Anthropic Messages API provider** (#32(b)) — `apiKind: anthropic`, `/v1/messages` with a forced `filename` tool, through the same request rules as `ai::ask`. Gates M4 completion. |
| #33 | N9 | P0 | naming | **Naming prompt, structured output, and eval set** — JSON-schema output, validation, and Tier 1 sanitizing. Eval set of ~50 real titles (podcasts, music, misc) to pick the default local model; target ≥90% accepted unedited and <3s per item. Split (D-029): **(a)**, done in PR #88: prompt, schema, validation, a 54-case author-written seed set and the eval runner. **(b)**, the owner's: 200–300 real, owner-labelled items and the real model runs on the pinned files. (b) gates M4 completion. |

## M5 – Polish & release

| Issue | Req | Pri | Area | Item |
|---|---|---|---|---|
| #18 | — | P0 | ui | **Visual design pass, light/dark theme** — Sleek, modern single-window UI that follows the system theme; thumbnails, polished states, empty state. Checked on all three OSes. |
| #19 | — | P1 | ui | **Keyboard shortcuts** — ⌘/Ctrl+V and Enter adds, ⌘/Ctrl+Enter starts, Delete removes the selected item; focus starts in the URL bar. |
| #20 | 6.5 | P1 | ui | **Completion notifications and history section** — OS notification on finish; Completed list with Show in folder (reuses #39), Copy URL, and Clear. |
| #21 | — | P1 | infra | **yt-dlp auto-update (at most daily)** — Run yt-dlp -U at launch if the last check was over 24h ago; setting to disable. |
| #22 | Q6 | P1 | queue | **Drag to reorder the queue** — Drag rows to change the order Download all uses. Cancel was split out to #74. |
| #23 | D3,D5 | P1 | download | **Per-item format override; embed thumbnail + metadata** — Per-row Default/Audio toggle; --embed-thumbnail --embed-metadata where supported. |
| #24 | NFR | P1 | infra | **Signed installers for all platforms** — macOS notarized .dmg, Windows signed MSI/NSIS (certificate decision pending), Linux AppImage + .deb. |
| #43 | NFR | P1 | infra | **Content Security Policy for the webview** — `app.security.csp` is null; set a strict CSP and verify it on all three webviews. |
| #44 | NFR | P2 | infra | **Verify yt-dlp release GPG signatures** — Check `SHA2-256SUMS.sig` against upstream's key when pins change (and the ffmpeg `.asc`). |
| #71 | NFR | P2 | ui | **Native UI smoke checks on Windows and Linux** — Folder picker (#10), Open folder / Show in folder (#39) and multi-line paste (#13), checked in the real app on each OS. M2 audit. |
| #83 | Q7 | P2 | queue | **Check queue.json's version before loading or saving over it** — A newer-format file is refused and preserved, not downgraded on the next save. M2 audit (`20261020-200119-audit-CgC4`). |
| #84 | Q6,Q7 | P2 | queue | **Command-level tests for cancel and quit** — Drive `queue_cancel`, `complete_download` and quit through the commands, especially a finish racing a cancel. M2 audit. |
| #85 | Q7 | P2 | queue | **Keep row ids within JavaScript's safe integer range** — Cap restored and allocated ids at 2^53 − 1 (or send them as strings). M2 audit (P3). |
| #86 | NFR | P2 | infra | **Make ffmpeg rebuilds reproducible** — Record the build environment, include it in the CI cache key, and pin it if needed. M2 audit (P3). |
| #93 | N8 | P2 | naming | **Keyless API on Linux without a Secret Service** — Treat an unreadable credential store as "no key" for a keyless service (Ollama, LM Studio), keeping the status note. #32 follow-up (P3). |

## Later

| Issue | Req | Pri | Area | Item |
|---|---|---|---|---|
| #26 | N5 | P2 | naming | **Optional subfolders by channel/artist** — Template support for {channel}/{title}. Off by default; v1 keeps one flat folder. |
| #27 | Q9 | P2 | queue | **Expand playlist URLs into items** — Confirm before adding more than 20 items. |
| #28 | Q4,Q10 | P2 | queue | **Auto-start and concurrency setting (1–3)** — Begin automatically when items are added; allow parallel downloads. |
| #37 | D1 | P2 | download | **Separate in-progress and completed folders** — Optional temp and completed folders with editable defaults; one folder by default (D-003, D-008). A temp folder maps to yt-dlp `-P temp:<dir>`. |
