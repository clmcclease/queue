# QA

QA comes from an independent, non-Claude model, **Codex CLI with `gpt-6.1-sol` at high reasoning effort**, so code isn't only checked by the model that wrote it. Codex reads `AGENTS.md` itself, so it follows the same working agreement.

There are two ways to run it.

## A. From Claude Code: the `codex-qa` agent

Claude delegates to `.claude/agents/codex-qa.md`, which runs `scripts/codex-qa` and relays the report verbatim.

| Mode | Command | Sandbox | When |
|---|---|---|---|
| `review` | `scripts/codex-qa review [--base main \| --commit <sha>]` | read-only (`codex exec`; `codex review` can't take a custom checklist) | **Required** before asking for commit approval on any backlog item |
| `test` | `scripts/codex-qa test "#14"` | snapshot worktree (HEAD + uncommitted + untracked), workspace-write + network | After implementing an item: runs the checks, verifies acceptance criteria, writes the missing unit tests |
| `audit` | `scripts/codex-qa audit` | read-only | At the end of each milestone, or when "is everything set up right?" comes up |

- Reports are saved in `qa/reports/` (git-ignored).
- Tests that Codex writes are exported to `qa/reports/<run>.tests.patch`, and the worktree is kept for inspection (the script prints its path and the remove command). By the owner's standing approval (2026-10-13), Claude applies the patch without its `docs/STATUS.md` edit (`git apply --exclude=docs/STATUS.md <patch>`) once its tests pass against any fixes, then removes the worktree.
- The script closes Codex's stdin itself, so a caller holding a pipe open can't hang it (#122), and a log holding only `Reading additional input from stdin...` with no Codex banner means Codex was never reached: a launch defect, not a Codex failure (D-035).
- If Codex fails (auth, model, timeout), the script exits non-zero and prints the log path plus the last log lines. An empty report counts as a failure (exit 3). After a run, the last line is `codex-qa exit: <status>`, so a run whose output was piped (and so lost its exit code) still shows the result; run the script unpiped.
- To override the model, effort, or timeout: `CODEX_QA_MODEL=… CODEX_QA_EFFORT=… CODEX_QA_TIMEOUT=<seconds> scripts/codex-qa …` (the default timeout is 1500s).
- Prompts are in `qa/prompts/` and can be edited to tune what QA checks.

## B. Interactive Codex in a second terminal

Use this for investigations that need back-and-forth:

```sh
cd ~/Developer/queue
codex -m gpt-6.1-sol -c model_reasoning_effort='"high"' -s read-only
```

Use `-s workspace-write --worktree` if you want it to write tests without touching the main checkout. Any findings worth keeping go into a GitHub issue or `docs/STATUS.md`, so they aren't lost between terminals.

## C. Linux before pushing: the Linux box (#41, D-023)
`scripts/test-linux.sh` syncs this working tree (uncommitted changes included) to the box over SSH and runs CI's `test (linux)` steps there. Set `QUEUE_LINUX_HOST` to use another host (default `chrisbeast`), or `QUEUE_LINUX_XVFB=1` to run the tests under `xvfb-run`. The box keeps its own Linux sidecars (ffmpeg is built on the first run), `node_modules` and Rust cache.

The box needs:
- the Tauri build packages (`libwebkit2gtk-4.1-dev build-essential file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev pkg-config xvfb`), plus `curl`, `rsync`, `xz-utils` and `zlib1g-dev` for the ffmpeg build;
- `rustup`, and `mise` for the pinned Node.

## Rules

- QA never commits, pushes, or edits the main checkout.
- A QA verdict of FAIL blocks the commit request until the problem is fixed or the owner explicitly accepts it.
- The PR or commit summary says which QA modes were run and what their verdicts were. Never claim a QA run that didn't happen.
