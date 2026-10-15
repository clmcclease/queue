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
TARGET=x86_64-unknown-linux-gnu
XVFB="${QUEUE_LINUX_XVFB:-0}"

# Both reach a remote shell (ssh, and rsync's remote path; macOS's openrsync has no
# --protect-args), so only plain names are accepted rather than escaped.
case "$HOST" in -* | *[!A-Za-z0-9._@-]* | '') echo "QUEUE_LINUX_HOST must be a plain SSH host or alias: $HOST" >&2; exit 2 ;; esac
# DEST gets `rsync --delete`, so it must be a dedicated folder strictly inside the remote home:
# every segment starts with a letter or digit (no ".", "..", hidden folders or the home itself).
if ! [[ "$DEST" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*(/[A-Za-z0-9][A-Za-z0-9._-]*)*$ ]]; then
  echo "QUEUE_LINUX_DIR must be a plain folder under the remote home (segments start with a letter or digit): $DEST" >&2
  exit 2
fi
case "$XVFB" in 0 | 1) ;; *) echo "QUEUE_LINUX_XVFB must be 0 or 1" >&2; exit 2 ;; esac
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Before any --delete: on the box, the folder must resolve (symlinks followed) to a path strictly
# inside the home, and if it already has files it must be one this script made (its marker).
# shellcheck disable=SC2029  # expanding (and quoting) here, on the Mac, is the point
ssh "$HOST" "bash -s -- $(printf '%q ' "$DEST")" <<'GUARD'
set -euo pipefail
DEST="$1"; MARK=.queue-test-checkout
home="$(realpath "$HOME")"
mkdir -p "$HOME/$DEST"
real="$(realpath "$HOME/$DEST")"
case "$real" in "$home"/?*) ;; *) echo "Refusing: $DEST resolves to $real, not a folder inside $home" >&2; exit 3 ;; esac
if [ ! -e "$real/$MARK" ] && [ -n "$(ls -A "$real")" ]; then
  echo "Refusing: $real has files but wasn't made by scripts/test-linux.sh (no $MARK)" >&2
  exit 3
fi
touch "$real/$MARK"
GUARD

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
  --exclude '/.queue-test-checkout' \
  "$ROOT/" "$HOST:$DEST/"

echo "==> Running the checks on $HOST"
# The remote script gets its settings as arguments, quoted for the remote shell (ssh joins its
# arguments into one command line), never pasted into the script itself.
# shellcheck disable=SC2029  # expanding (and quoting) here, on the Mac, is the point
ssh "$HOST" "bash -s -- $(printf '%q ' "$DEST" "$TARGET" "$XVFB")" <<'REMOTE'
set -euo pipefail
DEST="$1"; TARGET="$2"; XVFB="$3"
cd "$HOME/$DEST"
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
eval "$(mise activate bash --shims)"   # the pinned Node from mise.toml
mise trust --quiet . >/dev/null 2>&1 || true
mise install --quiet

# Sidecars: ffmpeg/ffprobe are kept on the box and rebuilt whenever the build script or the
# manifest changes (like CI's cache key), or either binary is missing.
mkdir -p src-tauri/binaries
stamp="src-tauri/binaries/.ffmpeg-build-$TARGET"
want="$(cat scripts/build-ffmpeg.sh scripts/sidecars.json | sha256sum | cut -d' ' -f1)"
if [ ! -x "src-tauri/binaries/queue-ffmpeg-$TARGET" ] || [ ! -x "src-tauri/binaries/queue-ffprobe-$TARGET" ] ||
  [ "$(cat "$stamp" 2>/dev/null)" != "$want" ]; then
  echo "==> Building ffmpeg for $TARGET (build script or manifest changed; several minutes)"
  rm -f "$stamp"
  scripts/build-ffmpeg.sh "$TARGET"
  echo "$want" >"$stamp"
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
