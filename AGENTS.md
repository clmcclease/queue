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

- **Wrap yt-dlp, never fork it.** Bundle the official `yt-dlp` binary, `ffmpeg`/`ffprobe` built unmodified from the official signed source, and the official `deno` binary as Tauri sidecars (D-012). Run them as subprocesses. Don't vendor or patch yt-dlp or ffmpeg source.
- Stack: Tauri v2 (Rust) with React, TypeScript, and Tailwind. Pin exact versions and don't silently upgrade toolchains.
- Pass subprocess arguments as an argument array, never through a shell string. URLs and titles are untrusted input.
- Keep queue and naming logic separate from UI. The naming engine is a pure function: metadata + template → filename. It must have unit tests.
- The filename is computed before the download starts and passed to yt-dlp's `-o`. Never overwrite an existing file.
- Cross-platform: macOS, Windows, and Linux are all first-class. Don't use OS-specific paths, shortcuts, or wording ("Finder") without per-OS handling. Use the Windows filename rules everywhere.
- AI naming defaults to the local model. Item metadata only leaves the machine when the user picks the API provider. Tier 1 rules always sanitize AI output and act as the fallback.
- No telemetry and no accounts. API keys go in the OS credential store (the keyring crate), never in files or logs.
- Prefer small, understandable code over speculative abstractions or plugin systems.
- Never report a check as passed if you didn't run it. Say which platforms you actually tested.

## QA

Independent QA runs through Codex (`scripts/codex-qa`, documented in `docs/QA.md`):
- `review` is required before any commit request.
- `test "#<issue>"` is required after implementing a backlog item.
- `audit` runs at the end of each milestone.

When acting as QA, never edit the main checkout. Tests are written only in a separate worktree.

## Handoff after every work session

Update `docs/STATUS.md` with:
- the active requirement IDs and issues
- what changed
- the checks you actually ran
- open problems
- the next bounded action

Record confirmed decisions in `docs/DECISIONS.md`. Keep `docs/BACKLOG.md` and the GitHub issues in sync.
