#!/usr/bin/env bash
# Rebuilds the committed merge fixtures (#70); see README.md. macOS only: the H.264 and AAC
# encoders are the OS's own (avconvert, afconvert), since our minimal ffmpeg has none (D-012).
#
#   src-tauri/tests/fixtures/merge/make-fixtures.sh src-tauri/binaries/queue-ffmpeg-aarch64-apple-darwin
#
# Writes v_h264.mp4, v_mjpeg.mp4 and a_aac.m4a next to this script and prints their sha256.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
ffmpeg="${1:?usage: make-fixtures.sh <path to the bundled queue-ffmpeg>}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# Two flat-colour PNGs and one second of 8 kHz mono silence, from Python's standard library
# only (run isolated, -I).
python3 -I - "$tmp" <<'PY'
import struct, sys, zlib
out = sys.argv[1]
def png(path, w, h, rgb):
    raw = b"".join(b"\0" + bytes(rgb) * w for _ in range(h))
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
                + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))
png(f"{out}/small.png", 64, 48, (40, 120, 200))
png(f"{out}/large.png", 128, 96, (200, 80, 40))
with open(f"{out}/silence.wav", "wb") as f:
    f.write(b"RIFF" + struct.pack("<I", 36 + 16000) + b"WAVEfmt " + struct.pack("<IHHIIHH", 16, 1, 1, 8000, 16000, 2, 16)
            + b"data" + struct.pack("<I", 16000) + b"\0" * 16000)
PY

mjpeg() { "$ffmpeg" -hide_banner -loglevel error -y -framerate 10 -loop 1 -i "$1" -frames:v 10 -c:v mjpeg -pix_fmt yuvj420p "${@:2}"; }
# H.264: avconvert only decodes MJPEG tagged `jpeg` in .mov; remux without its silent track.
mjpeg "$tmp/small.png" -tag:v jpeg "$tmp/small.mov"
avconvert -s "$tmp/small.mov" -p PresetLowQuality -o "$tmp/h264.mp4" >/dev/null
"$ffmpeg" -hide_banner -loglevel error -y -i "$tmp/h264.mp4" -map 0:v:0 -c copy -movflags +faststart "$here/v_h264.mp4"
# A larger, higher-bitrate MJPEG stream, so "best" and "most compatible" pick different videos.
mjpeg "$tmp/large.png" -movflags +faststart "$here/v_mjpeg.mp4"
afconvert -f m4af -d aac "$tmp/silence.wav" "$here/a_aac.m4a"

cd "$here" && shasum -a 256 v_h264.mp4 v_mjpeg.mp4 a_aac.m4a
