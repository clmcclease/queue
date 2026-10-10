# Third-party notices

Queue bundles these programs as separate executables (sidecars). Each program is unmodified and runs as its own process. Versions and checksums are pinned in [`scripts/sidecars.json`](scripts/sidecars.json) (D-012).

| Program | Version | License | Source |
|---|---|---|---|
| yt-dlp | 2026.08.19 | The Unlicense. The official PyInstaller builds we ship include GPLv3+ components, so the binary as distributed is GPLv3+ | https://github.com/yt-dlp/yt-dlp/tree/2026.08.19 |
| FFmpeg (`ffmpeg`, `ffprobe`) | 9.0.2 | LGPL-2.1-or-later; no GPL or nonfree components are enabled | https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz (SHA-256 `8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e`), built unmodified with the configure flags in [`scripts/build-ffmpeg.sh`](scripts/build-ffmpeg.sh) |
| Deno | 2.9.7 | MIT | https://github.com/denoland/deno/tree/v2.9.7 |

FFmpeg is a trademark of Fabrice Bellard. To rebuild the exact FFmpeg binaries, run `scripts/build-ffmpeg.sh <target-triple>` against the tarball above, or replace them with your own build of the same name.
