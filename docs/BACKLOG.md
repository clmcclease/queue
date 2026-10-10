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
| #7 | Q4,Q5 | P0 | queue | **Sequential queue runner with item states** — Download all processes items in order: Queued → FetchingInfo → Naming → Ready → Downloading → Processing → Done/Failed. |
| #8 | D4 | P0 | download | **Live progress (percent, speed, ETA)** — Parse --newline JSON progress-template output and stream events to the UI. |
| #9 | Q6 | P0 | queue | **Remove and retry items with readable failure reasons** — A failed item shows a short reason (last stderr line) and Retry; the queue continues. |
| #10 | D1 | P0 | download | **Output folder picker, remembered** — Native folder dialog; persisted in settings. |
| #36 | 6.4 | P0 | ui | **Settings screen** — One screen for all settings, persisted in app-data and built with #10 (its first control). #11, #17, #25/#31/#32, #20, #21, and #37 add their controls here. |
| #11 | D2 | P0 | download | **Format presets: Default (yt-dlp decides) / Audio only** — Default passes no -f and forces no container. Audio only uses -x and keeps the best native audio with no re-encode. Default preset is Default. |
| #12 | Q7 | P1 | queue | **Persist queue and settings across restarts** — JSON or SQLite in the app-data directory; in-flight items resume as Ready. |
| #13 | Q8 | P1 | queue | **Paste multiple URLs at once** — Newline-separated paste creates one row per URL. |

## M3 – Smart naming (rules)

| Issue | Req | Pri | Area | Item |
|---|---|---|---|---|
| #14 | N-T1 | P0 | naming | **Tier-1 naming engine with cleanup rules** — Pure function: metadata + template → safe filename. Strip noise, emoji, and characters illegal on any OS (including Windows reserved names); cap length; prefer artist/track and series/episode fields. Table-driven unit tests with real titles. Also serves as the sanitizer and fallback for AI names. |
| #15 | N1 | P0 | naming | **Show proposed filename in row; inline edit** — Click the name to edit before download; the edited name is passed via -o. |
| #16 | N2 | P0 | naming | **Collision handling: append (n), never overwrite** — Check the target directory before download and pick a free name. |
| #17 | N3 | P1 | naming | **Naming template picker with live preview** — Presets {title}, {channel} - {title}, {upload_date} {title}; preview uses a sample item. |

## M4 – AI naming

| Issue | Req | Pri | Area | Item |
|---|---|---|---|---|
| #25 | N4 | P0 | naming | **AI naming provider toggle: Local (default) / API / Off** — NamingProvider trait with Local, Api, and Rules implementations, plus the settings UI. Any error, timeout (~10s), or invalid output falls back to Tier 1, and the row is marked "rules". |
| #30 | N6 | P0 | naming | **Local model runtime: bundled llama.cpp (llama-server) sidecar** — Official per-platform builds (Metal / CUDA or Vulkan / CPU fallback). Started on demand on a localhost port, unloaded after ~2 min idle. |
| #31 | N7 | P0 | naming | **Model manager: first-run model download** — Consented, resumable, checksum-verified GGUF download into app-data, with progress. Shows size and location; delete or switch model. Tier 1 is used until the model is ready. |
| #32 | N8 | P1 | naming | **API providers: Anthropic + OpenAI-compatible** — Base URL, model, and key (stored via the keyring crate in the OS credential store), plus a Test connection button. Covers OpenAI, OpenRouter, Ollama, and LM Studio. |
| #33 | N9 | P0 | naming | **Naming prompt, structured output, and eval set** — JSON-schema output, validation, and Tier 1 sanitizing. Eval set of ~50 real titles (podcasts, music, misc) to pick the default local model; target ≥90% accepted unedited and <3s per item. |

## M5 – Polish & release

| Issue | Req | Pri | Area | Item |
|---|---|---|---|---|
| #18 | — | P0 | ui | **Visual design pass, light/dark theme** — Sleek, modern single-window UI that follows the system theme; thumbnails, polished states, empty state. Checked on all three OSes. |
| #19 | — | P1 | ui | **Keyboard shortcuts** — ⌘/Ctrl+V and Enter adds, ⌘/Ctrl+Enter starts, Delete removes the selected item; focus starts in the URL bar. |
| #20 | 6.5 | P1 | ui | **Completion notifications and history section** — OS notification on finish; Completed list with Show in folder, Copy URL, and Clear. |
| #21 | — | P1 | infra | **yt-dlp auto-update (at most daily)** — Run yt-dlp -U at launch if the last check was over 24h ago; setting to disable. |
| #22 | Q6 | P1 | queue | **Drag to reorder and cancel in-progress item** — Cancel kills the subprocess and cleans up partial files. |
| #23 | D3,D5 | P1 | download | **Per-item format override; embed thumbnail + metadata** — Per-row Default/Audio toggle; --embed-thumbnail --embed-metadata where supported. |
| #24 | NFR | P1 | infra | **Signed installers for all platforms** — macOS notarized .dmg, Windows signed MSI/NSIS (certificate decision pending), Linux AppImage + .deb. |

## Later

| Issue | Req | Pri | Area | Item |
|---|---|---|---|---|
| #26 | N5 | P2 | naming | **Optional subfolders by channel/artist** — Template support for {channel}/{title}. Off by default; v1 keeps one flat folder. |
| #27 | Q9 | P2 | queue | **Expand playlist URLs into items** — Confirm before adding more than 20 items. |
| #28 | Q4,Q10 | P2 | queue | **Auto-start and concurrency setting (1–3)** — Begin automatically when items are added; allow parallel downloads. |
| #37 | D1 | P2 | download | **Separate in-progress and completed folders** — Optional temp and completed folders with editable defaults; one folder by default (D-003, D-008). A temp folder maps to yt-dlp `-P temp:<dir>`. |
