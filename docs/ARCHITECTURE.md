# Architecture

> Stack confirmed (D-002, D-006). Implements [PRD-001](prds/PRD-001-queue-v1.md). M1–M3 are built: the real queue, Tier-1 names, inline editing and templates. In M4, the AI provider setting and an OpenAI-compatible API service (#32) exist; the local runtime and model (#30, #31) don't yet, so names are rule-based unless an API is set up (see [STATUS](STATUS.md)). Sections marked **Planned** describe later milestones and don't exist yet.

## Components

```
┌──────────────── Tauri window (React + TS + Tailwind) ────────────────┐
│  UrlBar  →  QueueList (rows: thumb · title · proposed name · state)  │
│  Settings · History (planned, M5)                                    │
└───────────────▲──────────────────────────────┬───────────────────────┘
                │ events (progress, state)     │ commands (add, start, remove…)
┌───────────────┴──────────────────────────────▼───────────────────────┐
│ Rust core                                                            │
│  queue             sequential runner, state machine per item         │
│  meta · download   yt-dlp -J metadata; download + progress (proc)    │
│  settings          settings.json (config dir), queue.json (data dir) │
│  naming            pure fn: metadata + template → name   (#14)       │
│  naming::Provider  Local (default) | Api | Rules         (M4)        │
│  models::Manager   GGUF download/verify; llama-server    (M4)        │
└───────────────┬──────────────────────────────────────────────────────┘
                │ argv (no shell)
        sidecars: yt-dlp, ffmpeg/ffprobe, deno; llama-server (M4, localhost)
```

## Item lifecycle

Today: `Fetching → Queued → Downloading → Processing → Done | Failed`. Failed rows can be retried, a row interrupted by quitting comes back `Queued` (D-025), and a cancelled download goes back to `Queued`, marked Cancelled (D-027). A row gets its proposed name as it becomes `Queued` (#15); the AI provider (M4) may replace it later, while the row is still `Queued` and not edited.

1. **Add:** validate the URL, reject duplicates, run `yt-dlp -J --no-playlist <url>`, and store the metadata. The Tier-1 name (`naming::name`) is set under the same lock that makes the row `Queued`, so a Queued row always has a name. The user can edit it while the row is Queued; an edit is sanitized and never replaced automatically.
2. **Start:** the runner takes the next `Queued` item and runs `yt-dlp [-x] -P <fresh work dir> -o <name, % doubled>.%(ext)s --newline --progress-template <json> -- <url>` (D-014). The work dir is under the app cache, so partial and merge-temp files never sit in the user's folder. No `-f` by default (D-007).
3. **Progress:** parse each stdout line and emit `{id, percent, speed, eta}` to the UI.
4. **Finish:** take the one finished file from the work dir and place it in the output folder with a no-clobber move: a hard link, or across volumes a copy under a temporary name that's then renamed into place without replacing anything (D-014, #72). Plain `rename` is never used, because it replaces files. The filesystem itself refuses an existing name, and a crash mid-copy never leaves a cut-off file under the real name. Record it in history and remove the work dir. If the item failed, keep stderr's last `ERROR:` line as the reason and move on.

## Naming engine (Tier 1) — Built (#14, `src-tauri/src/naming.rs`)

- Input: yt-dlp metadata (`title`, `channel`, `upload_date`, `artist`, `track`, `series`, `season_number`, `episode_number`) and one of three templates: `{title}`, `{channel} - {title}`, `{upload_date} {title}` (D-029; no template language).
- Steps: pick the structured fields if present → apply the template → strip noise patterns → strip emoji → replace characters that aren't allowed in filenames → collapse whitespace → cap the length.
- Pure and deterministic (metadata + template → name, no filesystem access), covered by table-driven unit tests built from real titles.
- Collisions (N2, #16, already built) aren't part of the engine. The download layer picks a free ` (n)` variant and relies on the no-clobber placement (D-014), because only the filesystem knows which names exist (including case- and Unicode-equivalent ones).
- The filename rules are the strictest across OSes on every OS: `<>:"/\|?*` get the same fullwidth forms yt-dlp uses, Windows device names get a `_`, there are no leading or trailing dots or spaces, and a name is at most 150 characters and 200 bytes. `naming::sanitize` applies just these rules; it's the safety net for edited and AI names.

## AI naming (Tier 2) — Planned (M4)

- A blocking `naming::Provider` with `Local`, `Api` and `Rules`, run on one worker thread like the metadata fetches (D-029; no async runtime in app code). The AI name replaces `{title}` in the active template.
- `Local`: the model manager (`models.rs`, **built in #31**) downloads the pinned GGUF on the user's click into `app_local_data_dir()/models/`: chunked ranged GETs, resumable from `<file>.part`, the whole sha256 checked before the file gets its name, and `<file>.sha256` as the proof (nothing is hashed at launch). The banner and Settings → AI naming → Local show the size, source, location and progress, with Download, Cancel, Resume, Delete and the model choice. **Planned (#30):** start `llama-server -m <model> --port <free>` when first needed and stop it after ~2 min idle; until then a downloaded model reports `RuntimeMissing`.
- `Local` and `Api` share one OpenAI-compatible chat client, `ai::ask`. The native Anthropic Messages API is #91.
- **Built (#32):** `Api` resolves from settings.json's `apiBaseUrl` and `apiModel` (`apiKind` is `openai_compatible` today) and an optional key from the `Secrets` trait (`secrets.rs`). The real store is the `keyring` crate (service `dev.mcclease.queue`, account `api-key`), cached in memory after the first read. `cargo test` never touches the real store: tests use `secrets::Memory`, and one ignored round trip is run by hand. The endpoint is resolved inside each naming job, after the row's generation is taken (`ai::Resolving`), so a provider changed meanwhile is never sent to. Test connection names the preview sample through `ai::ask`.
- The prompt asks for JSON `{ "filename": "..." }`. The output is validated, run through the Tier 1 sanitizer, and dropped in favor of the Tier 1 name on error, timeout, or invalid output.
- The eval set (`naming-eval/`) holds real titles plus the expected names, and a runner scores accept rate and latency for each model.

## Sidecars

- Sidecars are pinned in `scripts/sidecars.json` for each target triple (aarch64/x86_64-apple-darwin, x86_64-pc-windows-msvc, x86_64-unknown-linux-gnu) and land in `src-tauri/binaries/` (git-ignored) as `queue-<name>-<triple>[.exe]` (D-012):
  - `scripts/fetch-sidecars.mjs` downloads and checksum-verifies the official `yt-dlp` onefile build and `deno` (yt-dlp's JS runtime for YouTube). `--check` confirms yt-dlp sees all its tools.
  - `scripts/build-ffmpeg.sh` builds a minimal LGPL `ffmpeg` + `ffprobe` from the signed upstream tarball, with no network protocols.
  - `llama-server` (llama.cpp release) is planned for #30 (M4).
- Tauri copies them next to the app binary and strips the triple. `src-tauri/src/sidecar.rs` resolves them there and builds the base yt-dlp argv (`--ignore-config --no-update --ffmpeg-location … --js-runtimes deno:…`).
- Models are never bundled. They're downloaded at runtime into the app-data directory.
- **Planned (#21, M5):** run `yt-dlp -U` at most once a day (configurable). Today the base argv passes `--no-update`, so yt-dlp only changes when the pinned version does.

## Persistence

- `settings.json` in the app config folder (D-020) and `queue.json` in the app data folder (D-025), both written atomically (a temp file, then a rename). A damaged `queue.json` is moved aside, and a save failure shows a warning in the window.
- Download staging folders live in the app cache folder and are removed after each download, or at the next launch if Queue was interrupted (D-014, #45).
- API keys (M4) go in the OS credential store via the `keyring` crate, never in these files. History (P1) is planned for M5.
