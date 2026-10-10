# Decisions

Status values: **Confirmed** (owner agreed) · **Provisional** (sensible default; can change with rationale).

## D-001 Wrap yt-dlp; don't fork it — Confirmed (2026-10-10)
yt-dlp is designed to be driven by other programs and already pulls page metadata. Forking it would mean taking on its constant fixes for site changes. We bundle the upstream binary and run it as a subprocess.

## D-002 Cross-platform; Tauri v2 + React/TS for the desktop shell — Confirmed (2026-10-10)
The owner wants macOS, Windows, and Linux. Options compared:
- **Tauri v2 (chosen):** ~10 MB shell, first-class sidecar binaries (yt-dlp, ffmpeg, llama-server), official dialog/notification/updater plugins, and a Rust backend that gives native keychain access. Trade-off: each OS uses a different system webview (WebKitGTK on Linux is the weakest), so the UI is tested on all three in CI.
- **Electron:** the same Chromium everywhere and a huge ecosystem, but a 150 MB+ install and much higher memory use, which goes against the "lightweight" goal. This is the fallback if webview differences become a real problem.
- **Flutter desktop:** good rendering, but a weaker subprocess/sidecar story, and Dart adds a language without any gain here.
- **Wails (Go):** a similar model to Tauri, but a smaller ecosystem and a less mature v3.
- **Qt:** heavier C++/licensing overhead for a small app.

## D-003 Compute the filename before download and pass it via `-o` — Provisional (2026-10-10)
yt-dlp writes the final name directly, so there's no separate rename step and no window where a file has the wrong name. Users can edit the proposed name inline before the download starts.

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

## D-011 CI: checks on every change, OS bundles on main — Confirmed (2026-10-10)
- `.github/workflows/build.yml` runs the checks (`npm test`, `cargo fmt`, clippy, `cargo test`) once on Linux for every PR and every push to `main`. Pushes to other branches without a PR don't run CI, which avoids running every PR twice (owner confirmed 2026-10-10).
- All 4 bundles build on every push to `main` and upload unsigned workflow artifacts: macOS arm64, macOS x64 (cross-compiled on the arm64 runner), Windows x64, and Linux x64. A PR only bundles when it has the `ci:bundle` label. The repo is private, and macOS minutes bill at 10× and Windows at 2×. The first hosted runs cost about 80 billed minutes per full run with a warm cache and about 110 cold, measured on PR #34.
- Runner images are pinned (`macos-26`, `windows-2025`, `ubuntu-24.04`), not `-latest`. Actions are pinned to commit SHAs. Node comes from `package.json` `engines` and Rust from `rust-toolchain.toml`.
- CI changes are tested on a branch with a draft PR before they go to `main`.
- Each push to `main` gets its own concurrency group, so queued runs (and their artifacts) are never dropped. A new push to a PR cancels that PR's older run.
