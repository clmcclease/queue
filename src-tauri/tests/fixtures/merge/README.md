# Merge fixtures (#70)

Tiny separate video and audio streams that `download_real_d2.rs` serves over loopback as a DASH manifest. This lets the real yt-dlp pick streams for each format preset and the bundled ffmpeg merge them, on every OS in CI, with no external network.

| File | Contents | Size | sha256 |
|---|---|---|---|
| `v_h264.mp4` | H.264 Baseline, 64×48, 10 frames at 10 fps, no audio | 1,630 B | `49b0dc581086eda88892ca5bb4e50f0ab1735f1657e5e200a7204943e68acca2` |
| `v_mjpeg.mp4` | MJPEG (`mp4v`), 128×96, 10 frames at 10 fps, no audio | 3,578 B | `f0e8ec5fb7967e0cfe609e70f4124323c6617d620a5deae7c5a342b4f4a5ddb2` |
| `a_aac.m4a` | AAC-LC, 8 kHz mono, 1 s of silence | 4,136 B | `5bfe3036ac26ada1eace4602d3a4cf969be1c0201a5e6794e3a662440564feef` |

The test makes the FLAC audio stream itself, from its WAV silence, with the bundled ffmpeg.

## Why they're committed

Our ffmpeg is remux-only (D-012), so it can't encode H.264 or AAC. These were made once on macOS with the OS's own encoders (`avconvert`, `afconvert`). Committing the bytes also keeps the tests from drifting with encoder versions. `.gitattributes` marks them binary, and the test checks each file's exact size before using it.

To rebuild them (macOS only), run this and then update this table and the sizes in `download_real_d2.rs`:

```sh
src-tauri/tests/fixtures/merge/make-fixtures.sh src-tauri/binaries/queue-ffmpeg-aarch64-apple-darwin
```

## Codec labels

yt-dlp doesn't probe DASH streams. It takes each codec from the manifest's `codecs` attribute, so the test's labels must match the real streams:
- `avc1.42c00a`: H.264 Baseline
- `mp4v.6C`: MJPEG in MP4 (RFC 6381)
- `mp4a.40.2`: AAC-LC
- `flac`

An unknown label such as `mjpeg` makes yt-dlp treat the stream as possibly carrying audio, and it then skips the merge.
