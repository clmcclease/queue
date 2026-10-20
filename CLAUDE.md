@AGENTS.md

## Claude Code notes

- Project subagents live in `.claude/agents/`:
  - `codex-qa`: independent QA by Codex (GPT-6.1-Sol, high effort). See [docs/QA.md](docs/QA.md).
  - `reviewer`: Claude's own read-only review of a stable diff.
  - `ytdlp-expert`: yt-dlp flags, format selection, output templates.
  - `architect`: Fable 5.1, read-only. While D-028 is active, it gives the approvals AGENTS.md reserves for the owner (within D-028's limits); ask it instead of the owner and record each ruling in STATUS.
- **QA gates (required):**
  1. After implementing a backlog item, run `codex-qa` in `test "#<issue>"` mode. Report its verdict, then bring in any tests it wrote: apply its tests patch without its `docs/STATUS.md` edit, once the tests pass against any fixes. The owner approved this standing practice on 2026-10-13. Say in the report and in STATUS which tests were added.
  2. Before asking for commit approval, run `codex-qa` in `review` mode. A FAIL blocks the request until it's fixed or the owner accepts it.
  3. At the end of each milestone, run `codex-qa` in `audit` mode.
  - Include the QA verdicts and report paths in the commit-approval request. QA runs take minutes, so run them once per stable diff, not after every edit.
  - If Codex itself fails (auth, model access, sandbox, timeout), retry at most once. Then stop and give the owner the exact error and the log path (`qa/reports/*.log`). The owner keeps Codex open in a second terminal and fixes problems there. Never skip the gate or work around it silently.
- **Starting a session:** read `docs/STATUS.md` first. Its "Next bounded action" is where work resumes.
- Use `gh` for issues, labels, and milestones. Issue titles start with their requirement ID, e.g. `[Q3] Fetch metadata on add`.
- Git attribution follows the session's system instructions.
