# Architecture

> Provisional until M1 lands. Implements [PRD-001](prds/PRD-001-queue-v1.md).

## Components

```
┌──────────────── Tauri window (React + TS + Tailwind) ────────────────┐
│  UrlBar  →  QueueList (rows: thumb · title · proposed name · state)  │
│  Settings · History                                                  │
└───────────────▲──────────────────────────────┬───────────────────────┘
                │ events (progress, state)     │ commands (add, start, remove…)
┌───────────────┴──────────────────────────────▼───────────────────────┐
│ Rust core                                                            │
│  queue::Runner     sequential worker, state machine per item         │
│  ytdlp::Client     spawns sidecar; -J metadata; download + progress  │
│  naming::Engine    pure fn: metadata + template → safe filename      │
│  naming::AiCleanup optional Anthropic call (M5), falls back to Tier 1│
│  store             JSON/SQLite in app-data dir; key in Keychain      │
└───────────────┬──────────────────────────────────────────────────────┘
                │ argv (no shell)
        sidecars: yt-dlp, ffmpeg
```

## Item lifecycle

`Queued → FetchingInfo → Ready → Downloading → Processing → Done | Failed`

1. **Add:** validate the URL, reject duplicates, run `yt-dlp -J --no-playlist <url>`, store the metadata, and compute the proposed filename.
2. **Start:** the runner takes the next `Ready` item and runs `yt-dlp -f <preset> -o <dir>/<name>.%(ext)s --newline --progress-template <json> <url>`.
3. **Progress:** parse each stdout line and emit `{id, percent, speed, eta}` to the UI.
4. **Finish:** check that the file exists and record it in history. If the item failed, keep stderr's last line as the reason and move on.

## Naming engine (Tier 1)

- Input: yt-dlp metadata (`title`, `channel`, `upload_date`, `artist`, `track`, `series`, `episode_number`, …) and a template string.
- Steps: pick the structured fields if present → apply the template → strip noise patterns → strip emoji → replace characters that aren't allowed in filenames → collapse whitespace → cap the length → resolve collisions by adding ` (n)`.
- Pure and deterministic, covered by table-driven unit tests built from real titles.

## Sidecars

- A `scripts/fetch-sidecars` step downloads pinned `yt-dlp` and `ffmpeg` (LGPL) builds for each target triple into `src-tauri/binaries/`. These binaries are git-ignored.
- At runtime, `yt-dlp -U` runs at most once a day (configurable).
