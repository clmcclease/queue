# Backlog

This file is the source of truth for scope and order. Each item has a matching GitHub issue (see [D-004](DECISIONS.md)).
Requirement IDs refer to [PRD-001](prds/PRD-001-queue-v1.md). `N-T1` = §6.3 Tier 1 naming; `6.5` = History.


## M1 – Walking skeleton

| Issue | Req | Pri | Area | Item |
|---|---|---|---|---|
| #1 | — | P0 | infra | **Scaffold Tauri v2 + React/TS/Tailwind app** — Create the app skeleton with pinned toolchain versions. `npm run tauri dev` opens an empty window. |
| #2 | D6 | P0 | infra | **Bundle yt-dlp + ffmpeg sidecars** — Script to fetch pinned yt-dlp and LGPL ffmpeg builds per target triple into src-tauri/binaries (git-ignored), wired up as Tauri sidecars. |
| #3 | Q1 | P0 | queue | **URL bar with + button and basic queue list** — Single-line input; Enter or + adds a row showing the raw URL. |
| #4 | D2 | P0 | download | **Download one item to a fixed folder named {title}** — Run the yt-dlp sidecar with an argv array and -o '%(title)s.%(ext)s'. Item shows done/failed. |

## M2 – Real queue

| Issue | Req | Pri | Area | Item |
|---|---|---|---|---|
| #5 | Q2 | P0 | queue | **Validate URLs and reject duplicates** — Inline error for invalid URLs; a duplicate shows a toast instead of a second row. |
| #6 | Q3 | P0 | queue | **Fetch metadata on add (yt-dlp -J)** — Row fills in thumbnail, title, channel, and duration before any download. |
| #7 | Q4,Q5 | P0 | queue | **Sequential queue runner with item states** — Download all processes items in order: Queued → FetchingInfo → Ready → Downloading → Processing → Done/Failed. |
| #8 | D4 | P0 | download | **Live progress (percent, speed, ETA)** — Parse --newline JSON progress-template output and stream events to the UI. |
| #9 | Q6 | P0 | queue | **Remove and retry items with readable failure reasons** — A failed item shows a short reason (last stderr line) and Retry; the queue continues. |
| #10 | D1 | P0 | download | **Output folder picker, remembered** — Native folder dialog; persisted in settings. |
| #11 | D2 | P0 | download | **Format presets: best video (mp4) / audio only** — Global preset selector mapped to yt-dlp -f / -x arguments. |
| #12 | Q7 | P1 | queue | **Persist queue and settings across restarts** — JSON or SQLite in the app-data directory; in-flight items resume as Ready. |
| #13 | Q8 | P1 | queue | **Paste multiple URLs at once** — Newline-separated paste creates one row per URL. |

## M3 – Smart naming

| Issue | Req | Pri | Area | Item |
|---|---|---|---|---|
| #14 | N-T1 | P0 | naming | **Tier-1 naming engine with cleanup rules** — Pure function: metadata + template → safe filename. Strip noise/emoji/illegal chars, cap length, prefer artist/track and series/episode fields. Table-driven unit tests with real titles. |
| #15 | N1 | P0 | naming | **Show proposed filename in row; inline edit** — Click the name to edit before download; the edited name is passed via -o. |
| #16 | N2 | P0 | naming | **Collision handling: append (n), never overwrite** — Check the target directory before download and pick a free name. |
| #17 | N3 | P1 | naming | **Naming template picker with live preview** — Presets {title}, {channel} - {title}, {upload_date} {title}; preview uses a sample item. |

## M4 – Polish

| Issue | Req | Pri | Area | Item |
|---|---|---|---|---|
| #18 | — | P0 | ui | **Visual design pass, light/dark theme** — Sleek, modern single-window UI that follows the system theme; thumbnails, polished states, empty state. |
| #19 | — | P1 | ui | **Keyboard shortcuts** — ⌘V+Enter adds, ⌘↩ starts, ⌫ removes the selected item; focus starts in the URL bar. |
| #20 | 6.5 | P1 | ui | **Completion notifications and history section** — macOS notification on finish; Completed list with Reveal in Finder, Copy URL, and Clear. |
| #21 | — | P1 | infra | **yt-dlp auto-update (at most daily)** — Run yt-dlp -U at launch if the last check was over 24h ago; setting to disable. |
| #22 | Q6 | P1 | queue | **Drag to reorder and cancel in-progress item** — Cancel kills the subprocess and cleans up partial files. |
| #23 | D3,D5 | P1 | download | **Per-item format override; embed thumbnail + metadata** — Per-row video/audio toggle; --embed-thumbnail --embed-metadata where supported. |
| #24 | — | P1 | infra | **Developer ID signing, notarization, .dmg** — Signed and notarized build that passes Gatekeeper on a clean Mac. |

## M5 – AI cleanup + P2

| Issue | Req | Pri | Area | Item |
|---|---|---|---|---|
| #25 | N4 | P1 | naming | **AI naming cleanup (opt-in, Claude Haiku)** — Send title/channel/description snippet to get a normalized name; key stored in Keychain; falls back to Tier 1 on any error. |
| #26 | N5 | P2 | naming | **Optional subfolders by channel/artist** — Template support for {channel}/{title}. |
| #27 | Q9 | P2 | queue | **Expand playlist URLs into items** — Confirm before adding more than 20 items. |
| #28 | Q4,Q10 | P2 | queue | **Auto-start and concurrency setting (1–3)** — Begin automatically when items are added; allow parallel downloads. |
