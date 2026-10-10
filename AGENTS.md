# Project working agreement

Read `README.md`, `docs/STATUS.md`, and the active PRD (`docs/prds/PRD-001-queue-v1.md`) before changing files. Read `docs/ARCHITECTURE.md`, `docs/DECISIONS.md`, and `docs/BACKLOG.md` as needed.

## Scope and authority

- Work on one milestone or one assigned backlog item at a time. Reference its requirement ID (Q*, D*, N*) and GitHub issue number.
- **Do not commit, push, merge, publish, create releases, or change GitHub settings without the owner's explicit approval for that action.** Once implementation is requested, editing files and running local checks is allowed.
- Approval for one commit doesn't cover later commits. Before asking for one, show the diff summary, the checks you ran, known limitations, and a proposed commit message.
- Inspect the working tree before editing. Never discard unrelated work or overwrite another agent's edits.
- Only one implementation agent writes to this checkout at a time. A reviewer agent may inspect a stable diff read-only.
- The user's current instructions win. If they conflict with a confirmed decision in `docs/DECISIONS.md`, point that out instead of silently changing the decision.

## Engineering

- **Wrap yt-dlp, never fork it.** Bundle the official `yt-dlp` and `ffmpeg` binaries as Tauri sidecars and run them as subprocesses. Don't vendor or patch yt-dlp source.
- Stack: Tauri v2 (Rust) with React, TypeScript, and Tailwind. Pin exact versions and don't silently upgrade toolchains.
- Pass subprocess arguments as an argument array, never through a shell string. URLs and titles are untrusted input.
- Keep queue and naming logic separate from UI. The naming engine is a pure function: metadata + template → filename. It must have unit tests.
- The filename is computed before the download starts and passed to yt-dlp's `-o`. Never overwrite an existing file.
- No telemetry and no accounts. The only network traffic is yt-dlp itself, update checks, and the opt-in LLM naming call. API keys go in the macOS Keychain, never in files or logs.
- Prefer small, understandable code over speculative abstractions or plugin systems.
- Never report a check as passed if you didn't run it. Say which platforms you actually tested.

## Handoff after every work session

Update `docs/STATUS.md` with:
- the active requirement IDs and issues
- what changed
- the checks you actually ran
- open problems
- the next bounded action

Record confirmed decisions in `docs/DECISIONS.md`. Keep `docs/BACKLOG.md` and the GitHub issues in sync.
