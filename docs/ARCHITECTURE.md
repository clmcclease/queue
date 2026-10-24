# Architecture

> Stack confirmed (D-002, D-006). Implements [PRD-001](prds/PRD-001-queue-v1.md). M1–M3 are built: the real queue, Tier-1 names, inline editing and templates. In M4, the AI provider setting, the API services (OpenAI-compatible, #32; Anthropic's Messages API, #91), the model manager (#31) and the local runtime (#30) exist: once the model is downloaded, names come from it (see [STATUS](STATUS.md)). Sections marked **Planned** describe later milestones and don't exist yet.

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
4. **Finish:** take the one finished file from the work dir and place it in the output folder with a no-clobber move: a hard link, or across volumes a copy under a temporary name that's then renamed into place without replacing anything (D-014, #72). Plain `rename` is never used, because it replaces files. The filesystem itself refuses an existing name, and a crash mid-copy never leaves a cut-off file under the real name. Remove the work dir (recording history is #20, M5). If the item failed, keep stderr's last `ERROR:` line as the reason and move on.

## Naming engine (Tier 1) — Built (#14, `src-tauri/src/naming.rs`)

- Input: yt-dlp metadata (`title`, `channel`, `upload_date`, `artist`, `track`, `series`, `season_number`, `episode_number`) and one of three templates: `{title}`, `{channel} - {title}`, `{upload_date} {title}` (D-029; no template language).
- Steps: pick the structured fields if present → apply the template → strip noise patterns → strip emoji → replace characters that aren't allowed in filenames → collapse whitespace → cap the length.
- Pure and deterministic (metadata + template → name, no filesystem access), covered by table-driven unit tests built from real titles.
- Collisions (N2, #16, already built) aren't part of the engine. The download layer picks a free ` (n)` variant and relies on the no-clobber placement (D-014), because only the filesystem knows which names exist (including case- and Unicode-equivalent ones).
- The filename rules are the strictest across OSes on every OS: `<>:"/\|?*` get the same fullwidth forms yt-dlp uses, Windows device names get a `_`, there are no leading or trailing dots or spaces, and a name is at most 150 characters and 200 bytes. `naming::sanitize` applies just these rules; it's the safety net for edited and AI names.

## AI naming (Tier 2) — Built (M4; #33(b)'s owner-labelled eval still gates the milestone)

- A blocking `naming::Provider` with `Local`, `Api` and `Rules`, run on one worker thread like the metadata fetches (D-029; no async runtime in app code). The AI name replaces `{title}` in the active template.
- `Local`: the model manager (`models.rs`, **built in #31**) downloads the pinned GGUF on the user's click into `app_local_data_dir()/models/`: chunked ranged GETs, resumable from `<file>.part`, the whole sha256 checked before the file gets its name, and `<file>.sha256` as the proof (nothing is hashed at launch). The banner and Settings → AI naming → Local show the size, source, location and progress, with Download, Cancel, Resume, Delete and the model choice. **Built (#30(b)):** `local::Runtime` starts `llama-server` inside the naming job (`ai::Starting`, after the row's generation is taken) on a free 127.0.0.1 port with D-029's arguments and a fresh 32-byte key in `LLAMA_API_KEY`, and waits up to 120 s for `/health`. It reuses a running server, restarts one that died, and tries a second port if the first was taken. Queue's own timer stops it after 2 minutes without a request; it's also stopped at quit, before a model is deleted or switched, and when the provider leaves Local. A start that fails is a failed ask (the rules tag). `RuntimeMissing` now only means a build without `llama-server`.
- `Local` and `Api` share one chat client, `ai::ask`, the single place a naming request is made. `Endpoint.kind` picks the wire format: OpenAI-compatible (`<base>/chat/completions`, `response_format`; `temperature: 0` and `max_tokens` 128, except that the host `api.openai.com` gets `max_completion_tokens` 1024 and no `temperature`, which its reasoning models refuse, D-034, #108) or, **built in #91**, Anthropic's Messages API (`<base>/v1/messages` with `x-api-key` and `anthropic-version: 2023-06-01`; structured outputs via `output_config.format`, the schema derived from `response_format()` minus `minLength`/`maxLength`; `max_tokens` 1024; no `temperature`, `thinking`, `effort` or forced tool, which some current Claude models refuse). Only a finished turn (`stop_reason: end_turn`) counts; its first `text` block goes to `ai::parse`. Anthropic without a saved key is `NotSetUp`, so nothing is sent.
- **Built (#32):** `Api` resolves from settings.json's `apiBaseUrl` and `apiModel` (`apiKind` is `openai_compatible` or `anthropic`) and an optional key from the `Secrets` trait (`secrets.rs`). The real store is the `keyring` crate (service `dev.mcclease.queue`, account `api-key`), cached in memory after the first read. `cargo test` never touches the real store: tests use `secrets::Memory`, and one ignored round trip is run by hand. The endpoint is resolved inside each naming job, after the row's generation is taken (`ai::Resolving`), so a provider changed meanwhile is never sent to. Test connection names the preview sample through `ai::ask`.
- The prompt asks for JSON `{ "filename": "..." }`. The output is validated, run through the Tier 1 sanitizer, and dropped in favor of the Tier 1 name on error, timeout, or invalid output.
- The eval set is `src-tauri/naming-eval/titles.json`, a 54-case **author-written seed** with expected names and accept lists. The ignored runner in `tests/naming_eval_33.rs` scores each model's accept rate, JSON validity, fallback rate, warm and cold latency, and D-016's failure counters. #33(b) adds 200–300 owner-labelled real items.

## Sidecars

- Sidecars are pinned in `scripts/sidecars.json` for each target triple (aarch64/x86_64-apple-darwin, x86_64-pc-windows-msvc, x86_64-unknown-linux-gnu) and land in `src-tauri/binaries/` (git-ignored) as `queue-<name>-<triple>[.exe]` (D-012):
  - `scripts/fetch-sidecars.mjs` downloads and checksum-verifies the official `yt-dlp` onefile build and `deno` (yt-dlp's JS runtime for YouTube). `--check` confirms yt-dlp sees all its tools. **Built (#44):** `--verify-ytdlp-signature` checks yt-dlp's signed `SHA2-256SUMS` for the pinned version with gpg against upstream's key committed at `scripts/keys/yt-dlp.asc` (fingerprint and source commit pinned in `sidecars.json`), and requires a signed line pairing each pinned asset name with its pinned sha256. CI's dedicated `verify-ytdlp` job runs it on every PR, and `test` needs that job, so no OS runs the pinned yt-dlp before its signed sums are checked. Locally it's optional and needs gpg. Normal fetches don't use gpg.
  - `scripts/build-ffmpeg.sh` builds a minimal LGPL `ffmpeg` + `ffprobe` from the signed upstream tarball, with no network protocols.
  - **Built (#30(a)):** the same script installs llama.cpp's official `llama-server` (b11669; CPU builds, Metal on macOS arm64) with exactly the shared libraries it loads, into one folder, `src-tauri/binaries/llama/`, with a `target.txt` naming its triple. It's bundled as a resource (`bundle.resources`: `binaries/llama/` → `llama/`), not an `externalBin`, because upstream's `@loader_path`/`$ORIGIN` rpath and ggml's backend search look beside the executable. `--check` (and `--check-llama <dir>` in CI, on the copy tauri-build makes) runs `--version` and `--list-devices` from that folder. `src-tauri/src/local.rs` finds and runs it (#30(b), above). CI also fetches a pinned 1.2 MB test GGUF (`testModel` in `sidecars.json`, `--test-model <dir>`; never bundled) and runs the real server's lifecycle test on each OS.
- Tauri copies them next to the app binary and strips the triple. `src-tauri/src/sidecar.rs` resolves them there and builds the base yt-dlp argv (`--ignore-config --no-update --ffmpeg-location … --js-runtimes deno:…`).
- Models are never bundled. They're downloaded at runtime into the app-data directory.
- **Planned (#21, M5):** run `yt-dlp -U` at most once a day (configurable). Today the base argv passes `--no-update`, so yt-dlp only changes when the pinned version does.

## Persistence

- `settings.json` in the app config folder (D-020) and `queue.json` in the app data folder (D-025), both written atomically (a temp file, then a rename). A damaged `queue.json` is moved aside, and a save failure shows a warning in the window.
- Download staging folders live in the app cache folder and are removed after each download, or at the next launch if Queue was interrupted (D-014, #45).
- API keys (M4) go in the OS credential store via the `keyring` crate, never in these files. History (P1) is planned for M5.
