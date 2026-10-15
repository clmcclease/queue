#!/usr/bin/env bash
# Build minimal LGPL ffmpeg + ffprobe from the signed upstream source, unmodified (#2, D-012).
#
#   scripts/build-ffmpeg.sh <target-triple>
#
# Targets: aarch64-apple-darwin and x86_64-apple-darwin (on a macOS host), x86_64-unknown-linux-gnu
# (on Linux), x86_64-pc-windows-msvc (cross-compiled on Linux with mingw-w64: apt install
# mingw-w64 libz-mingw-w64-dev). Output: src-tauri/binaries/queue-{ffmpeg,ffprobe}-<triple>[.exe].
# No network protocols, no GPL components; only what yt-dlp needs to merge streams, extract audio
# without re-encoding, and embed thumbnails/metadata/subtitles.
set -euo pipefail

target="${1:?usage: build-ffmpeg.sh <target-triple>}"
# Allowlist first: $target becomes part of a path that gets rm -rf'd below.
case "$target" in
  aarch64-apple-darwin|x86_64-apple-darwin|x86_64-unknown-linux-gnu|x86_64-pc-windows-msvc) ;;
  *) echo "unsupported target: $target" >&2; exit 2 ;;
esac
root="$(cd "$(dirname "$0")/.." && pwd)"
manifest="$root/scripts/sidecars.json"
# Pass the path as an argument, never inside the JS source: checkout paths may contain quotes.
field() { node -e 'console.log(require(process.argv[1]).ffmpeg[process.argv[2]])' "$manifest" "$1"; }
version="$(field version)"
url="$(field url)"
sha="$(field sha256)"
bin="$root/src-tauri/binaries"
cache="$bin/.cache"
tarball="$cache/ffmpeg-$version.tar.xz"
work="$cache/ffmpeg-build-$target"
mkdir -p "$cache"

sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }

if [[ ! -f "$tarball" || "$(sha256 "$tarball")" != "$sha" ]]; then
  echo "downloading $url"
  curl -fsSL --retry 3 -o "$tarball.part" "$url"
  mv "$tarball.part" "$tarball"
fi
actual="$(sha256 "$tarball")"
[[ "$actual" == "$sha" ]] || { echo "ffmpeg tarball sha256 mismatch: got $actual, want $sha" >&2; exit 1; }

rm -rf "$work" && mkdir -p "$work"
tar -xJf "$tarball" -C "$work" --strip-components=1

flags=(
  --disable-everything --disable-autodetect --disable-network --disable-doc --disable-ffplay
  --disable-debug --enable-small --enable-zlib --disable-x86asm  # remux only; no need for nasm
  --enable-protocol=file,pipe
  --enable-demuxer=mov,matroska,mp3,aac,ogg,flac,wav,mpegts,image2,image_png_pipe,image_jpeg_pipe,image_webp_pipe,concat,ffmetadata,webvtt,srt,ass,avi
  --enable-muxer=mp4,ipod,mov,matroska,matroska_audio,webm,ogg,opus,mp3,adts,flac,wav,image2,ffmetadata,webvtt,srt,ass
  --enable-parser=aac,h264,hevc,av1,vp8,vp9,opus,vorbis,mpegaudio,flac,mjpeg,png,webp
  --enable-bsf=aac_adtstoasc,h264_mp4toannexb,hevc_mp4toannexb,vp9_superframe,extract_extradata
  --enable-decoder=mjpeg,png,webp,aac,mp3,opus,vorbis,flac,pcm_s16le,webvtt,srt,subrip,ass,ssa,mov_text,vp9
  --enable-encoder=mjpeg,png,flac,pcm_s16le,webvtt,srt,subrip,ass,ssa,mov_text
  --enable-filter=scale,format,aformat,aresample,null,anull,crop
)
ext=""
case "$target" in
  aarch64-apple-darwin|x86_64-apple-darwin)
    arch="${target%%-*}"; [[ "$arch" == aarch64 ]] && arch=arm64
    export MACOSX_DEPLOYMENT_TARGET=10.15
    flags+=(--arch="$arch" --enable-cross-compile --target-os=darwin --cc="clang -arch $arch"
            --extra-cflags="-mmacosx-version-min=10.15" --extra-ldflags="-mmacosx-version-min=10.15") ;;
  x86_64-unknown-linux-gnu)
    flags+=(--extra-ldflags="-static-libgcc") ;;
  x86_64-pc-windows-msvc)
    ext=".exe"
    flags+=(--arch=x86_64 --target-os=mingw32 --cross-prefix=x86_64-w64-mingw32- --enable-cross-compile
            --extra-ldflags="-static") ;;
esac

cd "$work"
./configure "${flags[@]}" >configure.out || { tail -30 configure.out ffbuild/config.log >&2; exit 1; }
grep -q '^License: LGPL' configure.out || { echo "configure did not produce an LGPL build" >&2; grep License configure.out >&2; exit 1; }
jobs="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)"
make -j"$jobs" >make.out 2>&1 || { tail -40 make.out >&2; exit 1; }

for prog in ffmpeg ffprobe; do
  cp "$prog$ext" "$bin/queue-$prog-$target$ext"
  chmod +x "$bin/queue-$prog-$target$ext"
done
echo "built ffmpeg $version ($(grep '^License' configure.out)) for $target:"
ls -l "$bin"/queue-ff*-"$target"*
