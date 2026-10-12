//! Runs one yt-dlp download (D2, #4). The name is yt-dlp's `%(title)s` for now; the naming engine
//! (#14) and collision handling (#16) replace it in M3. yt-dlp picks the format: no `-f` (D-007).
//!
//! Each download runs in its own fresh working folder, so yt-dlp's partial and intermediate files
//! never sit next to the user's files. The finished file is then moved into the output folder
//! with an operation the filesystem refuses if the name already exists, so nothing is ever
//! overwritten (including names the filesystem treats as equal: case or Unicode form).
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::sidecar::{self, Sidecar};

/// Where yt-dlp gets the video from: a fresh extraction of the URL, or the `-J` info saved
/// when the row was added (D-017; its media links last about 6 h and are tied to this IP).
#[derive(Clone, Copy, Debug)]
pub enum Source<'a> {
    Url(&'a str),
    Info(&'a Path),
}

/// The full yt-dlp argv after the program name. `url` is untrusted: it goes after `--`, so it can
/// never be read as an option.
pub fn args(sidecar_dir: &Path, work_dir: &Path, url: &str) -> Vec<OsString> {
    args_for(sidecar_dir, work_dir, Source::Url(url))
}

pub fn args_for(sidecar_dir: &Path, work_dir: &Path, source: Source) -> Vec<OsString> {
    let mut args = sidecar::ytdlp_base_args(sidecar_dir);
    args.extend(
        [
            "--no-playlist",
            "--no-overwrites",
            "--windows-filenames",
            // Leave room under the 255-byte name limit for yt-dlp's own suffixes (.part, .f137,
            // .temp, .ytdl) and our " (n)" (N2). The naming engine (#14) caps names lower.
            "--trim-filenames",
            "200",
            // Date files by when they were downloaded, not the upload date.
            "--no-mtime",
            "--no-progress",
        ]
        .map(OsString::from),
    );
    args.push("-P".into());
    args.push(work_dir.into());
    // yt-dlp records the final path in a UTF-8 file in the work dir (console encodings vary).
    args.extend(["--print-to-file", "after_move:filepath", REPORT].map(OsString::from));
    args.extend(["-o", "%(title)s.%(ext)s"].map(OsString::from));
    match source {
        Source::Url(url) => {
            args.push("--".into());
            args.push(url.into());
        }
        Source::Info(path) => {
            args.push("--load-info-json".into());
            args.push(path.into());
        }
    }
    args
}

/// The readable reason from yt-dlp's stderr: its last `ERROR:` line, else the last line.
pub fn failure_reason(stderr: &str, status: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let line = lines
        .iter()
        .rev()
        .find(|l| l.starts_with("ERROR:"))
        .or(lines.last())
        .copied();
    match line {
        Some(l) => l.trim_start_matches("ERROR:").trim().to_owned(),
        None => format!("yt-dlp exited with {status}"),
    }
}

/// Where yt-dlp records the finished file's path, relative to the work dir (`-P`). A plain name,
/// so it means nothing as an output template.
pub const REPORT: &str = "queue-result.txt";

/// yt-dlp's own in-progress files: never a finished download (left over by a failed attempt).
fn is_partial(name: &str) -> bool {
    name.ends_with(".part") || name.ends_with(".ytdl") || name.contains(".part-Frag")
}

/// The finished file yt-dlp reported in [`REPORT`], if it's a regular file inside `work_dir`.
fn reported_file(work_dir: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(work_dir.join(REPORT)).ok()?;
    let path = PathBuf::from(text.lines().map(str::trim).rfind(|l| !l.is_empty())?);
    let (dir, file) = (
        fs::canonicalize(work_dir).ok()?,
        fs::canonicalize(&path).ok()?,
    );
    let inside = file.parent() == Some(dir.as_path());
    let name_ok = file.file_name().is_some_and(|n| n != REPORT);
    (inside && name_ok && fs::symlink_metadata(&path).ok()?.is_file()).then_some(path)
}

/// The one finished file yt-dlp left in its working folder (it deletes intermediates after a
/// merge). Anything else means we can't tell which file is the download.
pub fn finished_file(work_dir: &Path) -> Result<PathBuf, String> {
    let unreadable = |e: io::Error| format!("Couldn't read the download's working folder: {e}");
    let mut files = Vec::new();
    for entry in fs::read_dir(work_dir).map_err(unreadable)? {
        let entry = entry.map_err(unreadable)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == REPORT || is_partial(&name) {
            continue;
        }
        if !entry.file_type().map_err(unreadable)?.is_file() {
            return Err(format!(
                "yt-dlp left something other than a file: {}",
                entry.file_name().to_string_lossy()
            ));
        }
        files.push(entry.path());
    }
    match files.len() {
        0 => Err("yt-dlp finished without saving a file".into()),
        1 => Ok(files.remove(0)),
        n => Err(format!("yt-dlp left {n} files; expected one")),
    }
}

/// Moves `src` to `dest`, failing with `AlreadyExists` instead of replacing an existing file.
/// A hard link is atomic and no-clobber; across volumes (or on FAT) it falls back to a copy into
/// a file that must not exist yet. `fs::rename` isn't used: it replaces existing files.
pub fn place(src: &Path, dest: &Path) -> io::Result<()> {
    match fs::hard_link(src, dest) {
        Ok(()) => fs::remove_file(src),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
        Err(_) => copy_no_clobber(src, dest),
    }
}

/// `place`'s fallback: copy into `dest`, which must not exist yet, then remove `src`. A failed
/// copy removes the partial `dest` and keeps `src`.
pub fn copy_no_clobber(src: &Path, dest: &Path) -> io::Result<()> {
    let mut to = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)?;
    let copied = fs::File::open(src)
        .and_then(|mut from| io::copy(&mut from, &mut to))
        .and_then(|_| to.sync_all());
    if let Err(e) = copied {
        drop(to);
        let _ = fs::remove_file(dest);
        return Err(e);
    }
    fs::remove_file(src)
}

/// Highest ` (n)` tried before giving up (N2).
pub const MAX_COPIES: u32 = 999;

/// `Title.webm` → `Title (2).webm` (N2: keep both, never overwrite). The extension is the part
/// after the last dot; a name without one (or a dotfile) just gets the suffix.
pub fn numbered(name: &OsStr, n: u32) -> OsString {
    let name = name.to_string_lossy();
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (&name[..], ""),
    };
    let suffix = format!(" ({n})");
    // A name already at the limit must still fit its suffix: shorten the stem (never the
    // extension) at a character boundary. UTF-8 bytes ≥ UTF-16 units, so this also fits
    // Windows' 255-unit limit.
    let budget = MAX_NAME_BYTES.saturating_sub(suffix.len() + ext.len());
    let mut cut = stem.len().min(budget);
    while !stem.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{suffix}{ext}", &stem[..cut]).into()
}

/// Longest file name (in UTF-8 bytes) most filesystems accept (APFS, ext4, NTFS in UTF-16 units).
pub const MAX_NAME_BYTES: usize = 255;

/// Places `src` in `out_dir` as `name`, or the first free `name (2)` … `name (999)` (N2). Each
/// candidate is claimed with the atomic no-clobber [`place`], so even two downloads finishing at
/// once can't overwrite each other or an existing file.
pub fn place_unique(src: &Path, out_dir: &Path, name: &OsStr) -> io::Result<PathBuf> {
    let mut candidate = name.to_owned();
    for n in 1..=MAX_COPIES {
        if n > 1 {
            candidate = numbered(name, n);
        }
        let dest = out_dir.join(&candidate);
        match place(src, &dest) {
            Ok(()) => return Ok(dest),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::from(io::ErrorKind::AlreadyExists))
}

/// Downloads `url` into the fresh `work_dir`, moves the result into `out_dir`, and returns its
/// path or a readable failure reason. `work_dir` must not exist yet; it's always removed after.
pub fn run(
    sidecar_dir: &Path,
    work_dir: &Path,
    out_dir: &Path,
    url: &str,
) -> Result<String, String> {
    run_source(sidecar_dir, work_dir, out_dir, Source::Url(url))
}

/// [`run`] for any [`Source`].
pub fn run_source(
    sidecar_dir: &Path,
    work_dir: &Path,
    out_dir: &Path,
    source: Source,
) -> Result<String, String> {
    if let Some(parent) = work_dir.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Couldn't create a working folder: {e}"))?;
    }
    fs::create_dir(work_dir).map_err(|e| format!("Couldn't create a working folder: {e}"))?;
    let result = run_in(sidecar_dir, work_dir, out_dir, source).map_err(|f| f.reason);
    let _ = fs::remove_dir_all(work_dir); // partial and intermediate files, success or not
    result
}

/// Why one attempt failed, and whether a retry can help: only a 403 reported by yt-dlp itself
/// (D-017), never one of Queue's own errors (e.g. a file named "HTTP Error 403.webm" existing).
struct Failure {
    reason: String,
    retryable: bool,
}

impl From<String> for Failure {
    fn from(reason: String) -> Self {
        Failure {
            reason,
            retryable: false,
        }
    }
}

fn run_in(
    sidecar_dir: &Path,
    work_dir: &Path,
    out_dir: &Path,
    source: Source,
) -> Result<String, Failure> {
    let _ = fs::remove_file(work_dir.join(REPORT)); // a previous attempt's report
    let mut cmd = Command::new(Sidecar::YtDlp.path_in(sidecar_dir));
    cmd.args(args_for(sidecar_dir, work_dir, source))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000; // no console window flashing up from the GUI app
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = cmd
        .output()
        .map_err(|e| format!("Couldn't start yt-dlp: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = failure_reason(&stderr, &output.status.to_string());
        let retryable = is_forbidden(&reason);
        return Err(Failure { reason, retryable });
    }
    let file = match reported_file(work_dir) {
        Some(file) => file,
        None => finished_file(work_dir)?, // no report (e.g. test fakes): the strict rule
    };
    let name = file.file_name().unwrap_or_default().to_owned();
    fs::create_dir_all(out_dir)
        .map_err(|e| format!("Couldn't create {}: {e}", out_dir.display()))?;
    let dest = place_unique(&file, out_dir, &name).map_err(|e| match e.kind() {
        io::ErrorKind::AlreadyExists => format!(
            "Not saved: \"{}\" and {MAX_COPIES} numbered copies already exist",
            name.to_string_lossy()
        ),
        _ => format!("Couldn't save to {}: {e}", out_dir.display()),
    })?;
    Ok(dest.to_string_lossy().into_owned())
}

/// Waits before retry 2 and retry 3 after a 403 (D-017).
pub const RETRY_WAITS: [Duration; 2] = [Duration::from_secs(10), Duration::from_secs(30)];

/// YouTube sometimes rejects a media link with 403; a fresh extraction usually works (D-017).
/// yt-dlp itself never retries 4xx errors.
pub fn is_forbidden(reason: &str) -> bool {
    reason.contains("HTTP Error 403")
}

/// Downloads with up to two retries after a 403. The first attempt uses the saved info when
/// given; retries always re-extract from the URL (the saved links are what failed). All attempts
/// share one fresh work dir, so streams that already finished (`Title.f137.mp4`) and `.part`
/// files are reused; the dir is private to this download and removed at the end (D-014).
/// `wait` sleeps (injected for tests) and `on_retry(n)` reports attempt `n` before it starts.
pub fn run_with_retries(
    sidecar_dir: &Path,
    work_dir: &Path,
    out_dir: &Path,
    url: &str,
    info: Option<&Path>,
    mut wait: impl FnMut(Duration),
    mut on_retry: impl FnMut(u32),
) -> Result<String, String> {
    if let Some(parent) = work_dir.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Couldn't create a working folder: {e}"))?;
    }
    fs::create_dir(work_dir).map_err(|e| format!("Couldn't create a working folder: {e}"))?;
    let attempts = RETRY_WAITS.len() as u32 + 1;
    let mut result = Err(Failure::from("not started".to_string()));
    for attempt in 1..=attempts {
        let source = match (attempt, info) {
            (1, Some(path)) => Source::Info(path),
            _ => Source::Url(url),
        };
        result = run_in(sidecar_dir, work_dir, out_dir, source);
        match &result {
            Err(f) if f.retryable && attempt < attempts => {
                on_retry(attempt + 1);
                wait(RETRY_WAITS[attempt as usize - 1]);
            }
            _ => break,
        }
    }
    let _ = fs::remove_dir_all(work_dir);
    result.map_err(|f| f.reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("queue-d2-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn untrusted_url_comes_last_after_double_dash() {
        let url = "--exec=rm -rf ~";
        let a = args(Path::new("bin"), Path::new("work dir"), url);
        let n = a.len();
        assert_eq!(a[n - 2], "--");
        assert_eq!(a[n - 1], url);
        assert_eq!(a.iter().filter(|x| *x == url).count(), 1);
    }

    #[test]
    fn writes_title_named_files_into_the_work_dir_without_forcing_a_format() {
        let a = args(Path::new("bin"), Path::new("work dir"), "https://e.test/v");
        let at = |flag: &str| a.iter().position(|x| x == flag).unwrap();
        assert_eq!(a[at("-P") + 1], OsString::from("work dir"));
        assert_eq!(a[at("-o") + 1], "%(title)s.%(ext)s");
        let at = |flag: &str| a.iter().position(|x| x == flag).unwrap();
        assert_eq!(
            a[at("--print-to-file") + 1..at("--print-to-file") + 3],
            [
                OsString::from("after_move:filepath"),
                OsString::from(REPORT)
            ]
        );
        assert_eq!(a[at("--trim-filenames") + 1], "200");
        for flag in [
            "--no-overwrites",
            "--windows-filenames",
            "--no-mtime",
            "--no-playlist",
            "--ignore-config",
        ] {
            assert!(a.iter().any(|x| x == flag), "{flag}");
        }
        assert!(
            !a.iter().any(|x| x == "-f" || x == "--format"),
            "D-007: yt-dlp picks the format"
        );
    }

    #[test]
    fn info_source_loads_the_saved_json_instead_of_a_url() {
        let a = args_for(
            Path::new("bin"),
            Path::new("w"),
            Source::Info(Path::new("i.json")),
        );
        let at = a.iter().position(|x| x == "--load-info-json").unwrap();
        assert_eq!(a[at + 1], OsString::from("i.json"));
        assert!(!a.iter().any(|x| x == "--"), "no URL argument at all");
        assert_eq!(a.last().unwrap(), "i.json");
    }

    #[test]
    fn only_403s_are_retried() {
        assert!(is_forbidden(
            "unable to download video data: HTTP Error 403: Forbidden"
        ));
        assert!(!is_forbidden("HTTP Error 404: Not Found"));
        assert!(!is_forbidden("Video unavailable"));
    }

    #[test]
    fn failure_reason_prefers_the_last_error_line() {
        let stderr = "WARNING: something\nERROR: [youtube] abc: Video unavailable\n\n";
        assert_eq!(
            failure_reason(stderr, "exit status: 1"),
            "[youtube] abc: Video unavailable"
        );
        assert_eq!(failure_reason("just noise\nlast line", "x"), "last line");
        assert_eq!(
            failure_reason("  \n", "exit status: 2"),
            "yt-dlp exited with exit status: 2"
        );
    }

    #[test]
    fn finished_file_needs_exactly_one_regular_file() {
        let dir = temp("finished");
        assert!(finished_file(&dir).unwrap_err().contains("without saving"));
        fs::write(dir.join("a.webm"), b"a").unwrap();
        assert_eq!(finished_file(&dir).unwrap(), dir.join("a.webm"));
        fs::write(dir.join("b.webm"), b"b").unwrap();
        assert!(finished_file(&dir).unwrap_err().contains("2 files"));
        fs::remove_file(dir.join("b.webm")).unwrap();
        fs::create_dir(dir.join("sub")).unwrap();
        assert!(
            finished_file(&dir)
                .unwrap_err()
                .contains("other than a file")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn copy_fallback_copies_refuses_existing_and_cleans_up_failures() {
        let dir = temp("copy");
        let (src, dest) = (dir.join("src.webm"), dir.join("dest.webm"));
        fs::write(&src, b"new").unwrap();
        copy_no_clobber(&src, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"new");
        assert!(!src.exists());
        fs::write(&src, b"newer").unwrap();
        let err = copy_no_clobber(&src, &dest).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&dest).unwrap(), b"new", "existing file untouched");
        assert!(src.exists());
        let missing = dir.join("missing.webm");
        let fresh = dir.join("fresh.webm");
        assert!(copy_no_clobber(&missing, &fresh).is_err());
        assert!(!fresh.exists(), "a failed copy leaves no partial file");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_reported_file_wins_over_leftovers_but_must_be_inside_the_work_dir() {
        let dir = temp("report");
        fs::write(dir.join("Title.f137.mp4"), b"stale stream").unwrap();
        fs::write(dir.join("Title.webm.part"), b"partial").unwrap();
        fs::write(dir.join("Title.mkv"), b"final").unwrap();
        assert!(finished_file(&dir).is_err(), "ambiguous without a report");
        fs::write(
            dir.join(REPORT),
            format!("{}\n", dir.join("Title.mkv").display()),
        )
        .unwrap();
        assert_eq!(reported_file(&dir), Some(dir.join("Title.mkv")));
        let outside = std::env::temp_dir().join(format!("queue-outside-{}", std::process::id()));
        fs::write(&outside, b"x").unwrap();
        fs::write(dir.join(REPORT), outside.display().to_string()).unwrap();
        assert_eq!(
            reported_file(&dir),
            None,
            "a path outside the work dir is ignored"
        );
        fs::write(dir.join(REPORT), dir.join(REPORT).display().to_string()).unwrap();
        assert_eq!(
            reported_file(&dir),
            None,
            "the report itself is never the result"
        );
        fs::remove_file(outside).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn partial_files_are_never_counted_as_the_download() {
        let dir = temp("partials");
        for leftover in ["a.webm.part", "a.webm.ytdl", "a.f251.webm.part-Frag3"] {
            fs::write(dir.join(leftover), b"x").unwrap();
        }
        fs::write(dir.join("a.webm"), b"done").unwrap();
        assert_eq!(finished_file(&dir).unwrap(), dir.join("a.webm"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn numbered_names_keep_the_extension() {
        let n = |name: &str, i| numbered(OsStr::new(name), i).into_string().unwrap();
        assert_eq!(n("Title.webm", 2), "Title (2).webm");
        assert_eq!(n("Me at the zoo.f137.mp4", 3), "Me at the zoo.f137 (3).mp4");
        assert_eq!(n("README", 2), "README (2)");
        assert_eq!(n(".hidden", 2), ".hidden (2)");
        assert_eq!(n("日本語 🎵.opus", 2), "日本語 🎵 (2).opus");
        let long = format!("{}.webm", "a".repeat(250));
        let fitted = n(&long, 2);
        assert_eq!(fitted.len(), MAX_NAME_BYTES);
        assert!(fitted.ends_with(" (2).webm"));
        let wide = format!("{}.webm", "日".repeat(83)); // 249 bytes + 5
        let fitted = n(&wide, 999);
        assert!(fitted.len() <= MAX_NAME_BYTES && fitted.ends_with(" (999).webm"));
        assert!(fitted.starts_with("日日"), "cut at a character boundary");
    }

    #[test]
    fn place_unique_keeps_both_and_never_touches_existing_files() {
        let dir = temp("unique");
        let out = dir.join("out");
        fs::create_dir_all(&out).unwrap();
        fs::write(out.join("Title.webm"), b"first").unwrap();
        fs::create_dir(out.join("Title (2).webm")).unwrap(); // even a folder in the way
        let src = dir.join("new.webm");
        fs::write(&src, b"second").unwrap();
        let dest = place_unique(&src, &out, OsStr::new("Title.webm")).unwrap();
        assert_eq!(dest, out.join("Title (3).webm"));
        assert_eq!(fs::read(&dest).unwrap(), b"second");
        assert_eq!(fs::read(out.join("Title.webm")).unwrap(), b"first");
        assert!(out.join("Title (2).webm").is_dir());
        let src = dir.join("free.webm");
        fs::write(&src, b"x").unwrap();
        assert_eq!(
            place_unique(&src, &out, OsStr::new("Free.webm")).unwrap(),
            out.join("Free.webm")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn place_moves_the_file_and_never_replaces_an_existing_one() {
        let dir = temp("place");
        let (src, dest) = (dir.join("src.webm"), dir.join("dest.webm"));
        fs::write(&src, b"new").unwrap();
        place(&src, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"new");
        assert!(!src.exists());
        fs::write(&src, b"newer").unwrap();
        let err = place(&src, &dest).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&dest).unwrap(), b"new", "existing file untouched");
        assert!(src.exists(), "the source is kept when placing fails");
        fs::remove_dir_all(dir).unwrap();
    }
}
