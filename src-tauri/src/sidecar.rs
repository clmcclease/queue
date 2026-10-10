//! Bundled sidecars (#2, D-012). Tauri copies `bundle > externalBin` next to the app binary
//! (`target/<profile>/` in dev; `Contents/MacOS`, the install dir, or `/usr/bin` when bundled)
//! and strips the target-triple suffix. The `queue-` prefix keeps the Linux packages from
//! colliding with distro `yt-dlp`/`ffmpeg` in `/usr/bin`.
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sidecar {
    YtDlp,
    Ffmpeg,
    Deno,
}

impl Sidecar {
    pub fn file_name(self) -> String {
        let stem = match self {
            Sidecar::YtDlp => "queue-yt-dlp",
            Sidecar::Ffmpeg => "queue-ffmpeg",
            Sidecar::Deno => "queue-deno",
        };
        format!("{stem}{}", std::env::consts::EXE_SUFFIX)
    }

    pub fn path_in(self, dir: &Path) -> PathBuf {
        dir.join(self.file_name())
    }
}

/// The directory holding the sidecars: the one containing the running app binary.
pub fn dir() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "app binary has no parent dir"))
}

/// Arguments for every yt-dlp run: use only the bundled ffmpeg (yt-dlp derives `queue-ffprobe`
/// from the `queue-ffmpeg` file name) and deno, ignore user config files, and never self-update
/// (updates are #21).
pub fn ytdlp_base_args(dir: &Path) -> Vec<OsString> {
    let mut deno = OsString::from("deno:");
    deno.push(Sidecar::Deno.path_in(dir));
    vec![
        "--ignore-config".into(),
        "--no-update".into(),
        "--ffmpeg-location".into(),
        Sidecar::Ffmpeg.path_in(dir).into(),
        "--no-js-runtimes".into(),
        "--js-runtimes".into(),
        deno,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_prefixed_and_platform_suffixed() {
        let exe = std::env::consts::EXE_SUFFIX;
        assert_eq!(Sidecar::YtDlp.file_name(), format!("queue-yt-dlp{exe}"));
        assert_eq!(Sidecar::Ffmpeg.file_name(), format!("queue-ffmpeg{exe}"));
        assert_eq!(Sidecar::Deno.file_name(), format!("queue-deno{exe}"));
    }

    #[test]
    fn base_args_point_at_bundled_tools_only() {
        let dir = Path::new("some dir").join("with spaces");
        let args = ytdlp_base_args(&dir);
        let ffmpeg = args.iter().position(|a| a == "--ffmpeg-location").unwrap();
        assert_eq!(
            args[ffmpeg + 1],
            OsString::from(dir.join(Sidecar::Ffmpeg.file_name()))
        );
        let runtimes = args.iter().position(|a| a == "--js-runtimes").unwrap();
        assert_eq!(args[runtimes - 1], "--no-js-runtimes");
        let mut deno = OsString::from("deno:");
        deno.push(dir.join(Sidecar::Deno.file_name()));
        assert_eq!(args[runtimes + 1], deno);
        assert!(args.iter().any(|a| a == "--ignore-config"));
        assert!(args.iter().any(|a| a == "--no-update"));
    }
}
