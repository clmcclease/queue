# PRD: "Queue" — a desktop GUI for yt-dlp with smart file naming

**Status:** Draft v2 · **Owner:** Chris McClease · **Created:** 2026-10-10 · **Updated:** 2026-10-10 (size budget, D-013) · **Milestones:** see [BACKLOG](../BACKLOG.md)

## Context
Today the workflow is: copy a YouTube link → run `yt-dlp` in the terminal → file lands in a folder → rename it by hand. When several videos get downloaded in a row, the renaming gets skipped, files end up with useless names, and they get lost. The goal is a lightweight, polished, **cross-platform** desktop app that queues links, downloads them with yt-dlp into a folder you choose, and gives each file a clean, human-readable name. The name comes from the video's metadata and is tidied up by an AI model that runs **locally by default**.

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
5. It feels native, fast, and modern on **macOS, Windows, and Linux**: small install, quick launch, low idle CPU and memory.
6. AI naming works **offline and privately by default** (local model). Using a cloud API is an explicit opt-in.

## 3. Non-goals (v1)
- Web or mobile versions (possible later; see Future).
- Built-in media player or library browser.
- Accounts, cloud sync, or sharing.
- Bypassing DRM or paywalls, or downloading private/members-only content via cookies (possibly later as an advanced setting).
- Editing, trimming, or transcoding beyond what yt-dlp/ffmpeg do natively. We don't override yt-dlp's format choices by default.
- Organizing files into subfolders. Everything goes into one flat folder (subfolders are in the Later backlog).

## 4. Target user
A single power user (the owner) on any desktop OS who downloads podcasts, music, and videos to watch offline and wants them organized without effort. Secondary: non-technical friends who'd never touch a CLI.

## 5. Core user flow
1. Open the app. The URL bar is focused.
2. Paste a link and press Enter or click **+**. A row appears with a spinner, then fills in the **thumbnail, title, channel, duration, and the proposed filename** (fetched from metadata and AI-cleaned; nothing is downloaded yet).
3. Repeat for more links. Reorder by drag, or remove with ✕.
4. Click **Download all**. Items process in order, each showing a progress bar, speed, and ETA.
5. When an item finishes, the file is saved under its smart name. The row shows ✓ with a **Show in folder** button.
6. Failures show a short, readable reason and a **Retry** button. The queue keeps going.

## 6. Functional requirements

### 6.1 Queue
| ID | Requirement | Priority |
|---|---|---|
| Q1 | Single-line URL input with **+** button; Enter also adds | P0 |
| Q2 | Validate URL; reject duplicates already in queue | P0 |
| Q3 | On add, fetch metadata (`yt-dlp -J`) to show title/thumbnail/duration before download | P0 |
| Q4 | Sequential processing (concurrency = 1 by default; setting for 2–3) | P0 / Later |
| Q5 | Per-item states: Queued → Fetching info → Naming → Ready → Downloading → Processing → Done / Failed | P0 |
| Q6 | Remove, reorder (drag), retry, cancel individual items | P0 (remove/retry), P1 (reorder/cancel) |
| Q7 | Queue persists across app restarts | P1 |
| Q8 | Paste multiple URLs at once (newline-separated) → multiple items | P1 |
| Q9 | Playlist URL → expands into individual items (with confirm if > 20) | Later |
| Q10 | "Auto-start" toggle: begin downloading as soon as items are added | Later |

### 6.2 Download
| ID | Requirement | Priority |
|---|---|---|
| D1 | User-selectable output folder, remembered between sessions | P0 |
| D2 | Format presets: **Default**, which lets yt-dlp choose the best format and container itself (no `-f` override, nothing forced to mp4), and **Audio only** (`-x`, keeping yt-dlp's best native audio format with no re-encode). Default is **Default**. | P0 |
| D3 | Per-item format override (e.g., this one audio-only) | P1 |
| D4 | Live progress: %, speed, ETA (parsed from yt-dlp progress output) | P0 |
| D5 | Embed thumbnail + metadata into file where supported | P1 |
| D6 | Any site yt-dlp supports works, not just YouTube | P0 (inherent) |

### 6.3 Smart naming (the core feature)
The proposed name is computed as soon as metadata arrives, shown in the row, and passed to yt-dlp when the download starts.

**Tier 1, deterministic rules (P0).** This is always available and acts as the fallback.
- Uses yt-dlp's metadata fields with a template. The default template is `{title}`. Optional presets: `{channel} - {title}`, `{upload_date} {title}`.
- Clean-up rules: strip emoji and clickbait noise ("(OFFICIAL VIDEO)", "[4K]", "🔥"), collapse whitespace, remove characters that aren't allowed in filenames on *any* OS (Windows has the strictest rules, including reserved names like `CON` and `NUL`), and cap the length at about 150 characters.
- Uses structured fields when they exist: `artist`/`track` for music and `series`/`episode` for shows.

**Tier 2, AI naming (P0 for v1, on by default).** The model gets the title, channel, a short description snippet, and any structured fields, and returns a normalized name in a consistent style. The idea is similar to OpenWhispr's local-or-API choice:

| Provider | Default | Notes |
|---|---|---|
| **Local model** | ✅ | Runs on-device through a bundled llama.cpp runtime with a small instruct model (≈1–3B parameters, GGUF). It works offline and nothing leaves the machine. The model is downloaded on first run with consent and a progress bar; it isn't bundled in the installer. GPU acceleration is used where available (Metal, CUDA/Vulkan) with CPU fallback. |
| **API** | | The user pastes an API key. Supported: Anthropic (Claude Haiku) and any OpenAI-compatible endpoint, which also covers OpenAI, OpenRouter, Ollama, and LM Studio. The key is stored in the OS keychain/credential store. |
| **Off** | | Tier 1 rules only. |

- Examples:
  - `Joe Rogan Experience #2045 - Mel Gibson` on channel `PowerfulJRE` → `Joe Rogan Experience #2045 – Mel Gibson`
  - `Daft Punk - Get Lucky (Official Audio) ft. Pharrell Williams, Nile Rodgers` → `Daft Punk – Get Lucky (feat. Pharrell Williams & Nile Rodgers)`
- AI output always goes through the Tier 1 sanitizer. If the model errors, times out (about 10s), or returns an empty or overlong name, the app quietly uses the Tier 1 name and marks the row "rules".
- Until the local model is downloaded, Tier 1 is used and a non-blocking banner offers the download.

**Naming requirements:**
| ID | Requirement | Priority |
|---|---|---|
| N1 | Proposed filename shown in queue row before download; click to edit inline | P0 |
| N2 | Name collision → append ` (2)`, never overwrite | P0 |
| N3 | Template picker in settings with live preview | P1 |
| N4 | AI naming provider setting: **Local (default) / API / Off** | P0 |
| N5 | Optional subfolder by channel/artist (`{channel}/{title}.ext`) | Later |
| N6 | Bundled local runtime (llama.cpp) per platform, model loaded on demand and unloaded when idle | P0 |
| N7 | Model manager: first-run download (checksum-verified, resumable), show size/location, delete or switch model | P0 |
| N8 | API providers: Anthropic + OpenAI-compatible (base URL, model, key); "Test connection" button | P1 |
| N9 | Naming prompt with structured output, validation, and an eval set of real titles used to pick the default model | P0 |

*Implementation note:* the name is decided before the download starts and passed to yt-dlp's `-o` flag, so yt-dlp writes the final name directly and there's no separate rename step. This matches "name it appropriately once complete" but avoids a window where a file sits under the wrong name.

### 6.4 Settings
Download folder · format preset (Default / Audio only) · naming template · AI naming provider (Local / API / Off) + model manager + API key · concurrency · auto-start · auto-update yt-dlp (on by default) · notifications on completion.

### 6.5 History (P1)
A "Completed" section listing finished items with Show in folder, Copy source URL, and Clear history.

## 7. Non-functional requirements
- **Cross-platform:** macOS (Apple Silicon and Intel), Windows 10/11 (x64), and Linux (x64; AppImage and .deb) from M1 on. CI builds all three on every push.
- **Lightweight:** installer under 256 MB including yt-dlp, ffmpeg, deno, and the llama.cpp runtime (raised from ~80 MB on 2026-10-10 to fit bundled deno; D-013). The AI model (~1–2 GB) is a separate, optional first-run download. Cold launch under 1s. Idle memory under 150 MB with no model loaded. The model is only loaded while naming and unloaded after about 2 minutes idle.
- **Polished UI:** a single window with a dark/light theme that follows the system, smooth animations, thumbnails, and keyboard-first use (⌘/Ctrl+V and Enter adds, ⌘/Ctrl+Enter starts, Delete removes the selected item). It follows each OS's conventions for window controls and menus.
- **Resilient:** one item failing never stops the queue. yt-dlp self-updates (`yt-dlp -U`) at launch, at most once a day.
- **Private:** no telemetry. With the default settings, the only network calls are yt-dlp itself, update checks, and the one-time model download. Item metadata is only sent to a third party if the user picks the API provider.

## 8. Technical approach
- **Shell: Tauri v2** (Rust backend + system webview UI), confirmed in [D-002](../DECISIONS.md).
  - It's the best fit for a lightweight cross-platform app that mostly orchestrates subprocesses. The app shell is about 10 MB. Tauri has first-class "sidecar" binaries (yt-dlp, ffmpeg, llama-server) and official plugins for dialogs, notifications, the filesystem, the shell, and the auto-updater. Rust also gives native access to the OS keychain.
  - Trade-off: each OS uses its own webview (WebKit, WebView2, WebKitGTK), so UI is tested on all three. Linux/WebKitGTK is the weakest of the three. Electron is the fallback if webview differences become painful, at the cost of a ~150 MB+ install and higher memory use.
- **UI:** React, TypeScript, Tailwind, and a small component kit (shadcn/ui).
- **Engine:** bundle the official `yt-dlp` and `ffmpeg` binaries as Tauri sidecars and drive them from Rust:
  - `yt-dlp -J <url>` to get metadata when an item is added
  - `yt-dlp [-x] -o <computed path>.%(ext)s --newline --progress-template ...` to download with progress the app can parse. By default there's no `-f`, so yt-dlp picks the format.
- **AI naming:** a `NamingProvider` trait with three implementations: `Local`, `Api`, and `Rules`.
  - **Local** runs a bundled `llama-server` sidecar from llama.cpp's official per-platform builds on a localhost port, and talks to it through its OpenAI-compatible API. Because of that, the **Local** and **API** providers share one client.
  - Models live in the app-data directory.
  - API keys are stored with the `keyring` crate (macOS Keychain, Windows Credential Manager, Linux Secret Service).
- **Persistence:** a JSON file or SQLite in the app-data directory for the queue, history, and settings.

## 9. Success metrics (personal-project scale)
- No manual renames needed in a typical week.
- On the eval set, at least 90% of AI names are accepted unedited. Local naming takes under 3s per item on a recent laptop.
- Under 5% of downloads fail (excluding the source being unavailable).
- Adding 10 links and starting the queue takes under 30 seconds.

## 10. Milestones
1. **M1 – Walking skeleton:** paste URL → download to a fixed folder with the `{title}` name. Bare UI. Builds on all three OSes in CI.
2. **M2 – Real queue:** metadata preview, progress, sequential processing, retry and remove, folder picker, format presets, persistence.
3. **M3 – Smart naming (rules):** Tier 1 engine, inline name editing, collision handling, templates.
4. **M4 – AI naming:** local runtime + model manager (default), API providers, provider toggle, prompt + eval.
5. **M5 – Polish & release:** visual design pass, keyboard shortcuts, notifications, history, yt-dlp auto-update, signed installers for all three OSes.
6. **Later:** subfolders, playlists, auto-start/concurrency, and the Future items.

## 11. Risks
| Risk | Mitigation |
|---|---|
| YouTube changes break downloads | Bundle upstream yt-dlp, auto-update it, never fork |
| Messy titles produce bad names | AI naming plus Tier 1 sanitizer, inline edit before download |
| Small local model gives poor or inconsistent names | Eval set (N9) to choose the model, strict output validation, Tier 1 fallback, API option |
| Local model is too slow or heavy on older hardware | Small quantized model, GPU when available, unload when idle, "Off" or API as escape hatches |
| Model download size (~1–2 GB) | Optional, consented, resumable, and Tier 1 works meanwhile |
| Webview differences across OSes (WebKitGTK on Linux) | CI on all three, manual smoke test per release, conservative CSS |
| ffmpeg licensing/size | Use an LGPL build of ffmpeg; it's only needed to merge video and audio streams |
| ToS / copyright | Personal-use tool; no DRM circumvention; disclaimer in About |
| Unsigned builds blocked (Gatekeeper, SmartScreen) | macOS notarization; Windows code signing (cost to be decided); Linux AppImage |

## 12. Decisions made (2026-10-10)
1. **Platform:** cross-platform (macOS, Windows, Linux) → Tauri v2.
2. **AI naming:** in v1. Provider toggle Local / API / Off, with **Local as the default**.
3. **Format:** let yt-dlp decide. No forced container or re-encode.
4. **Subfolders:** not in v1. Moved to the Later backlog (N5).

## 13. Open questions
1. Default local model: chosen by the N9 eval. Candidates are small instruct GGUFs (e.g. Qwen 2.5/3 1.5–3B, Llama 3.2 3B, Gemma 3 1B/4B).
2. Windows code signing: buy a certificate, or accept SmartScreen warnings for personal use?

## 14. Future
- Browser extension or share-menu item that sends the current tab to the queue.
- Clipboard watcher: offer to queue video links when they're copied.
- Web or mobile companion that queues links remotely for the desktop app to download.
