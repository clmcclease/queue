# Queue

A lightweight desktop app for queuing video and audio downloads. Paste links, press **+**, then hit **Download all**. Queue runs [yt-dlp](https://github.com/yt-dlp/yt-dlp) for each link, and every file gets a clean, human-readable name built from the page's metadata and tidied up by an on-device AI model, so there's nothing to rename by hand.

> **Status:** M0 (scaffold and PRD). No app code yet. See [docs/STATUS.md](docs/STATUS.md).

## Docs

| Doc | Purpose |
|---|---|
| [PRD-001](docs/prds/PRD-001-queue-v1.md) | Product requirements for v1 |
| [BACKLOG](docs/BACKLOG.md) | Milestones M1–M5 + Later → work items → GitHub issues |
| [ARCHITECTURE](docs/ARCHITECTURE.md) | Stack, components, data flow |
| [DECISIONS](docs/DECISIONS.md) | Decision log |
| [STATUS](docs/STATUS.md) | Where things stand and what's next |
| [AGENTS](AGENTS.md) | Working agreement for AI agents and contributors |

## Planned stack

Tauri v2 (Rust) · React + TypeScript + Tailwind · bundled `yt-dlp`, `ffmpeg`, and `llama.cpp` sidecars · macOS, Windows, and Linux. AI naming runs on a local model by default, with an optional API provider.

## Layout

```
assets/        icons, design mockups (see assets/README.md)
docs/          PRD, backlog, architecture, decisions, status
.claude/       project subagents
.github/       issue and PR templates
```

## Legal

Queue is for personal use. It does not get around DRM. You are responsible for following the terms of service of the sites you download from.
