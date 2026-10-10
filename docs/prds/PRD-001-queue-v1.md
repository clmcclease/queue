# PRD: "Queue" — a desktop GUI for yt-dlp with smart file naming

**Status:** Draft · **Owner:** Chris McClease · **Created:** 2026-10-10 · **Milestones:** see [BACKLOG](../BACKLOG.md)

## Context
Today the workflow is: copy a YouTube link → run `yt-dlp` in the terminal → file lands in a folder → rename it by hand. When several videos get downloaded in a row, the renaming gets skipped, files end up with useless names, and they get lost. The goal is a lightweight, polished desktop app that queues links, downloads them with yt-dlp into a folder you choose, and gives each file a clean, human-readable name based on the video's actual title and metadata.

**A note on approach:** we don't need to fork yt-dlp. It's built to be run by other programs, and it already pulls the page's metadata (title, channel, upload date, episode/track info). The app should **bundle the official yt-dlp binary and run it as a subprocess**. That keeps us on upstream releases, which matters because YouTube breaks downloaders often and yt-dlp ships fixes within days. A fork would turn that into ongoing maintenance for us.

---

## 1. Problem statement
- Manual renaming doesn't scale. Batch downloads end up with default or garbled filenames.
- The CLI is fine for one-off downloads but awkward for "collect links over time, download later."
- There's no at-a-glance view of what's queued, downloading, done, or failed.

## 2. Goals
1. Paste a URL → click **+** (or press Enter) → it's in the queue. Under 2 seconds, no dialogs.
2. Download the whole queue one item at a time with a single **Start** action.
3. Every finished file has a clean, descriptive name without the user doing anything.
4. The user picks the download folder, and the app remembers it.
5. It feels native, fast, and modern: small install, quick launch, low idle CPU and memory.

## 3. Non-goals (v1)
- Web or mobile versions (possible later; see Future).
- Built-in media player or library browser.
- Accounts, cloud sync, or sharing.
- Bypassing DRM or paywalls, or downloading private/members-only content via cookies (possibly later as an advanced setting).
- Editing, trimming, or transcoding beyond what yt-dlp/ffmpeg do natively.

## 4. Target user
A single power user (the owner) on macOS who downloads podcasts, music, and videos to watch offline and wants them organized without effort. Secondary: non-technical friends who'd never touch a CLI.

## 5. Core user flow
1. Open the app. The URL bar is focused.
2. Paste a link and press Enter or click **+**. A row appears with a spinner, then fills in the **thumbnail, title, channel, and duration** (fetched from metadata, no download yet).
3. Repeat for more links. Reorder by drag, or remove with ✕.
4. Click **Download all**. Items process in order, each showing a progress bar, speed, and ETA.
5. When an item finishes, the file is saved under its smart name. The row shows ✓ with a **Reveal in Finder** button.
6. Failures show a short, readable reason and a **Retry** button. The queue keeps going.

## 6. Functional requirements

### 6.1 Queue
| ID | Requirement | Priority |
|---|---|---|
| Q1 | Single-line URL input with **+** button; Enter also adds | P0 |
| Q2 | Validate URL; reject duplicates already in queue | P0 |
| Q3 | On add, fetch metadata (`yt-dlp -J`) to show title/thumbnail/duration before download | P0 |
| Q4 | Sequential processing (concurrency = 1 by default; setting for 2–3) | P0 / P2 |
| Q5 | Per-item states: Queued → Fetching info → Downloading → Processing → Done / Failed | P0 |
| Q6 | Remove, reorder (drag), retry, cancel individual items | P0 (remove/retry), P1 (reorder/cancel) |
| Q7 | Queue persists across app restarts | P1 |
| Q8 | Paste multiple URLs at once (newline-separated) → multiple items | P1 |
| Q9 | Playlist URL → expands into individual items (with confirm if > 20) | P2 |
| Q10 | "Auto-start" toggle: begin downloading as soon as items are added | P2 |

### 6.2 Download
| ID | Requirement | Priority |
|---|---|---|
| D1 | User-selectable output folder, remembered between sessions | P0 |
| D2 | Format presets: **Best video (mp4)**, **Audio only (m4a/mp3)** | P0 |
| D3 | Per-item format override (e.g., this one audio-only) | P1 |
| D4 | Live progress: %, speed, ETA (parsed from yt-dlp progress output) | P0 |
| D5 | Embed thumbnail + metadata into file where supported | P1 |
| D6 | Any site yt-dlp supports works, not just YouTube | P0 (inherent) |

### 6.3 Smart naming (the core feature)
The app works out a clean name from the video's metadata and applies it when the download finishes.

**Tier 1, deterministic (P0):** use yt-dlp's metadata fields with a template.
- Default template: `{title}`. Optional presets: `{channel} - {title}`, `{upload_date} {title}`.
- Clean-up rules: strip emoji and clickbait noise ("(OFFICIAL VIDEO)", "[4K]", "🔥"), collapse whitespace, remove characters that aren't allowed in filenames, and cap the length at about 150 characters.
- Use structured fields when they exist: `artist`/`track` for music and `series`/`episode` for shows.

**Tier 2, AI-assisted cleanup (P1, opt-in):** send the title, channel, and description snippet to an LLM (Claude Haiku) and ask for a normalized name in a consistent style.
- Example raw title: `Joe Rogan Experience #2045 - Mel Gibson`, channel `PowerfulJRE` → `Joe Rogan Experience #2045 – Mel Gibson`
- Example raw title: `Daft Punk - Get Lucky (Official Audio) ft. Pharrell Williams, Nile Rodgers` → `Daft Punk – Get Lucky (feat. Pharrell Williams & Nile Rodgers)`
- Needs a user-supplied API key in settings. If it's off or fails, Tier 1 is used.

**Naming requirements:**
| ID | Requirement | Priority |
|---|---|---|
| N1 | Proposed filename shown in queue row before download; click to edit inline | P0 |
| N2 | Name collision → append ` (2)`, never overwrite | P0 |
| N3 | Template picker in settings with live preview | P1 |
| N4 | AI cleanup toggle + API key field | P1 |
| N5 | Optional subfolder by channel/artist (`{channel}/{title}.mp4`) | P2 |

*Implementation note:* the name is decided before the download starts and passed to yt-dlp's `-o` flag, so yt-dlp writes the final name directly and there's no separate rename step. This matches "name it appropriately once complete" but avoids a window where a file sits under the wrong name.

### 6.4 Settings
Download folder · format preset · naming template · AI cleanup (on/off + key) · concurrency · auto-start · auto-update yt-dlp (on by default) · notifications on completion.

### 6.5 History (P1)
A "Completed" section listing finished items with Reveal in Finder, Copy source URL, and Clear history.

## 7. Non-functional requirements
- **Lightweight:** installer under ~40 MB including yt-dlp and ffmpeg. Cold launch under 1s. Idle memory under 150 MB.
- **Polished UI:** a single window with a dark/light theme that follows the system, smooth animations, thumbnails, and keyboard-first use (⌘V and Enter adds, ⌘↩ starts, ⌫ removes the selected item).
- **Resilient:** one item failing never stops the queue. yt-dlp self-updates (`yt-dlp -U`) at launch, at most once a day.
- **Private:** no telemetry. The only network calls are yt-dlp itself, update checks, and the optional LLM call.
- **Platform:** macOS first (Apple Silicon and Intel). The stack allows Windows and Linux later with minimal changes.

## 8. Recommended technical approach
- **Shell:** **Tauri v2** (Rust backend plus a web UI). It produces much smaller installers and uses less memory than Electron, and stays cross-platform for later.
  - Alternative if this will only ever be a Mac app: native **SwiftUI**. It's the most native feel, but it's Mac-only.
- **UI:** React, TypeScript, Tailwind, and a small component kit (shadcn/ui).
- **Engine:** bundle the official `yt-dlp` and `ffmpeg` binaries as Tauri sidecars and drive them from Rust:
  - `yt-dlp -J <url>` to get metadata when an item is added
  - `yt-dlp -f <preset> -o <computed path> --newline --progress-template ...` to download with progress the app can parse
- **Persistence:** a JSON file or SQLite in the app-data directory for the queue, history, and settings.
- **AI naming:** a direct HTTPS call to the Anthropic API from the Rust backend. The key is stored in the macOS Keychain.

## 9. Success metrics (personal-project scale)
- No manual renames needed in a typical week.
- Under 5% of downloads fail (excluding the source being unavailable).
- Adding 10 links and starting the queue takes under 30 seconds.

## 10. Milestones
1. **M1 – Walking skeleton:** paste URL → download to a fixed folder with the `{title}` name. Bare UI.
2. **M2 – Real queue:** metadata preview, progress, sequential processing, retry and remove, folder picker, persistence.
3. **M3 – Smart naming:** clean-up rules, templates, inline name editing, collision handling.
4. **M4 – Polish:** visual design pass, keyboard shortcuts, notifications, history, yt-dlp auto-update, signed and notarized `.dmg`.
5. **M5 – AI cleanup** (opt-in) and the P2 items.

## 11. Risks
| Risk | Mitigation |
|---|---|
| YouTube changes break downloads | Bundle upstream yt-dlp, auto-update it, never fork |
| Messy titles produce bad names | Clean-up rules, inline edit before download, optional AI tier |
| ffmpeg licensing/size | Use an LGPL build of ffmpeg; it's only needed to merge video and audio streams |
| ToS / copyright | Personal-use tool; no DRM circumvention; disclaimer in About |
| macOS Gatekeeper blocks unsigned app | Developer ID signing and notarization in M4 |

## 12. Open questions
1. Is a Mac-only app fine (which would make SwiftUI an option), or do you want Windows/Linux eventually (Tauri)?
2. Should AI naming be in v1, or is metadata-based naming good enough to start?
3. Mostly video, mostly audio (podcasts/music), or a mix? This sets the default format.
4. Do you want to organize into subfolders by channel or show, or keep one flat folder?

## 13. Future
- Browser extension or Share-menu item that sends the current tab to the queue.
- Clipboard watcher: offer to queue YouTube links when they're copied.
- Web or mobile companion that queues links remotely for the desktop app to download.
