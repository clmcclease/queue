# Third-party software

Queue is licensed under MIT OR Apache-2.0 (see `LICENSE-MIT` and `LICENSE-APACHE`). Its installers
also ship these separate programs, unmodified, as Tauri sidecars that Queue runs as subprocesses
(D-012). Each keeps its own license; as separate programs, they don't change Queue's. Exact versions and checksums are pinned in
[`scripts/sidecars.json`](scripts/sidecars.json).

| Program | Version | License | Source |
|---|---|---|---|
| [yt-dlp](https://github.com/yt-dlp/yt-dlp) | 2026.08.19 | Source: [Unlicense](https://github.com/yt-dlp/yt-dlp/blob/master/LICENSE) (public domain). The official PyInstaller builds we ship include GPLv3+ components, so **the binary as distributed is GPLv3+** | Official release binaries, SHA-256 checked; source: <https://github.com/yt-dlp/yt-dlp/tree/2026.08.19> |
| [deno](https://github.com/denoland/deno) | v2.9.7 | [MIT](https://github.com/denoland/deno/blob/main/LICENSE.md) | Official release binaries, SHA-256 checked; source: <https://github.com/denoland/deno/tree/v2.9.7> |
| [FFmpeg](https://ffmpeg.org) (`ffmpeg`, `ffprobe`) | 9.0.2 | [LGPL-2.1-or-later](https://ffmpeg.org/legal.html) | Built by Queue from the official source |

## FFmpeg

Queue's `ffmpeg` and `ffprobe` are built from the unmodified official FFmpeg 9.0.2 source,
<https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz>
(SHA-256 `8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e`).
They're built by [`scripts/build-ffmpeg.sh`](scripts/build-ffmpeg.sh), which lists every configure
flag. The build is LGPL-only: no `--enable-gpl`, no `--enable-nonfree`, and networking is off.
FFmpeg is licensed under the GNU Lesser General Public License version 2.1 or later. To rebuild
the exact binaries, run `scripts/build-ffmpeg.sh <target-triple>` against that tarball, or
replace them with your own build of the same name. FFmpeg is a trademark of Fabrice Bellard. Release installers will
include the LGPL text and this notice (#24).

## Libraries compiled into Queue

Queue itself is built on [Tauri](https://tauri.app) (MIT or Apache-2.0), React (MIT) and other
Rust crates and npm packages, listed with exact versions in `src-tauri/Cargo.lock` and
`package-lock.json`. Their licenses are permissive (MIT, Apache-2.0, BSD, ISC, Zlib, Unicode). A
generated list with full license texts will ship with release installers (#24).
