#!/usr/bin/env bash
# Run the CI test job's checks on the Linux box before pushing (#41). Syncs this working tree
# (uncommitted changes included) to the box over SSH, then runs the same steps as CI's
# `test (linux)` job there.
#
#   scripts/test-linux.sh            default host: chrisbeast (an alias in ~/.ssh/config)
#   QUEUE_LINUX_HOST=other scripts/test-linux.sh
#   QUEUE_LINUX_XVFB=1 scripts/test-linux.sh     run the tests under xvfb-run (no test needs a display yet)
#
# The box needs the Tauri build packages, rustup, and mise (docs/QA.md section C). The box's
# own Linux sidecars, node_modules and Rust build cache stay there between runs; they're never
# synced over or deleted.
set -euo pipefail

HOST="${QUEUE_LINUX_HOST:-chrisbeast}"
DEST="${QUEUE_LINUX_DIR:-queue-test}"   # relative to the remote user's home
ROOT="$(git -C "$(dirname "$0")/.." rev-parse --show-toplevel)"
TARGET=x86_64-unknown-linux-gnu

echo "==> Syncing $ROOT to $HOST:~/$DEST"
# --delete keeps the box an exact copy of this tree, except the paths it owns (excluded here,
# so they're neither copied nor removed).
rsync -az --delete \
  --exclude '/.git/' \
  --exclude '/node_modules/' \
  --exclude '/dist/' \
  --exclude '/qa/reports/' \
  --exclude '/.claude/' \
  --exclude '/src-tauri/target/' \
  --exclude '/src-tauri/binaries/' \
  --exclude '/src-tauri/gen/' \
  "$ROOT/" "$HOST:$DEST/"

echo "==> Running the checks on $HOST"
# The remote script gets its settings as arguments, never by pasting them into the command.
ssh "$HOST" bash -s -- "$DEST" "$TARGET" "${QUEUE_LINUX_XVFB:-0}" <<'REMOTE'
set -euo pipefail
DEST="$1"; TARGET="$2"; XVFB="$3"
cd "$HOME/$DEST"
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
eval "$(mise activate bash --shims)"   # the pinned Node from mise.toml
mise trust --quiet . >/dev/null 2>&1 || true
mise install --quiet

# Sidecars: built once per ffmpeg/manifest version and kept on the box.
mkdir -p src-tauri/binaries
if [ ! -x "src-tauri/binaries/queue-ffmpeg-$TARGET" ]; then
  echo "==> Building ffmpeg for $TARGET (first run only; several minutes)"
  scripts/build-ffmpeg.sh "$TARGET"
fi
node scripts/fetch-sidecars.mjs --target "$TARGET" --check

rustup toolchain install   # the version pinned in rust-toolchain.toml
run() { if [ "$XVFB" = 1 ]; then xvfb-run -a "$@"; else "$@"; fi; }

echo "==> Frontend"
npm ci --no-audit --no-fund
npm run build
run npm test

echo "==> Rust"
cd src-tauri
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
run cargo test --locked
echo "==> Real-sidecar loopback tests"
QUEUE_QA_REAL_SIDECARS="$PWD/binaries" run cargo test --locked --test download_real_d2 -- --ignored --test-threads=1
echo "==> All Linux checks passed on $(hostname)"
REMOTE
