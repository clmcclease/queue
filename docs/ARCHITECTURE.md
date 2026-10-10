# Architecture

> Stack confirmed (D-002, D-006); details provisional until M1 lands. Implements [PRD-001](prds/PRD-001-queue-v1.md).

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
│  naming::Provider  Local (default) | Api | Rules; Tier-1 sanitized   │
│  models::Manager   GGUF download/verify; llama-server lifecycle      │
│  store             JSON/SQLite in app-data; API key in OS keyring    │
└───────────────┬──────────────────────────────────────────────────────┘
                │ argv (no shell)
        sidecars: yt-dlp, ffmpeg, llama-server (localhost, on demand)
```

## Item lifecycle

`Queued → FetchingInfo → Naming → Ready → Downloading → Processing → Done | Failed`

1. **Add:** validate the URL, reject duplicates, run `yt-dlp -J --no-playlist <url>`, store the metadata, then compute the proposed filename with the active naming provider. Tier 1 runs first, so a name is always available instantly.
2. **Start:** the runner takes the next `Ready` item and runs `yt-dlp [-x] -o <dir>/<name>.%(ext)s --newline --progress-template <json> <url>`. No `-f` by default (D-007).
3. **Progress:** parse each stdout line and emit `{id, percent, speed, eta}` to the UI.
4. **Finish:** check that the file exists and record it in history. If the item failed, keep stderr's last line as the reason and move on.

## Naming engine (Tier 1)

- Input: yt-dlp metadata (`title`, `channel`, `upload_date`, `artist`, `track`, `series`, `episode_number`, …) and a template string.
- Steps: pick the structured fields if present → apply the template → strip noise patterns → strip emoji → replace characters that aren't allowed in filenames → collapse whitespace → cap the length → resolve collisions by adding ` (n)`.
- Pure and deterministic, covered by table-driven unit tests built from real titles.
- The filename rules are the strictest across OSes (Windows reserved names, trailing dots and spaces, `<>:"/\|?*`).

## AI naming (Tier 2)

- `trait NamingProvider { async fn name(&self, meta: &Meta) -> Result<String> }` with three implementations: `Local`, `Api`, and `Rules`.
- `Local`: `models::Manager` makes sure the GGUF model exists and starts `llama-server -m <model> --port <free>` when first needed. It stops the server after ~2 min idle.
- `Local` and `Api` share one OpenAI-compatible chat client. Anthropic gets a thin adapter.
- The prompt asks for JSON `{ "filename": "..." }`. The output is validated, run through the Tier 1 sanitizer, and dropped in favor of the Tier 1 name on error, timeout, or invalid output.
- The eval set (`naming-eval/`) holds real titles plus the expected names, and a runner scores accept rate and latency for each model.

## Sidecars

- A `scripts/fetch-sidecars` step downloads pinned `yt-dlp`, `ffmpeg` (LGPL), and `llama-server` (llama.cpp release) builds for each target triple (aarch64/x86_64-apple-darwin, x86_64-pc-windows-msvc, x86_64-unknown-linux-gnu) into `src-tauri/binaries/`. These binaries are git-ignored.
- Models are never bundled. They're downloaded at runtime into the app-data directory.
- At runtime, `yt-dlp -U` runs at most once a day (configurable).
