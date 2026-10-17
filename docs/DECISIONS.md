# Decisions

Status values: **Confirmed** (owner agreed) · **Provisional** (sensible default; can change with rationale).

## D-001 Wrap yt-dlp; don't fork it — Confirmed (2026-10-10)
yt-dlp is designed to be driven by other programs and already pulls page metadata. Forking it would mean taking on its constant fixes for site changes. We bundle the upstream binary and run it as a subprocess.

## D-002 Cross-platform; Tauri v2 + React/TS for the desktop shell — Confirmed (2026-10-10)
The owner wants macOS, Windows, and Linux. Options compared:
- **Tauri v2 (chosen):** ~10 MB shell, first-class sidecar binaries (yt-dlp, ffmpeg, llama-server), official dialog/notification/updater plugins, and a Rust backend that gives native keychain access. Trade-off: each OS uses a different system webview (WebKitGTK on Linux is the weakest), so the app is built for all three in CI. UI behavior tests run in jsdom (simulated DOM); native-webview smoke tests are still a gap (see STATUS).
- **Electron:** the same Chromium everywhere and a huge ecosystem, but a 150 MB+ install and much higher memory use, which goes against the "lightweight" goal. This is the fallback if webview differences become a real problem.
- **Flutter desktop:** good rendering, but a weaker subprocess/sidecar story, and Dart adds a language without any gain here.
- **Wails (Go):** a similar model to Tauri, but a smaller ecosystem and a less mature v3.
- **Qt:** heavier C++/licensing overhead for a small app.

## D-003 Compute the filename before download and pass it via `-o` — Provisional (2026-10-10)
yt-dlp writes the final name directly, so there's no separate rename step and no window where a file has the wrong name. Users can edit the proposed name inline before the download starts. *Amended by D-014:* the name is still decided up front, but yt-dlp writes it into a private work dir, and the app then moves the finished file into place without overwriting.

## D-004 Backlog lives in `docs/BACKLOG.md` and GitHub Issues — Confirmed (2026-10-10)
BACKLOG.md is the source of truth for scope and order. Each item links to a GitHub issue that tracks the work.

## D-005 Private GitHub repo `clmcclease/queue` — Confirmed (2026-10-10)

## D-006 AI naming in v1: local model by default, API optional — Confirmed (2026-10-10)
Provider toggle: Local (default) / API / Off, similar to OpenWhispr. Local uses a bundled llama.cpp `llama-server` sidecar plus a small GGUF model that is downloaded on first run with consent. API supports Anthropic and any OpenAI-compatible endpoint. Since llama-server exposes an OpenAI-compatible API, Local and API share one client. Tier 1 rules always sanitize the AI output and act as the fallback. The default model will be chosen by the eval set (N9).

## D-007 Let yt-dlp choose formats — Confirmed (2026-10-10)
The default preset passes no `-f` and forces no container. Audio only uses `-x` without re-encoding.

## D-008 No subfolders in v1 — Confirmed (2026-10-10)
One flat output folder. Subfolders by channel/artist (N5) are in the Later backlog.

## D-009 Independent QA with Codex (GPT-6.1-Sol, high effort) — Confirmed (2026-10-10)
A non-Claude model does QA so the code isn't only checked by the model that wrote it. It's wired in two ways: a `codex-qa` Claude Code agent that runs `scripts/codex-qa` (review, test, or audit), and interactive Codex in a second terminal. See docs/QA.md.

## D-010 Toolchain pinning: mise for Node, rustup for Rust — Confirmed (2026-10-10)
- Node 24.21.0 (Active LTS), pinned in `mise.toml` and `package.json` `engines`. `.npmrc` sets `save-exact` and `engine-strict`.
- Rust 1.99.0, pinned in `rust-toolchain.toml`, which rustup and CI both read.
- npm packages are pinned to exact versions, with `package-lock.json` committed. Direct crates are pinned with `=` in `Cargo.toml`, with `Cargo.lock` committed.
- TypeScript stays on 6.0.3, the line create-tauri-app ships, rather than the new native 7.x compiler. Moving to 7.x is a deliberate later upgrade.
- Upgrades are explicit changes with their own commit, never silent.

## D-011 CI: checks on every change, OS bundles on main — Confirmed (2026-10-10; checks on all 3 OSes since D-023)
- `.github/workflows/build.yml` runs the checks (`npm test`, `cargo fmt`, clippy, `cargo test`) for every PR and every push to `main` (once on Linux until D-023; now on Linux, Windows and macOS). Pushes to other branches without a PR don't run CI, which avoids running every PR twice (owner confirmed 2026-10-10).
- All 4 bundles build on every push to `main` and upload unsigned workflow artifacts: macOS arm64, macOS x64 (cross-compiled on the arm64 runner), Windows x64, and Linux x64. A PR only bundles when it has the `ci:bundle` label. The repo is private, and macOS minutes bill at 10× and Windows at 2×. The first hosted runs cost about 80 billed minutes per full run with a warm cache and about 110 cold, measured on PR #34.
- Runner images are pinned (`macos-26`, `windows-2025`, `ubuntu-24.04`), not `-latest`. Actions are pinned to commit SHAs. Node comes from `package.json` `engines` and Rust from `rust-toolchain.toml`.
- CI changes are tested on a branch with a draft PR before they go to `main`.
- Each push to `main` gets its own concurrency group, so queued runs (and their artifacts) are never dropped. A new push to a PR cancels that PR's older run.

## D-012 Sidecars: yt-dlp onefile, ffmpeg built from source, bundled deno — Confirmed (2026-10-10)
Research for #2 (ytdlp-expert, live sources) and owner choices:
- **yt-dlp:** the official onefile stable build per OS (`yt-dlp_macos` universal2, `yt-dlp.exe`, `yt-dlp_linux`), pinned in `scripts/sidecars.json` with SHA-256 values from upstream `SHA2-256SUMS`. Upstream tags are permanent, so there's no mirror. Each call spends about 2 s unpacking (measured on macOS arm64); onedir would avoid that, but it doesn't fit `externalBin` and adds signing work. Revisit if it feels slow.
- **ffmpeg/ffprobe:** no trustworthy pinned LGPL macOS build exists, and BtbN's LGPL Windows/Linux builds are about 140 MB per binary, with tags deleted after 2 years. So we **build the official signed source tarball (9.0.2), unmodified,** with one minimal LGPL configuration (`scripts/build-ffmpeg.sh`). That's about 3.7 MB per binary. Networking is off (no TLS/HLS/DASH): yt-dlp downloads normally, and only live streams and `--download-sections` would need it. There's no mp3 encoder, since audio-only keeps the native format (D-007). This replaces "bundle the official ffmpeg binaries" in AGENTS.md and PRD §8; upstream publishes no official binaries.
- **JS runtime:** recent yt-dlp needs one for full YouTube support. The owner chose **deno** (pinned v2.9.7, recommended and enabled by default upstream, with the strongest sandbox) over QuickJS-NG (about 2 MB). Cost: about 40 MB compressed per platform.
- **Wiring:** Tauri `externalBin` sidecars named `queue-yt-dlp`, `queue-ffmpeg`, `queue-ffprobe`, `queue-deno`. The prefix keeps the Linux `.deb` from colliding with distro `/usr/bin/yt-dlp` and `/usr/bin/ffmpeg`. yt-dlp is always run with `--ignore-config --no-update --ffmpeg-location <queue-ffmpeg> --no-js-runtimes --js-runtimes deno:<queue-deno>`. yt-dlp derives `queue-ffprobe` from the ffmpeg file name, which was verified in its source and on macOS.
- **Size:** the macOS arm64 `.dmg` is **80.8 MB** before llama-server: deno is 81 MB of the 123 MB installed, and yt-dlp is 37 MB. That broke PRD §7's ~80 MB target, which D-013 raises.
- **Licenses:** yt-dlp's PyInstaller builds are GPLv3+ (shipped as a separate program), ffmpeg is LGPL-2.1+ (we ship the exact source URL and configure line), and deno is MIT. See `THIRD_PARTY_NOTICES.md`. The notices are shown in the app with the About screen (M5).

## D-013 Installer size budget: under 256 MB — Confirmed (2026-10-10)
The owner raised PRD §7's installer budget from about 80 MB to **under 256 MB per platform**. The limit covers yt-dlp, ffmpeg/ffprobe, deno, and the llama.cpp runtime; the AI model stays a separate first-run download. Reason: bundled deno (D-012) put the macOS `.dmg` at 80.8 MB before llama-server. Keep the other "lightweight" goals: cold launch under 1 s and idle memory under 150 MB.

## D-014 Staged downloads with no-clobber placement — Confirmed (2026-10-10)
Codex QA on #4 showed that letting yt-dlp write straight into the user's folder can't be made safe: merge-temp names like `Foo.temp.webm` can clobber real files, the printed path breaks on non-UTF-8 Windows consoles, and name checks miss case- and Unicode-equivalent names. So each download runs with `-P <fresh dir under the app cache>`. The one finished file is then placed in the output folder with `fs::hard_link` (atomic, fails if the name exists), falling back to a `create_new` copy across volumes; `rename` is never used because it replaces files. The work dir is always removed. The owner approved this with #4 and tested it in the app. Consequences: no `.part` files appear in the user's folder; the naming engine stays pure, and collision choice (#16) belongs to the download layer. Follow-up done in #45: at startup, the copy that owns the queue lock deletes `downloads/item-*` work dirs and `info/session-*` saved-info folders left by earlier runs (real folders only, inside real `downloads`/`info` folders; never symlinks at either level). Each folder and its parent are checked again right before deletion. Accepted limitation (owner, 2026-10-15): a process that can already write the cache could still swap a folder in the sub-millisecond gap between that check and the delete. Handle-based no-follow deletion was judged disproportionate. Quitting (#67): the cross-volume copy stops between 1 MiB chunks and removes its partial file. Accepted limitation (owner, 2026-10-17), to be fixed in M2 by #72: a write that blocks past the 5 s quit wait (a slow or disconnected drive), or a crash mid-copy, can still leave a cut-off file under the final name, because the copy writes the final name directly.

## D-015 Optional "Most compatible (MP4)" preset — Confirmed (2026-10-11)
D-007's default (yt-dlp decides) often yields `.webm` (AV1/VP9 + Opus), which QuickTime and Finder preview can't play. #11 adds an optional preset (the default stays "yt-dlp decides"). The flags come from ytdlp-expert, verified against yt-dlp 2026.08.19 with our minimal ffmpeg on macOS; AVFoundation reported every output playable:
- **Video:** `-S vcodec:h264,vext:mp4,lang,quality,res,fps,hdr:12,acodec:aac,aext:m4a --merge-output-format mp4/webm/mkv`. A format sort rather than an `-f` filter, so something always downloads. It's **strict H.264** (owner's choice): H.264 wins even at low resolution, and on YouTube H.264 usually tops out at 1080p. Without H.264 it falls back to VP9/AV1 in `.webm` or `.mkv`, so the extension stays honest.
- **Audio-only:** `-f ba/b -S acodec:aac,aext:m4a,vext:mp4 -x`. AAC is stream-copied to `.m4a`; Opus sources stay `.opus`.
- **Never** use `-t mp4`, `--remux-video mp4`, a forced `--merge-output-format mp4`, `--audio-format m4a`, or `-t aac`. Our ffmpeg has no VP9 decoder, so VP9 remuxed into MP4 is **silently corrupt** (empty `vpcC`, yet yt-dlp exits 0), and an audio conversion needs an encoder we don't ship.
- **No H.264 available (owner's choice):** keep the honest `.webm`/`.mkv` and show a row notice such as "Not QuickTime-compatible (VP9/Opus)". No re-encoding, which keeps D-012's minimal LGPL build.
- **ffmpeg additions (owner's choice), both LGPL:** `--enable-decoder=vp9`, defense in depth against the corrupt-MP4 trap, and `--enable-demuxer=avi`, for sites like archive.org that serve `.avi`, which today's default can fail on. Done with #11. Measured on macOS arm64: `ffmpeg` grew 3.69 → 4.27 MB (+15.5%) and `ffprobe` 3.51 → 4.10 MB (+16.8%), about +1.2 MB per platform before compression, well inside D-013's 256 MB.
- **Implemented (#11):** four choices in Settings: Default, Most compatible video (MP4), Audio only, Most compatible audio (M4A). `download::Format` maps each to exactly the flags above. The fallback notice is OS-neutral rather than "Not QuickTime-compatible" (AGENTS.md: no OS-specific wording): "No H.264 version was available, so it's saved as .webm. Some players can't open it." It shows on the finished row and is decided from the codecs yt-dlp reports choosing (`vcodec|acodec`), since an `.mp4` can hold AV1. It falls back to the extension when they're unknown. Running downloads keep the format they started with.

## D-016 Default local naming model: Qwen3-4B-Instruct-2507 — Provisional (2026-10-11)
Research of current small GGUF models (sources in the #33 issue). The owner accepted the size; the N9 eval (#33) confirms the choice on the pinned files.
- **Default:** Qwen3-4B-Instruct-2507 Q4_K_M (2.50 GB, Apache-2.0; reputable GGUFs from bartowski, unsloth, and lmstudio-community; no official Qwen GGUF). It has the best instruction following without a thinking mode in this size range (IFEval 83.4) and strong multilingual scores. About 2.5 s per item on a base M1 (estimate), but 5–6 s on CPU-only machines.
- **Small option:** Qwen3.5-2B Q4_K_M (1.28 GB, Apache-2.0). Suggested automatically when there's no GPU backend or under 8 GB of RAM; the user can switch either way (#31).
- **Rejected:** Llama 3.2 (branding and AUP pass-through), Gemma 3 (restrictive terms; Gemma 4 E2B is 3.1 GB), Qwen2.5-3B (non-commercial `qwen-research` license), and weaker multilingual options.
- **Runtime notes for #30/#33:**
  - llama-server on `127.0.0.1` with a random port and a per-launch `--api-key`; `-c 2048 --parallel 1`, `-ngl 99` on GPU, and `--sleep-idle-seconds 120` (N6).
  - Reasoning off (`--reasoning off` plus `chat_template_kwargs.enable_thinking=false`).
  - JSON-schema `response_format` (`filename`, 1–150 chars); temperature 0–0.2, `max_tokens` ~128.
  - Metadata is passed as data, never as instructions.
  - Pin by HF repo commit plus the file's LFS sha256.
- **Consequence:** PRD §7's "~1–2 GB" model download becomes ~2.5 GB for the default (1.3 GB for the small option).

## D-017 YouTube 403s: retry and reuse fetched info; stay on stable yt-dlp — Confirmed (2026-10-11)
The owner hit intermittent `HTTP Error 403: Forbidden` on downloads. ytdlp-expert reproduced it on 2026.08.19: YouTube sometimes rejects a `visionos`-client https link, and yt-dlp never retries 4xx errors. It isn't PO tokens (default clients don't need them; the "GVS PO Token experiment" log line concerns the unused `web` client) and isn't rate limiting (9 extractions in 10 min were fine). The nightly's new `tv_samsung` client fixed the case, but there's no newer stable release yet.
- **Chosen:** retry a 403 item up to 2 times with backoff (about 10 s, then 30 s), and download from the saved `-J` via `--load-info-json` within its ~6 h link lifetime, falling back to a fresh extraction (#48). Stay on stable; adopt the next stable that includes `tv_samsung` (via the pin bump process, #21/#44).
- **Rejected for now:** pinning a nightly (less tested, changes daily); bundling a PO-token provider (bgutil: GPL-3.0, needs a server and native deps, and doesn't address this failure); hard-coding player clients (yt-dlp's defaults change per release).
- **Metadata fetches stay one at a time** (owner's choice): gentlest on YouTube.
- **Process-tree stop (#6):** SIGTERM the group first, then SIGKILL after 3 s. A SIGKILL alone leaks the onefile launcher's ~70 MB `_MEI*` unpack dir every time. Windows uses a Job Object.

*D-014/D-017 implementation notes (#48, 2026-10-11):* retries of one download share **one** fresh private work dir, so finished streams and `.part` files are reused (D-014 still holds: the dir is per download, created fresh, removed at the end). yt-dlp reports the finished path via `--print-to-file after_move:filepath queue-result.txt`, a UTF-8 file in the work dir (console encodings vary). Queue uses that path only if it's a regular file inside the work dir; otherwise it falls back to the strict one-file rule, ignoring `.part`/`.ytdl` leftovers. Only a 403 **reported by yt-dlp** is retried, never Queue's own errors. Saved info is used while under 5 h old (a margin before the ~6 h expiry).

## D-018 Name collisions keep both: ` (2)`, ` (3)`… (N2, #16) — Confirmed (2026-10-11)
The owner hit "Not saved: … already exists" during a Download all and asked for "keep both" instead. PRD N2 ("append ` (2)`, never overwrite") is pulled forward from M3 into #7. When the finished file's name is taken, `download::place_unique` tries `Title (2).ext`, `(3)`, … up to `(999)`. Each candidate is claimed with D-014's atomic no-clobber `place` (hard link, else a `create_new` copy), so neither an existing file nor a concurrent download is ever overwritten. Case- and Unicode-equivalent names count as taken where the filesystem says so. The second copy is numbered `(2)`, matching Finder and Explorer. When the naming engine (#14) computes names before download, collision choice stays here in the download layer (ARCHITECTURE). Names near the 255-byte limit still fit: `numbered` shortens the stem (never the extension) at a character boundary, and yt-dlp runs with `--trim-filenames 200`, leaving room for its own `.part`/`.f137`/`.temp` suffixes (a 250-char title used to fail outright).

## D-019 Download all doesn't wait behind rows still fetching info — Confirmed (2026-10-11)
`queue::run_all` takes the next **Queued** row by id. A row still Fetching (up to the 90 s limit) is waited for at the end but doesn't hold up ready rows, so one slow or failing fetch can't stall a batch. Among ready rows the order is strict. Codex QA noted that this differs from strict insertion order; the owner can switch to strict order later if wanted.

## D-020 Settings live in one JSON file in the app config folder — Provisional (2026-10-13)
For #10 (D1), and as the base for the settings screen (#36): `settings.json` sits in Tauri's `app_config_dir`. It's written atomically (a temp file, then a rename) and read with defaults for any missing or damaged value, so a bad file never stops the app from starting. API keys never go here; they use the OS credential store (AGENTS.md). The folder picker is the official `tauri-plugin-dialog` (=2.8.1, the stable 2.x line; 3.0 is alpha), called from a Rust command, so the webview gets no new permission. A chosen download folder that no longer exists (e.g. an unplugged drive) fails the download with a clear reason instead of being recreated as an empty local folder. With no folder chosen, the OS Downloads folder is used and created if needed.

## D-021 QA-written tests are added by default — Confirmed (2026-10-13)
The owner said "always add them from this point forward" about the tests that `codex-qa test` writes. Claude applies the tests patch without its `docs/STATUS.md` edit, once the tests pass against any fixes, and reports which tests were added. This replaces "offer, never merge automatically" in CLAUDE.md and docs/QA.md.

## D-022 Open folder and Show in folder go through Queue's own commands — Provisional (2026-10-13)
For #39 (D7): the actions go through two of Queue's own commands. `tauri-plugin-opener` (=2.7.0, the stable 2.x line; 3.0 is alpha) is used only on Linux, and only for one Rust function. It's never registered, so the webview gets no opener permission. `open_download_folder` takes no arguments and always opens the current download folder. `reveal_download` takes a row id and reveals that Done row's saved file, after checking it still exists. The UI can't name a path, which is stricter than scoping the plugin to the download folder. Wording is per OS (`src/platform.ts`): "Show in Finder" on macOS, "Show in File Explorer" on Windows, "Show in folder" on Linux. The Open folder tooltip names Finder, File Explorer or "your file manager". Both commands are async, and all blocking work runs off the main thread. Opening and revealing are Queue's own `reveal.rs`, because opener 2.7.0 canonicalizes on the macOS main thread, drops Windows shell errors, reports detached launches as successes, and has a broken Linux portal fallback. Every launch reports its failure. macOS uses `open` and `open -R`, with the exit status checked. Windows uses `ShellExecuteExW` and `SHOpenFolderAndSelectItems`, with COM initialized on the worker thread. Linux uses `xdg-open` (its exit status is awaited on a worker, with no cutoff) and opener's `FileManager1` reveal (the plugin is a Linux-only dependency). When Linux can only open the folder, the user is told with a notice.

## D-023 Tests run natively on Linux, Windows and macOS; the repo goes public — Confirmed (2026-10-14)
At the owner's request (#41), the `test` job in `build.yml` runs the full checks on `ubuntu-24.04`, `windows-2025` and `macos-26` (arm64) for every PR and every push to `main`. That's `npm run build`, `npm test`, `cargo fmt`, clippy `-D warnings`, `cargo test --locked` and the real-sidecar loopback tests (`download_real_d2 --ignored`, no external network), with `fail-fast: false` and each OS using its own sidecars. This replaces D-011's Linux-only checks; bundles are unchanged. Rust stays pinned in `rust-toolchain.toml` (D-010), not "stable".
- **Why public:** the matrix (and the existing macOS bundles at 10×) used up the private plan's 2,000 Actions minutes in 6 days, about 3,100 billed minutes. On 2026-10-14 the owner chose to make the repo public, where standard GitHub-hosted runners cost nothing, rather than ration Windows and macOS runs.
- **No self-hosted runner:** the owner's Ubuntu box briefly served as a runner. It was unregistered before going public: on a public repo, anyone's PR could run code on it (GitHub's guidance). The workflow uses no secrets and no `pull_request_target`.
- **Linux box over SSH:** `scripts/test-linux.sh` still runs the same checks on the box over SSH before a push. Only the owner can trigger it.
- **No xvfb:** no test needs a display (the UI tests use jsdom). `QUEUE_LINUX_XVFB=1` wraps the box runs in `xvfb-run` if one ever does.

## D-024 License: MIT OR Apache-2.0 — Confirmed (2026-10-14)
For the public repo (D-023), the owner chose the usual Rust and Tauri dual license: `LICENSE-MIT` and `LICENSE-APACHE`, with `license = "MIT OR Apache-2.0"` in `Cargo.toml` and `package.json`. The bundled sidecars keep their own licenses: yt-dlp's source is Unlicense, but its official PyInstaller binary is GPLv3+; deno is MIT; and FFmpeg is an LGPL-2.1-or-later build from the official source. As separate programs, they don't change Queue's license. `THIRD_PARTY_NOTICES.md` names them and points to the exact FFmpeg source and build script. Release installers will carry the license texts and a generated dependency-license list (#24). `SECURITY.md` asks for private vulnerability reports. `CONTRIBUTING.md` welcomes issues and asks for an issue before any PR. The README states responsible use: no DRM or access-control circumvention.

## D-025 The queue is saved to queue.json and restored on launch — Confirmed (2026-10-15)
For #12 (Q7), the queue lives in `queue.json` in the app data folder, written atomically by the same writer as `settings.json` (D-020).
- **What's saved:** each row's id, link, state, video info, and finished path or failure reason, plus its notice. Live progress, retry attempts and the saved `-J` info aren't, so a restored row extracts again.
- **When:** a background saver writes within 0.5 s of any change worth saving (live progress doesn't count), plus once more when the app exits. The JSON is made under the queue lock and written outside it.
- **On launch (owner's choices):**
  - Finished and failed rows are kept, with their files, notices and Retry.
  - A download that was running when Queue closed comes back **Queued**, with the note "Interrupted when Queue closed. Download it again when you're ready." It waits for Download or Download all; nothing starts by itself.
  - Rows still fetching their info fetch it again. The duplicate check is rebuilt from each saved link. Rows whose link no longer validates are dropped, and new ids continue after the highest saved one.
  - A damaged `queue.json` is moved aside to `queue.json.bad`, or a unique `queue.json.bad-<time>` if that name is taken, and the queue starts empty. If it can't be moved, or an existing file can't be read at all, queue saving is off for that session, so it's never overwritten.
  - Ids never wrap: adding at the limit fails with a message, and saved rows with impossible ids are dropped.
- **One running copy (owner's choice):** the official `tauri-plugin-single-instance` (=2.5.1) brings the open window to the front when Queue is launched again, matching the PRD's single window. As a backstop, only the copy holding an exclusive OS lock on `queue.lock` (`std::fs::File::try_lock`, held for the whole run) saves the queue. Startup waits up to 10 s for that lock before loading, since a quitting copy may still be finishing its final save. If the queue can't be saved for any reason, the window says so; it's never silent.
