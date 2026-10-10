# Queue

A lightweight desktop app for queuing video and audio downloads. Paste links, press **+**, then hit **Download all**. Queue runs [yt-dlp](https://github.com/yt-dlp/yt-dlp) for each link, and every file gets a clean, human-readable name built from the page's metadata and tidied up by an on-device AI model, so there's nothing to rename by hand.

> **Status:** M1 (walking skeleton) in progress. The app opens an empty window. See [docs/STATUS.md](docs/STATUS.md).

## Docs

| Doc | Purpose |
|---|---|
| [PRD-001](docs/prds/PRD-001-queue-v1.md) | Product requirements for v1 |
| [BACKLOG](docs/BACKLOG.md) | Milestones M1–M5 + Later → work items → GitHub issues |
| [ARCHITECTURE](docs/ARCHITECTURE.md) | Stack, components, data flow |
| [DECISIONS](docs/DECISIONS.md) | Decision log |
| [STATUS](docs/STATUS.md) | Where things stand and what's next |
| [QA](docs/QA.md) | Independent QA with Codex (GPT-6.1-Sol) |
| [AGENTS](AGENTS.md) | Working agreement for AI agents and contributors |

## Development

Toolchain versions are pinned in the repo (D-010): Node in `mise.toml`, Rust in `rust-toolchain.toml`.

```sh
# one-time setup on macOS (use the equivalent installers on Windows and Linux)
brew install mise rustup
# put the pinned Node and Homebrew's keg-only rustup on PATH (add both lines to ~/.zshrc)
eval "$(mise activate zsh)"
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
mise install                       # installs the pinned Node; rustup reads rust-toolchain.toml on first cargo run
npm ci
scripts/build-ffmpeg.sh aarch64-apple-darwin   # minimal LGPL ffmpeg/ffprobe from source (~30 s; use your target triple)
node scripts/fetch-sidecars.mjs --check         # pinned yt-dlp + deno, checks yt-dlp sees all of them

npm run tauri dev                  # opens the app window
npm run build && npm test          # frontend build + scaffold tests
```

CI (`.github/workflows/build.yml`) runs the checks on every PR and every push to `main`, and builds unsigned bundles for all four targets on every push to `main` (D-011).

Linux also needs the [Tauri system dependencies](https://tauri.app/start/prerequisites/) (WebKitGTK and friends). Windows needs WebView2 and the MSVC build tools. `build-ffmpeg.sh` runs on macOS and Linux only; it cross-compiles the Windows build with mingw-w64. On Windows, copy the `ffmpeg-*` artifact from a CI run into `src-tauri/binaries/` instead.

Bundled third-party programs and their licenses are listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Planned stack

Tauri v2 (Rust) · React + TypeScript + Tailwind · bundled `yt-dlp`, `ffmpeg`, and `llama.cpp` sidecars · macOS, Windows, and Linux. AI naming runs on a local model by default, with an optional API provider.

## Layout

```
src/          React UI
src-tauri/    Rust core and Tauri config
assets/        icons, design mockups (see assets/README.md)
docs/          PRD, backlog, architecture, decisions, status, QA
qa/           Codex QA prompts; reports (git-ignored)
scripts/      codex-qa and (later) build helpers
.claude/       project subagents
.github/       issue and PR templates
```

## Legal

Queue is for personal use. It does not get around DRM. You are responsible for following the terms of service of the sites you download from.
