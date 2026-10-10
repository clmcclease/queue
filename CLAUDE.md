@AGENTS.md

## Claude Code notes

- Project subagents live in `.claude/agents/`:
  - `codex-qa`: independent QA by Codex (GPT-6.1-Sol, high effort). See [docs/QA.md](docs/QA.md).
  - `reviewer`: Claude's own read-only review of a stable diff.
  - `ytdlp-expert`: yt-dlp flags, format selection, output templates.
- **QA gates (required):**
  1. After implementing a backlog item, run `codex-qa` in `test "#<issue>"` mode. Report its verdict, then offer to bring in any tests it wrote (never merge them automatically).
  2. Before asking for commit approval, run `codex-qa` in `review` mode. A FAIL blocks the request until it's fixed or the owner accepts it.
  3. At the end of each milestone, run `codex-qa` in `audit` mode.
  - Include the QA verdicts and report paths in the commit-approval request. QA runs take minutes, so run them once per stable diff, not after every edit.
- Use `gh` for issues, labels, and milestones. Issue titles start with their requirement ID, e.g. `[Q3] Fetch metadata on add`.
- Git attribution follows the session's system instructions.
