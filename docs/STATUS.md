# Status

**Updated:** 2026-10-10 · **Milestone:** M0 (scaffold) complete → **M1 next**

## Current state
- Repo scaffolded with the PRD, backlog, architecture notes, decision log, agent working agreement, and GitHub labels, milestones, and issues.
- PRD v2 (2026-10-10) settles the original open questions:
  - cross-platform with Tauri v2 (D-002)
  - AI naming in v1, Local by default with API/Off options (D-006)
  - yt-dlp chooses formats (D-007)
  - no subfolders in v1 (D-008)
- Independent QA is wired in (D-009, docs/QA.md):
  - `scripts/codex-qa` has review, test, and audit modes and uses Codex `gpt-6.1-sol` at high effort.
  - There's a `codex-qa` Claude Code agent, plus a documented second-terminal workflow.
  - QA gates are in CLAUDE.md, AGENTS.md, and the PR template.
- No application code yet.

## Checks performed (2026-10-10, macOS only)
- Live Codex smoke test with `gpt-6.1-sol` at high effort: OK.
- `scripts/codex-qa` against a stub `codex`:
  - invalid arguments are rejected (exit 2)
  - a nonzero Codex exit is reported with the log tail
  - an empty report counts as a failure (exit 3)
  - concurrent runs get unique report files
  - the timeout kills the whole process group with no orphans
  - test-mode snapshot includes uncommitted, untracked, symlinked, and executable files, creates no commits, and the patch contains only QA's additions
- Real `codex-qa review` runs on this change:
  - FAIL: auto-commit, unchecked staging, symlinks. Fixed.
  - PASS WITH NOTES: timeout, stale STATUS. Fixed.
- Not verified:
  - a real `test` or `audit` run (there's no app code to test yet)
  - the `codex-qa` agent invoked through Claude Code. Project agents load when a session starts inside `~/Developer/queue`.
  - Windows and Linux

## Open questions (PRD §13)
1. Which default local model? To be decided by the N9 eval.
2. Windows code signing: buy a certificate, or accept SmartScreen warnings? (#24)

## Next bounded action
In a Claude Code session started in `~/Developer/queue`, confirm the `codex-qa` agent appears. Then start **M1 / #1**: scaffold the Tauri v2 + React/TS app, followed by the cross-platform CI build so problems on any OS show up from day one.
