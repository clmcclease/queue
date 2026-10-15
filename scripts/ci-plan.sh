#!/usr/bin/env bash
# Decides what a CI run needs, to spend as few paid Actions minutes as possible (D-023). Called by
# build.yml's `plan` job; qa/tests/ci.test.mjs runs it directly.
#
# - Windows and macOS tests (and the macOS ffmpeg build; macOS bills 10x, Windows 2x) run on
#   pushes to main, manual runs, and PRs labelled ci:native or ci:bundle.
# - Linux tests run on the owner's self-hosted box when it's online (free), else on hosted Linux.
#   "Online" needs RUNNER_STATUS_TOKEN (a fine-grained token with this repo's Administration: read)
#   because the workflow token can't list runners; without it the box counts as offline. A PR
#   labelled ci:box uses the box regardless. Fork PRs never use the box: it runs the repo's code.
#
# Inputs (env): EVENT (github.event_name), LABELS (JSON array of PR label names), FORK
# ("true" for a PR from a fork), RUNNER_TOKEN (optional), REPO (owner/name), API (optional base URL).
# Outputs (to $GITHUB_OUTPUT, or stdout): box, native, tests (matrix JSON), ffmpeg (matrix JSON).
set -euo pipefail

EVENT="${EVENT:?}"
LABELS="${LABELS:-[]}"
FORK="${FORK:-false}"
REPO="${REPO:-}"
API="${API:-https://api.github.com}"

has_label() { grep -Fq "\"$1\"" <<<"$LABELS"; }

native=false
case "$EVENT" in push | workflow_dispatch) native=true ;; esac
if has_label ci:native || has_label ci:bundle; then native=true; fi

box=false
if [ "$FORK" != true ]; then
  if has_label ci:box; then
    box=true
  elif [ -n "${RUNNER_TOKEN:-}" ] && [ -n "$REPO" ]; then
    # Any failure (network, token, API) means "offline": hosted Linux then runs instead.
    online=$(curl -fsS --max-time 15 \
      -H "Authorization: Bearer $RUNNER_TOKEN" -H "Accept: application/vnd.github+json" \
      "$API/repos/$REPO/actions/runners" 2>/dev/null |
      jq '[.runners[] | select(.status == "online") | select([.labels[].name] | index("ryzen"))] | length' \
        2>/dev/null) || online=0
    if [ "${online:-0}" -gt 0 ]; then box=true; fi
  fi
fi

linux='{"os":"linux","runner":"ubuntu-24.04","target":"x86_64-unknown-linux-gnu"}'
windows='{"os":"windows","runner":"windows-2025","target":"x86_64-pc-windows-msvc"}'
macos='{"os":"macos","runner":"macos-26","target":"aarch64-apple-darwin"}'
ff_linux='{"runner":"ubuntu-24.04","targets":"x86_64-unknown-linux-gnu x86_64-pc-windows-msvc","key":"linux-windows"}'
ff_macos='{"runner":"macos-26","targets":"aarch64-apple-darwin x86_64-apple-darwin","key":"macos"}'

# Plain strings, not arrays: macOS's bash 3.2 (where the tests also run this) trips on an
# empty array under `set -u`.
rows=""
if [ "$box" != true ]; then rows="$linux"; fi
if [ "$native" = true ]; then rows="${rows:+$rows,}$windows,$macos"; fi
tests="[$rows]"
ffmpeg="[$ff_linux]"
if [ "$native" = true ]; then ffmpeg="[$ff_macos,$ff_linux]"; fi

outputs() {
  echo "box=$box"
  echo "native=$native"
  echo "tests=$tests"
  echo "ffmpeg=$ffmpeg"
}
# Plain stdout when run by hand or by the tests: appending to /dev/stdout fails on Linux when
# stdout is a socket.
if [ -n "${GITHUB_OUTPUT:-}" ]; then outputs >>"$GITHUB_OUTPUT"; else outputs; fi
