//! Runs one yt-dlp download (D2, #4). The name is yt-dlp's `%(title)s` for now; the naming engine
//! (#14) and collision handling (#16) replace it in M3. yt-dlp picks the format: no `-f` (D-007).
//!
//! Each download runs in its own fresh working folder, so yt-dlp's partial and intermediate files
//! never sit next to the user's files. The finished file is then moved into the output folder
//! with an operation the filesystem refuses if the name already exists, so nothing is ever
//! overwritten (including names the filesystem treats as equal: case or Unicode form).
use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use crate::proc;
use crate::sidecar::{self, Sidecar};

/// Marks yt-dlp's download progress lines (see [`PROGRESS_TEMPLATE`]).
pub const PROGRESS_PREFIX: &str = "QUEUE-PROGRESS ";
/// Marks yt-dlp's postprocessing lines (merge, audio extraction, fixups, the final move).
pub const POST_PREFIX: &str = "QUEUE-POST ";
pub const PROGRESS_TEMPLATE: &str = concat!(
    "download:QUEUE-PROGRESS {",
    r#""status":%(progress.status|null)j,"downloaded":%(progress.downloaded_bytes|null)j,"#,
    r#""total":%(progress.total_bytes|null)j,"total_estimate":%(progress.total_bytes_estimate|null)j,"#,
    r#""speed":%(progress.speed|null)j,"eta":%(progress.eta|null)j,"#,
    r#""fragment":%(progress.fragment_index|null)j,"fragments":%(progress.fragment_count|null)j}"#
);
pub const POST_TEMPLATE: &str = concat!(
    "postprocess:QUEUE-POST {",
    r#""status":%(progress.status|null)j,"postprocessor":%(progress.postprocessor|null)j}"#
);

/// Live download progress for the row (D4). Every field is optional: sites report what they can.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Progress {
    /// 0–100, from bytes when the total is known (or estimated), else from HLS/DASH fragments.
    pub percent: Option<f64>,
    pub downloaded: Option<u64>,
    pub total: Option<u64>,
    /// Bytes per second.
    pub speed: Option<f64>,
    /// Seconds left.
    pub eta: Option<u64>,
    /// Which stream is downloading: a merged video downloads video (part 1) then audio (part 2).
    pub part: u32,
}

/// What a download reports while it runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Progress(Progress),
    /// A postprocessor started (merging streams, extracting audio, fixups): the row shows
    /// "Processing". yt-dlp's final MoveFiles step isn't reported.
    Processing(String),
}

#[derive(serde::Deserialize)]
struct RawProgress {
    status: Option<String>,
    downloaded: Option<f64>,
    total: Option<f64>,
    total_estimate: Option<f64>,
    speed: Option<f64>,
    eta: Option<f64>,
    fragment: Option<f64>,
    fragments: Option<f64>,
}

#[derive(serde::Deserialize)]
struct RawPost {
    status: Option<String>,
    postprocessor: Option<String>,
}

/// A finite, non-negative number that fits in a u64, else none (yt-dlp values are untrusted).
fn count(v: Option<f64>) -> Option<u64> {
    v.filter(|v| v.is_finite() && *v >= 0.0 && *v < u64::MAX as f64)
        .map(|v| v as u64)
}

/// Parses one stdout line. `finished` counts finished streams so far, giving the `part`; it is
/// advanced here. Anything that isn't a well-formed progress line is ignored.
pub fn parse_line(line: &str, finished: &mut u32) -> Option<Event> {
    if let Some(json) = line.strip_prefix(PROGRESS_PREFIX) {
        let raw: RawProgress = serde_json::from_str(json.trim()).ok()?;
        let part = *finished + 1;
        if raw.status.as_deref() == Some("finished") {
            *finished += 1;
        }
        let downloaded = count(raw.downloaded);
        // A zero total means unknown (some sites send 0 alongside a real estimate).
        let total = count(raw.total)
            .filter(|t| *t > 0)
            .or(count(raw.total_estimate).filter(|t| *t > 0));
        let percent = match (downloaded, total, count(raw.fragment), count(raw.fragments)) {
            (Some(d), Some(t), _, _) => Some(d as f64 / t as f64 * 100.0),
            (_, _, Some(i), Some(n)) if n > 0 => Some(i as f64 / n as f64 * 100.0),
            _ => None,
        }
        .map(|p| p.clamp(0.0, 100.0));
        return Some(Event::Progress(Progress {
            percent,
            downloaded,
            total,
            speed: raw.speed.filter(|s| s.is_finite() && *s >= 0.0),
            eta: count(raw.eta),
            part,
        }));
    }
    if let Some(json) = line.strip_prefix(POST_PREFIX) {
        let raw: RawPost = serde_json::from_str(json.trim()).ok()?;
        let name = raw.postprocessor?;
        return (raw.status.as_deref() == Some("started") && name != "MoveFiles")
            .then_some(Event::Processing(name));
    }
    None
}

/// Longest stdout line kept; progress lines are ~250 bytes, anything far longer is skipped.
const MAX_LINE: usize = 64 * 1024;

/// How long the pipes may stay open after yt-dlp exits before its leftover descendants are
/// stopped.
const DRAIN_GRACE: Duration = Duration::from_secs(5);

/// Most events that may wait for the caller before the stdout reader waits for room.
const EVENT_BACKLOG: usize = 64;

/// Minimum gap between progress updates delivered for one download (a few a second is smooth;
/// yt-dlp can print hundreds).
pub const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// Holds back progress that comes within [`PROGRESS_EVERY`] of the last one delivered, except
/// the first of each part and its first 100%, which go at once. (Downloads that outgrow an
/// underestimated total report 100% repeatedly; those wait like any other.) A held update
/// stays in the [`Mailbox`], where newer ones replace it, and goes out once it's due, so the
/// newest value shows even if yt-dlp then goes quiet.
#[derive(Default)]
struct Throttle {
    last: Option<(Instant, u32)>,
    full: bool, // this part's 100% was delivered
}

impl Throttle {
    fn due(&self, p: &Progress, now: Instant) -> bool {
        match self.last {
            None => true,
            Some((at, part)) => {
                part != p.part
                    || (p.percent == Some(100.0) && !self.full)
                    || now.saturating_duration_since(at) >= PROGRESS_EVERY
            }
        }
    }

    fn delivered(&mut self, p: &Progress, now: Instant) {
        let same_part = self.last.is_some_and(|(_, part)| part == p.part);
        self.full = (same_part && self.full) || p.percent == Some(100.0);
        self.last = Some((now, p.part));
    }
}

/// Hands parsed events from the stdout reader to the caller. Progress that arrives while the
/// newest waiting event is progress for the same part replaces it (see [`supersedes`]), so a
/// flood stays one entry and the
/// newest value is ready the moment the caller looks. Other events (Processing) keep their
/// order and are never dropped; the reader waits when [`EVENT_BACKLOG`] are queued.
#[derive(Default)]
struct Mailbox {
    state: Mutex<MailboxState>,
    changed: Condvar,
}

#[derive(Default)]
struct MailboxState {
    events: VecDeque<Event>,
    closed: bool,  // the reader reached EOF
    hung_up: bool, // the caller stopped taking events
}

enum Take {
    Event(Event),
    Empty,
    NotYet, // the oldest event isn't `ready` yet
    Closed,
}

impl Mailbox {
    fn lock(&self) -> std::sync::MutexGuard<'_, MailboxState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn put(&self, event: Event) {
        let mut state = self.lock();
        if let (Event::Progress(new), Some(Event::Progress(last))) =
            (&event, state.events.back_mut())
            && supersedes(new, last)
        {
            *last = new.clone();
        } else {
            while state.events.len() >= EVENT_BACKLOG && !state.hung_up {
                state = self
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            if state.hung_up {
                return;
            }
            state.events.push_back(event);
        }
        self.changed.notify_all();
    }

    fn close(&self) {
        self.lock().closed = true;
        self.changed.notify_all();
    }

    /// The oldest waiting event if `ready` accepts it, waiting up to `timeout` for one.
    fn take(&self, timeout: Duration, ready: impl Fn(&Event) -> bool) -> Take {
        let state = self.lock();
        let (mut state, _) = self
            .changed
            .wait_timeout_while(state, timeout, |s| s.events.is_empty() && !s.closed)
            .unwrap_or_else(PoisonError::into_inner);
        if state.events.front().is_some_and(|e| !ready(e)) {
            return Take::NotYet;
        }
        match state.events.pop_front() {
            Some(event) => {
                self.changed.notify_all(); // room for a waiting reader
                Take::Event(event)
            }
            None if state.closed => Take::Closed,
            None => Take::Empty,
        }
    }
}

/// Whether `new` may replace a waiting `old`: same part, and a part's 100% (which the UI
/// always shows) only gives way to another 100%.
fn supersedes(new: &Progress, old: &Progress) -> bool {
    let full = |p: &Progress| p.percent == Some(100.0);
    new.part == old.part && (!full(old) || full(new))
}

/// Marks the [`Mailbox`] abandoned when the caller is done, so a waiting reader returns.
struct HangUp(Arc<Mailbox>);

impl Drop for HangUp {
    fn drop(&mut self) {
        self.0.lock().hung_up = true;
        self.0.changed.notify_all();
    }
}

/// Reads `pipe` line by line (bounded), calling `on_line` for each complete line.
fn for_each_line(pipe: impl io::Read, mut on_line: impl FnMut(&str)) {
    use io::BufRead;
    let mut reader = io::BufReader::new(pipe);
    let mut line = Vec::new();
    let mut too_long = false;
    loop {
        let buf = match reader.fill_buf() {
            Ok([]) | Err(_) => break,
            Ok(buf) => buf,
        };
        let (chunk, end) = match buf.iter().position(|b| *b == b'\n') {
            Some(i) => (&buf[..i], Some(i + 1)),
            None => (buf, None),
        };
        if !too_long && line.len() + chunk.len() <= MAX_LINE {
            line.extend_from_slice(chunk);
        } else {
            too_long = true;
            line.clear();
        }
        let used = end.unwrap_or(buf.len());
        reader.consume(used);
        if end.is_some() {
            if !too_long {
                on_line(String::from_utf8_lossy(&line).trim_end_matches('\r'));
            }
            line.clear();
            too_long = false;
        }
    }
}

/// Where yt-dlp gets the video from: a fresh extraction of the URL, or the `-J` info saved
/// when the row was added (D-017; its media links last about 6 h and are tied to this IP).
/// What to download (D2, #11, D-015). Only changes yt-dlp's format flags; nothing is re-encoded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Format {
    /// yt-dlp picks the best format and container itself (D-007): no `-f`, nothing forced.
    #[default]
    Default,
    /// Prefers H.264 + AAC in `.mp4`, which plays almost everywhere. Strict H.264 (D-015): it
    /// wins even at a lower resolution. Without it, falls back to an honest `.webm`/`.mkv`.
    CompatibleVideo,
    /// Audio only: the best native audio, extracted without re-encoding (`.opus`, `.m4a`, ...).
    Audio,
    /// Audio only, preferring AAC in `.m4a`; otherwise the best native audio.
    CompatibleAudio,
}

impl Format {
    /// yt-dlp flags for this format. Never `-t mp4`, `--remux-video`, a forced
    /// `--merge-output-format mp4` or `--audio-format`: remuxing VP9 into MP4 or converting audio
    /// needs codecs our minimal ffmpeg doesn't have, and the result is silently broken (D-015).
    pub fn args(self) -> &'static [&'static str] {
        match self {
            Format::Default => &[],
            Format::CompatibleVideo => &[
                "-S",
                "vcodec:h264,vext:mp4,lang,quality,res,fps,hdr:12,acodec:aac,aext:m4a",
                "--merge-output-format",
                "mp4/webm/mkv",
            ],
            Format::Audio => &["-x"],
            Format::CompatibleAudio => &["-f", "ba/b", "-S", "acodec:aac,aext:m4a,vext:mp4", "-x"],
        }
    }

    /// For the "most compatible" formats, a notice when the saved file isn't the compatible
    /// container after all (the site had no H.264 or AAC). D-015 keeps the honest extension
    /// rather than re-encoding. Worded for every OS: no player names.
    pub fn notice(self, saved: &Path) -> Option<String> {
        let ext = saved
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let (expected, missing): (&[&str], &str) = match self {
            Format::CompatibleVideo => (&["mp4", "m4v"], "No H.264 version was available"),
            Format::CompatibleAudio => (&["m4a", "aac", "mp3"], "No AAC version was available"),
            Format::Default | Format::Audio => return None,
        };
        if expected.contains(&ext.as_str()) {
            return None;
        }
        let shown = if ext.is_empty() {
            "a different format".to_string()
        } else {
            format!(".{ext}")
        };
        Some(format!(
            "{missing}, so it's saved as {shown}. Some players can't open it."
        ))
    }

    /// Like [`Format::notice`], but decided from the codecs yt-dlp actually chose when they're
    /// known: an `.mp4` can hold AV1, and H.264 can come in another container. Falls back to the
    /// extension when yt-dlp didn't report a codec.
    pub fn notice_for(self, saved: &Path, codecs: &Codecs) -> Option<String> {
        let ext = saved
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let shown = if ext.is_empty() {
            "a different format".to_string()
        } else {
            format!(".{ext}")
        };
        let video = codecs.video.as_deref().map(codec_name);
        let audio = codecs.audio.as_deref().map(codec_name);
        match self {
            Format::Default | Format::Audio => None,
            Format::CompatibleVideo => {
                // Each codec yt-dlp did report is judged on its own: a known problem always
                // gets its notice, even when the other codec is unknown.
                if let Some(video) = video.filter(|v| *v != "H.264") {
                    let what = if video == "no" {
                        "This site had no video".to_string()
                    } else {
                        format!("No H.264 version was available, so it has {video} video")
                    };
                    return Some(format!(
                        "{what}; it's saved as {shown}. Some players can't open it."
                    ));
                }
                if let Some(audio) = audio.filter(|a| !matches!(*a, "AAC" | "no")) {
                    return Some(format!(
                        "No AAC audio was available, so it has {audio} audio; it's saved as {shown}. Some players can't open it."
                    ));
                }
                match (video, audio) {
                    // Both known and fine: only the container can still be wrong.
                    (Some(_), Some(_)) => (!matches!(ext.as_str(), "mp4" | "m4v")).then(|| {
                        format!(
                            "It's H.264 but saved as {shown}. Some players can't open that container."
                        )
                    }),
                    // Nothing known is wrong, but something is unknown: the extension decides.
                    _ => self.notice(saved),
                }
            }
            Format::CompatibleAudio => {
                let Some(audio) = audio else {
                    return self.notice(saved);
                };
                (!matches!(audio, "AAC" | "MP3")).then(|| {
                    format!(
                        "No AAC version was available, so it's {audio} audio saved as {shown}. Some players can't open it."
                    )
                })
            }
        }
    }
}

/// The codecs yt-dlp chose, as it reported them; `None` where it didn't know.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Codecs {
    pub video: Option<String>,
    pub audio: Option<String>,
}

impl Codecs {
    /// Parses yt-dlp's `vcodec|acodec` line ("NA" or empty means unknown).
    pub fn parse(line: &str) -> Self {
        let known = |c: &str| {
            let c = c.trim();
            (!c.is_empty() && c != "NA").then(|| c.to_string())
        };
        let (video, audio) = line.split_once('|').unwrap_or((line, ""));
        Codecs {
            video: known(video),
            audio: known(audio),
        }
    }
}

/// A readable codec name for notices: "avc1.640028" is H.264, "av01.0.08M.08" is AV1.
fn codec_name(codec: &str) -> &'static str {
    let c = codec.to_ascii_lowercase();
    let starts = |prefixes: &[&str]| prefixes.iter().any(|p| c.starts_with(p));
    if c == "none" {
        "no"
    } else if starts(&["avc", "h264"]) {
        "H.264"
    } else if starts(&["hev", "hvc", "h265"]) {
        "HEVC"
    } else if starts(&["av01", "av1"]) {
        "AV1"
    } else if starts(&["vp09", "vp9"]) {
        "VP9"
    } else if starts(&["vp8"]) {
        "VP8"
    } else if c == "mp4a.40.34" || starts(&["mp3"]) {
        "MP3"
    } else if starts(&["mp4a", "aac"]) {
        "AAC"
    } else if starts(&["opus"]) {
        "Opus"
    } else if starts(&["vorbis"]) {
        "Vorbis"
    } else if starts(&["flac"]) {
        "FLAC"
    } else if starts(&["pcm"]) {
        "PCM"
    } else {
        "another"
    }
}

/// A finished download: where it was saved, and the codecs yt-dlp chose (#11).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    pub path: String,
    pub codecs: Codecs,
}

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

/// [`args_for_format`] with [`Format::Default`].
pub fn args_for(sidecar_dir: &Path, work_dir: &Path, source: Source) -> Vec<OsString> {
    args_for_format(sidecar_dir, work_dir, source, Format::Default)
}

/// The full yt-dlp argv for `source` downloaded as `format`.
pub fn args_for_format(
    sidecar_dir: &Path,
    work_dir: &Path,
    source: Source,
    format: Format,
) -> Vec<OsString> {
    let mut args = sidecar::ytdlp_base_args(sidecar_dir);
    args.extend(format.args().iter().map(OsString::from));
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
            // Live progress (D4, #8): one JSON line per update, prefixed so it can't be mixed up
            // with any other output. `|null` keeps missing values valid JSON.
            "--progress",
            "--newline",
            "--progress-template",
            PROGRESS_TEMPLATE,
            "--progress-template",
            POST_TEMPLATE,
        ]
        .map(OsString::from),
    );
    args.push("-P".into());
    args.push(work_dir.into());
    // yt-dlp records the final path in a UTF-8 file in the work dir (console encodings vary).
    args.extend(["--print-to-file", "after_move:filepath", REPORT].map(OsString::from));
    // The codecs it chose, so a "most compatible" notice can be exact (#11).
    args.extend(
        [
            "--print-to-file",
            "after_move:%(vcodec)s|%(acodec)s",
            CODECS,
        ]
        .map(OsString::from),
    );
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
    // An `ERROR:` with nothing after it says nothing: skip it, and fall back to the exit status.
    let body = |l: &&str| l.trim_start_matches("ERROR:").trim().to_owned();
    let line = lines
        .iter()
        .rev()
        .find(|l| l.starts_with("ERROR:") && !body(l).is_empty())
        .or(lines.last())
        .map(body)
        .filter(|l| !l.is_empty());
    match line {
        Some(l) => readable(&l).to_owned(),
        None => format!("yt-dlp exited with {status}"),
    }
}

/// Drops yt-dlp's `[extractor] video-id: ` lead-in (Q6), so the row says "Video unavailable"
/// rather than "[youtube] dQw4w9WgXcQ: Video unavailable". The row already shows the link.
fn readable(line: &str) -> &str {
    let Some(rest) = line
        .strip_prefix('[')
        .and_then(|l| l.split_once("] "))
        .map(|(_, rest)| rest)
    else {
        return line;
    };
    let rest = match rest.split_once(' ') {
        Some((id, tail)) if id.len() > 1 && id.ends_with(':') => tail.trim_start(),
        None if rest.ends_with(':') => "", // just an id: nothing readable is left
        _ => rest,
    };
    if rest.is_empty() { line } else { rest }
}

/// Where yt-dlp records the finished file's path, relative to the work dir (`-P`). A plain name,
/// so it means nothing as an output template.
pub const REPORT: &str = "queue-result.txt";

/// Where yt-dlp records the chosen `vcodec|acodec` (#11), next to [`REPORT`].
pub const CODECS: &str = "queue-codecs.txt";

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
    let name_ok = file.file_name().is_some_and(|n| n != REPORT && n != CODECS);
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
        if name == REPORT || name == CODECS || is_partial(&name) {
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
    let result = run_in(
        sidecar_dir,
        work_dir,
        out_dir,
        source,
        Format::Default,
        &mut |_| {},
    )
    .map(|saved| saved.path)
    .map_err(|f| f.reason);
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
    format: Format,
    on_event: &mut dyn FnMut(Event),
) -> Result<Saved, Failure> {
    let _ = fs::remove_file(work_dir.join(REPORT)); // a previous attempt's report
    let _ = fs::remove_file(work_dir.join(CODECS));
    let mut cmd = Command::new(Sidecar::YtDlp.path_in(sidecar_dir));
    cmd.args(args_for_format(sidecar_dir, work_dir, source, format))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut tree = proc::spawn_tree(&mut cmd).map_err(|e| format!("Couldn't start yt-dlp: {e}"))?;
    let stdout = tree.child.stdout.take().expect("piped");
    let stderr = tree.child.stderr.take().expect("piped");
    // Both pipes are read on their own threads (stderr as a bounded tail, for the failure
    // reason) so neither can fill up and block yt-dlp. Events come back through a [`Mailbox`]
    // and `on_event` runs here, while this thread also watches for yt-dlp to exit.
    let mailbox = Arc::new(Mailbox::default());
    let _hang_up = HangUp(mailbox.clone()); // however this returns, the reader can't block
    let sender = mailbox.clone();
    let (err_tx, err) = mpsc::channel();
    let readers = std::thread::Builder::new()
        .name("yt-dlp stdout".into())
        .spawn(move || {
            let mut finished = 0;
            for_each_line(stdout, |line| {
                if let Some(event) = parse_line(line, &mut finished) {
                    sender.put(event);
                }
            });
            sender.close();
        })
        .and_then(|_| {
            std::thread::Builder::new()
                .name("yt-dlp stderr".into())
                .spawn(move || {
                    let _ = err_tx.send(proc::read_tail(stderr, 64 * 1024).unwrap_or_default());
                })
        });
    if let Err(e) = readers {
        tree.terminate();
        return Err(format!("Couldn't read yt-dlp's output: {e}").into());
    }
    // A descendant (ffmpeg, the onefile child) can outlive yt-dlp and keep a pipe open. Once
    // yt-dlp has exited, the pipes get DRAIN_GRACE to close; then the rest of the tree is
    // stopped, so a stuck child can never hold the download slot.
    let mut status = None;
    let mut drain_until = None;
    let mut throttle = Throttle::default(); // per attempt: a retry's first update goes at once
    loop {
        let ready = |e: &Event| match e {
            Event::Progress(p) => throttle.due(p, Instant::now()),
            Event::Processing(_) => true,
        };
        match mailbox.take(Duration::from_millis(25), ready) {
            Take::Event(Event::Progress(p)) => {
                throttle.delivered(&p, Instant::now());
                on_event(Event::Progress(p));
            }
            Take::Event(event) => on_event(event),
            Take::Closed => break, // stdout closed
            Take::Empty => {}
            Take::NotYet => std::thread::sleep(Duration::from_millis(25)),
        }
        if status.is_none() {
            match tree.child.try_wait() {
                Ok(Some(s)) => {
                    status = Some(s);
                    drain_until = Some(Instant::now() + DRAIN_GRACE);
                }
                Ok(None) => {}
                Err(e) => {
                    tree.terminate();
                    return Err(format!("Couldn't wait for yt-dlp: {e}").into());
                }
            }
        }
        if drain_until.is_some_and(|t| Instant::now() >= t) {
            tree.terminate(); // a descendant still holds stdout open
            break;
        }
    }
    let status = match status {
        Some(status) => status,
        None => tree
            .child
            .wait()
            .map_err(|e| format!("Couldn't wait for yt-dlp: {e}"))?,
    };
    let drain_until = drain_until.unwrap_or_else(|| Instant::now() + DRAIN_GRACE);
    let stderr = match err.recv_timeout(drain_until.saturating_duration_since(Instant::now())) {
        Ok(stderr) => stderr,
        Err(_) => {
            tree.terminate(); // a descendant still holds stderr open
            err.recv_timeout(Duration::from_secs(1)).unwrap_or_default()
        }
    };
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr);
        let reason = failure_reason(&stderr, &status.to_string());
        let retryable = is_forbidden(&reason);
        return Err(Failure { reason, retryable });
    }
    let file = match reported_file(work_dir) {
        Some(file) => file,
        None => finished_file(work_dir)?, // no report (e.g. test fakes): the strict rule
    };
    let name = file.file_name().unwrap_or_default().to_owned();
    // Never created here: a chosen folder that vanished (an unplugged drive) must not come
    // back as an empty local folder (D-020). The caller creates the default Downloads folder.
    if !out_dir.is_dir() {
        return Err(crate::settings::unavailable(out_dir).into());
    }
    let dest = place_unique(&file, out_dir, &name).map_err(|e| match e.kind() {
        io::ErrorKind::AlreadyExists => format!(
            "Not saved: \"{}\" and {MAX_COPIES} numbered copies already exist",
            name.to_string_lossy()
        ),
        _ => format!("Couldn't save to {}: {e}", out_dir.display()),
    })?;
    let codecs = fs::read_to_string(work_dir.join(CODECS))
        .ok()
        .and_then(|t| {
            t.lines()
                .map(str::trim)
                .rfind(|l| !l.is_empty())
                .map(Codecs::parse)
        })
        .unwrap_or_default();
    Ok(Saved {
        path: dest.to_string_lossy().into_owned(),
        codecs,
    })
}

/// Leftovers in the app cache from earlier runs (#45): download work dirs (`downloads/item-*`,
/// D-014) and saved `-J` sessions (`info/session-*`, D-017). A run removes its own when it ends,
/// but a quit or crash mid-download leaves them, and a restored queue (#12) never reuses them.
/// Only real folders with those names, inside real `downloads`/`info` folders, are listed:
/// never files, and never symlinks at either level (they could point anywhere). Call it before this run creates any of its own.
pub fn stale_work_dirs(cache_dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for (sub, prefix) in [("downloads", "item-"), ("info", "session-")] {
        let container = cache_dir.join(sub);
        // The container itself must be a real folder too: a `downloads` that's a symlink would
        // lead the cleanup into someone else's folder.
        if !fs::symlink_metadata(&container).is_ok_and(|m| m.file_type().is_dir()) {
            continue;
        }
        let Ok(entries) = fs::read_dir(&container) else {
            continue;
        };
        for entry in entries.flatten() {
            // `file_type` doesn't follow symlinks: a link to a folder isn't a folder here.
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            let named = entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.starts_with(prefix));
            if is_dir && named {
                found.push(entry.path());
            }
        }
    }
    found.sort();
    found
}

/// Deletes one folder found by [`stale_work_dirs`], after checking again, right before deleting,
/// that it and its `downloads`/`info` parent are still real folders (not symlinks): something
/// could have swapped them since they were listed. A process that can already write to the cache
/// could still race the instant between this check and the delete; that sub-millisecond window
/// is accepted (#45, D-014).
pub fn remove_stale(dir: &Path) -> io::Result<()> {
    let real_dir = |p: &Path| fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_dir());
    let parent = dir.parent().unwrap_or(dir);
    if !real_dir(parent) || !real_dir(dir) {
        return Err(io::Error::other(format!(
            "{} isn't a real folder anymore; not deleting it",
            dir.display()
        )));
    }
    fs::remove_dir_all(dir)
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
    wait: impl FnMut(Duration),
    on_retry: impl FnMut(u32),
) -> Result<String, String> {
    run_with_retries_events(
        sidecar_dir,
        work_dir,
        out_dir,
        url,
        info,
        wait,
        on_retry,
        |_| {},
    )
}

/// [`run_with_retries`], also streaming progress and processing [`Event`]s.
#[allow(clippy::too_many_arguments)]
pub fn run_with_retries_events(
    sidecar_dir: &Path,
    work_dir: &Path,
    out_dir: &Path,
    url: &str,
    info: Option<&Path>,
    wait: impl FnMut(Duration),
    on_retry: impl FnMut(u32),
    on_event: impl FnMut(Event),
) -> Result<String, String> {
    run_with_retries_format(
        sidecar_dir,
        work_dir,
        out_dir,
        url,
        info,
        Format::Default,
        wait,
        on_retry,
        on_event,
    )
}

/// [`run_with_retries_events`] downloading as `format` (#11).
#[allow(clippy::too_many_arguments)]
pub fn run_with_retries_format(
    sidecar_dir: &Path,
    work_dir: &Path,
    out_dir: &Path,
    url: &str,
    info: Option<&Path>,
    format: Format,
    wait: impl FnMut(Duration),
    on_retry: impl FnMut(u32),
    on_event: impl FnMut(Event),
) -> Result<String, String> {
    run_with_retries_saved(
        sidecar_dir,
        work_dir,
        out_dir,
        url,
        info,
        format,
        wait,
        on_retry,
        on_event,
    )
    .map(|saved| saved.path)
}

/// [`run_with_retries_format`], also returning the codecs yt-dlp chose (for the notice).
#[allow(clippy::too_many_arguments)]
pub fn run_with_retries_saved(
    sidecar_dir: &Path,
    work_dir: &Path,
    out_dir: &Path,
    url: &str,
    info: Option<&Path>,
    format: Format,
    mut wait: impl FnMut(Duration),
    mut on_retry: impl FnMut(u32),
    mut on_event: impl FnMut(Event),
) -> Result<Saved, String> {
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
        result = run_in(
            sidecar_dir,
            work_dir,
            out_dir,
            source,
            format,
            &mut on_event,
        );
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

    #[test]
    fn only_earlier_runs_staging_and_info_folders_are_stale() {
        let cache = std::env::temp_dir().join(format!("queue-45-stale-{}", std::process::id()));
        let _ = fs::remove_dir_all(&cache);
        let downloads = cache.join("downloads");
        let info = cache.join("info");
        fs::create_dir_all(downloads.join("item-3-1760000000000000000").join("sub")).unwrap();
        fs::write(
            downloads.join("item-3-1760000000000000000/Video.f137.mp4.part"),
            b"partial",
        )
        .unwrap();
        fs::create_dir_all(downloads.join("item-日本語")).unwrap();
        fs::write(downloads.join("item-file-not-dir"), b"x").unwrap();
        fs::create_dir_all(downloads.join("keep-me")).unwrap();
        fs::create_dir_all(info.join("session-123-456-0")).unwrap();
        fs::write(info.join("session-123-456-0/item-1.json"), b"{}").unwrap();
        fs::create_dir_all(info.join("other")).unwrap();
        let outside = cache.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("precious.txt"), b"keep").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, downloads.join("item-link")).unwrap();

        let stale = stale_work_dirs(&cache);
        assert_eq!(
            stale,
            vec![
                downloads.join("item-3-1760000000000000000"),
                downloads.join("item-日本語"),
                info.join("session-123-456-0"),
            ]
        );
        for dir in stale {
            fs::remove_dir_all(dir).unwrap();
        }
        assert!(downloads.join("item-file-not-dir").is_file());
        assert!(downloads.join("keep-me").is_dir());
        assert!(info.join("other").is_dir());
        assert_eq!(
            fs::read(outside.join("precious.txt")).unwrap(),
            b"keep",
            "a symlink is never followed"
        );
        assert!(stale_work_dirs(&cache.join("missing")).is_empty());
        fs::remove_dir_all(cache).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_container_swapped_for_a_symlink_after_listing_is_not_followed() {
        let root = std::env::temp_dir().join(format!("queue-45-swap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let cache = root.join("cache");
        fs::create_dir_all(cache.join("downloads/item-1")).unwrap();
        let elsewhere = root.join("elsewhere");
        fs::create_dir_all(elsewhere.join("item-1")).unwrap();
        fs::write(elsewhere.join("item-1/precious.txt"), b"keep").unwrap();
        let stale = stale_work_dirs(&cache);
        assert_eq!(stale, vec![cache.join("downloads/item-1")]);
        // Between listing and deleting, `downloads` is replaced by a link to another folder.
        fs::rename(cache.join("downloads"), root.join("moved")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, cache.join("downloads")).unwrap();
        assert!(remove_stale(&stale[0]).is_err(), "refused");
        assert_eq!(
            fs::read(elsewhere.join("item-1/precious.txt")).unwrap(),
            b"keep"
        );
        // And a real one is still deleted.
        fs::remove_file(cache.join("downloads")).unwrap();
        fs::rename(root.join("moved"), cache.join("downloads")).unwrap();
        remove_stale(&stale[0]).unwrap();
        assert!(!stale[0].exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn formats_add_only_their_own_flags_and_never_remux_or_convert() {
        let base = args_for(
            Path::new("/s"),
            Path::new("/w"),
            Source::Url("https://e.test/v"),
        );
        for (format, flags) in [
            (Format::Default, vec![]),
            (
                Format::CompatibleVideo,
                vec![
                    "-S",
                    "vcodec:h264,vext:mp4,lang,quality,res,fps,hdr:12,acodec:aac,aext:m4a",
                    "--merge-output-format",
                    "mp4/webm/mkv",
                ],
            ),
            (Format::Audio, vec!["-x"]),
            (
                Format::CompatibleAudio,
                vec!["-f", "ba/b", "-S", "acodec:aac,aext:m4a,vext:mp4", "-x"],
            ),
        ] {
            let args: Vec<String> = args_for_format(
                Path::new("/s"),
                Path::new("/w"),
                Source::Url("https://e.test/v"),
                format,
            )
            .into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
            let extra: Vec<&str> = args
                .iter()
                .filter(|a| !base.iter().any(|b| b.to_string_lossy() == a.as_str()))
                .map(String::as_str)
                .collect();
            assert_eq!(extra, flags, "{format:?}");
            assert_eq!(args.len(), base.len() + flags.len());
            assert!(
                args.ends_with(&["--".into(), "https://e.test/v".into()]),
                "URL stays last, after --"
            );
            for banned in ["-t", "--remux-video", "--recode-video", "--audio-format"] {
                assert!(
                    !args.iter().any(|a| a == banned),
                    "{format:?} must not use {banned} (D-015)"
                );
            }
            assert!(
                !args
                    .windows(2)
                    .any(|w| w[0] == "--merge-output-format" && w[1] == "mp4")
            );
        }
    }

    #[test]
    fn only_the_compatible_formats_flag_an_incompatible_file() {
        let notice = |f: Format, name: &str| f.notice(Path::new(name));
        assert_eq!(notice(Format::CompatibleVideo, "/d/a.mp4"), None);
        assert_eq!(notice(Format::CompatibleVideo, "/d/a (2).MP4"), None);
        assert_eq!(
            notice(Format::CompatibleVideo, "/d/日本語.webm").as_deref(),
            Some(
                "No H.264 version was available, so it's saved as .webm. Some players can't open it."
            )
        );
        assert!(
            notice(Format::CompatibleVideo, "/d/a.mkv")
                .unwrap()
                .contains(".mkv")
        );
        assert_eq!(notice(Format::CompatibleAudio, "/d/a.m4a"), None);
        assert_eq!(
            notice(Format::CompatibleAudio, "/d/a.opus").as_deref(),
            Some(
                "No AAC version was available, so it's saved as .opus. Some players can't open it."
            )
        );
        assert!(
            notice(Format::CompatibleAudio, "/d/noext")
                .unwrap()
                .contains("a different format")
        );
        for f in [Format::Default, Format::Audio] {
            assert_eq!(
                notice(f, "/d/a.webm"),
                None,
                "{f:?} never claims compatibility"
            );
        }
        assert!(
            !format!("{:?}", notice(Format::CompatibleVideo, "/d/a.webm")).contains("QuickTime")
        );
    }

    #[test]
    fn the_notice_is_decided_by_the_codecs_yt_dlp_chose() {
        let c = |v: &str, a: &str| Codecs::parse(&format!("{v}|{a}"));
        let video = |path: &str, codecs: Codecs| {
            Format::CompatibleVideo.notice_for(Path::new(path), &codecs)
        };
        assert_eq!(
            video("/d/a.mp4", c("avc1.640028", "mp4a.40.2")),
            None,
            "H.264 + AAC in MP4"
        );
        let av1 = video("/d/a.mp4", c("av01.0.08M.08", "mp4a.40.2"))
            .expect("AV1 in an .mp4 still needs a notice");
        assert!(
            av1.contains("No H.264") && av1.contains("AV1") && av1.contains(".mp4"),
            "{av1}"
        );
        let vp9 = video("/d/a.webm", c("vp09.00.40.08", "opus")).unwrap();
        assert!(vp9.contains("VP9") && vp9.contains(".webm"), "{vp9}");
        let opus = video("/d/a.mp4", c("avc1.4d401e", "opus")).unwrap();
        assert!(
            opus.contains("No AAC audio") && opus.contains("Opus"),
            "{opus}"
        );
        let mkv = video("/d/a.mkv", c("avc1.4d401e", "mp4a.40.2")).unwrap();
        assert!(mkv.contains("H.264 but saved as .mkv"), "{mkv}");
        assert_eq!(
            video("/d/a.mp4", c("avc1", "none")),
            None,
            "H.264 with no audio track is fine"
        );
        assert!(
            video("/d/a.m4a", c("none", "mp4a.40.2"))
                .unwrap()
                .starts_with("This site had no video")
        );
        // One codec unknown: the known one still decides when it's a problem (review round 2).
        assert!(
            video("/d/a.mp4", c("av01.0.08M.08", "NA"))
                .unwrap()
                .contains("AV1")
        );
        assert!(video("/d/a.mp4", c("NA", "opus")).unwrap().contains("Opus"));
        assert_eq!(
            video("/d/a.mp4", c("avc1.4d401e", "NA")),
            None,
            "known-good + unknown: .mp4 decides"
        );
        assert!(
            video("/d/a.webm", c("avc1.4d401e", "NA"))
                .unwrap()
                .contains(".webm")
        );
        // Unknown codecs: the extension decides, as before.
        assert_eq!(video("/d/a.mp4", c("NA", "NA")), None);
        assert!(
            video("/d/a.webm", Codecs::default())
                .unwrap()
                .contains(".webm")
        );

        let audio = |path: &str, codecs: Codecs| {
            Format::CompatibleAudio.notice_for(Path::new(path), &codecs)
        };
        assert_eq!(audio("/d/a.m4a", c("none", "mp4a.40.2")), None);
        assert_eq!(audio("/d/a.mp3", c("none", "mp3")), None);
        let opus = audio("/d/a.opus", c("none", "opus")).unwrap();
        assert!(opus.contains("Opus audio saved as .opus"), "{opus}");
        assert!(
            audio("/d/a.opus", c("none", "NA")).is_some(),
            "unknown audio: the extension decides"
        );
        for f in [Format::Default, Format::Audio] {
            assert_eq!(
                f.notice_for(Path::new("/d/a.webm"), &c("vp9", "opus")),
                None
            );
        }
        for codecs in [c("av01", "opus"), c("vp9", "vorbis"), c("hev1", "flac")] {
            let n = video("/d/a.mkv", codecs).unwrap();
            assert!(
                !n.contains("QuickTime") && !n.contains("Finder"),
                "OS-neutral: {n}"
            );
        }
    }

    #[test]
    fn codecs_parse_yt_dlp_lines_and_treat_na_as_unknown() {
        assert_eq!(
            Codecs::parse("avc1.640028|mp4a.40.2"),
            Codecs {
                video: Some("avc1.640028".into()),
                audio: Some("mp4a.40.2".into())
            }
        );
        assert_eq!(
            Codecs::parse("NA|opus"),
            Codecs {
                video: None,
                audio: Some("opus".into())
            }
        );
        assert_eq!(
            Codecs::parse(" none | NA "),
            Codecs {
                video: Some("none".into()),
                audio: None
            }
        );
        assert_eq!(Codecs::parse(""), Codecs::default());
        assert_eq!(codec_name("mp4a.40.34"), "MP3");
        assert_eq!(codec_name("AVC1.4D401E"), "H.264");
        assert_eq!(codec_name("something-new"), "another");
    }

    #[test]
    fn formats_serialize_as_camel_case_for_the_ui_and_settings() {
        for (f, name) in [
            (Format::Default, "default"),
            (Format::CompatibleVideo, "compatibleVideo"),
            (Format::Audio, "audio"),
            (Format::CompatibleAudio, "compatibleAudio"),
        ] {
            assert_eq!(serde_json::to_value(f).unwrap(), name);
            assert_eq!(serde_json::from_value::<Format>(name.into()).unwrap(), f);
        }
        assert!(serde_json::from_value::<Format>("mp4".into()).is_err());
    }

    #[test]
    fn a_full_mailbox_keeps_order_and_a_hang_up_releases_a_blocked_reader() {
        let mailbox = Arc::new(Mailbox::default());
        let reader = {
            let mailbox = mailbox.clone();
            std::thread::spawn(move || {
                for n in 0..=EVENT_BACKLOG {
                    mailbox.put(Event::Processing(n.to_string())); // never merged
                }
            })
        };
        let taken: Vec<_> = (0..3)
            .map(|_| match mailbox.take(Duration::from_secs(5), |_| true) {
                Take::Event(Event::Processing(n)) => n,
                _ => panic!("expected an event"),
            })
            .collect();
        assert_eq!(taken, ["0", "1", "2"]);
        reader.join().unwrap(); // the 65th fit once one was taken
        let blocked = {
            let mailbox = mailbox.clone();
            std::thread::spawn(move || {
                for n in 0..10 {
                    mailbox.put(Event::Processing(format!("more {n}")));
                }
            })
        };
        drop(HangUp(mailbox));
        blocked.join().unwrap(); // returns instead of waiting forever
    }

    #[test]
    fn progress_is_throttled_within_a_part_but_not_across_parts_or_at_100() {
        let p = |percent: f64, part: u32| Progress {
            percent: Some(percent),
            downloaded: None,
            total: None,
            speed: None,
            eta: None,
            part,
        };
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut t = Throttle::default();
        let mut t = |p: &Progress, now| {
            let due = t.due(p, now);
            if due {
                t.delivered(p, now);
            }
            due
        };
        assert!(t(&p(1.0, 1), ms(0)), "first update");
        assert!(!t(&p(2.0, 1), ms(100)), "too soon");
        assert!(!t(&p(3.0, 1), ms(249)), "still too soon");
        assert!(t(&p(4.0, 1), ms(250)), "gap elapsed");
        assert!(t(&p(100.0, 1), ms(260)), "the first 100% always");
        assert!(!t(&p(100.0, 1), ms(262)), "a repeated 100% is throttled");
        assert!(!t(&p(100.0, 1), ms(265)));
        assert!(t(&p(0.5, 2), ms(270)), "new part always");
        assert!(!t(&p(1.0, 2), ms(300)));
        assert!(t(&p(2.0, 2), ms(520)));
        assert!(t(&p(100.0, 2), ms(530)), "each part's first 100%");
    }

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
        assert!(a.iter().any(|x| x == "--newline") && !a.iter().any(|x| x == "--no-progress"));
        assert!(a.iter().any(|x| x == PROGRESS_TEMPLATE) && a.iter().any(|x| x == POST_TEMPLATE));
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
    fn progress_lines_parse_with_bytes_estimates_fragments_and_parts() {
        let mut finished = 0;
        let line = |s: &str| format!("{PROGRESS_PREFIX}{s}");
        let ev = parse_line(
            &line(
                r#"{"status":"downloading","downloaded":50,"total":200,"total_estimate":null,"speed":1000.5,"eta":3,"fragment":null,"fragments":null}"#,
            ),
            &mut finished,
        );
        assert_eq!(
            ev,
            Some(Event::Progress(Progress {
                percent: Some(25.0),
                downloaded: Some(50),
                total: Some(200),
                speed: Some(1000.5),
                eta: Some(3),
                part: 1
            }))
        );
        let est = parse_line(
            &line(
                r#"{"status":"downloading","downloaded":10,"total":null,"total_estimate":40,"speed":null,"eta":null,"fragment":null,"fragments":null}"#,
            ),
            &mut finished,
        );
        assert!(
            matches!(est, Some(Event::Progress(Progress { percent: Some(p), .. })) if p == 25.0)
        );
        let frags = parse_line(
            &line(
                r#"{"status":"downloading","downloaded":null,"total":null,"total_estimate":null,"speed":null,"eta":null,"fragment":3,"fragments":12}"#,
            ),
            &mut finished,
        );
        assert!(
            matches!(frags, Some(Event::Progress(Progress { percent: Some(p), .. })) if p == 25.0)
        );
        parse_line(
            &line(r#"{"status":"finished","downloaded":200,"total":200}"#),
            &mut finished,
        );
        assert_eq!(finished, 1);
        let second = parse_line(
            &line(r#"{"status":"downloading","downloaded":1,"total":2}"#),
            &mut finished,
        );
        assert!(
            matches!(second, Some(Event::Progress(Progress { part: 2, .. }))),
            "audio is part 2"
        );
    }

    #[test]
    fn bad_progress_values_are_dropped_and_other_lines_ignored() {
        let mut finished = 0;
        let ev = parse_line(
            &format!(
                r#"{PROGRESS_PREFIX}{{"status":"downloading","downloaded":-5,"total":0,"speed":-1,"eta":1e308}}"#
            ),
            &mut finished,
        );
        assert_eq!(
            ev,
            Some(Event::Progress(Progress {
                part: 1,
                ..Progress::default()
            }))
        );
        let over = parse_line(
            &format!(r#"{PROGRESS_PREFIX}{{"downloaded":300,"total":200}}"#),
            &mut finished,
        );
        assert!(
            matches!(over, Some(Event::Progress(Progress { percent: Some(p), .. })) if p == 100.0),
            "clamped"
        );
        for junk in [
            "[download] 10%",
            "QUEUE-PROGRESS not json",
            "QUEUE-PROGRESS {\"downloaded\":",
            "",
        ] {
            assert_eq!(parse_line(junk, &mut finished), None, "{junk:?}");
        }
    }

    #[test]
    fn postprocessing_starts_processing_except_the_final_move() {
        let mut f = 0;
        let post = |s: &str| format!("{POST_PREFIX}{s}");
        assert_eq!(
            parse_line(
                &post(r#"{"status":"started","postprocessor":"Merger"}"#),
                &mut f
            ),
            Some(Event::Processing("Merger".into()))
        );
        assert_eq!(
            parse_line(
                &post(r#"{"status":"finished","postprocessor":"Merger"}"#),
                &mut f
            ),
            None
        );
        assert_eq!(
            parse_line(
                &post(r#"{"status":"started","postprocessor":"MoveFiles"}"#),
                &mut f
            ),
            None
        );
        assert_eq!(
            parse_line(
                &post(r#"{"status":"started","postprocessor":null}"#),
                &mut f
            ),
            None
        );
    }

    #[test]
    fn lines_are_split_bounded_and_crlf_tolerant() {
        let long = "x".repeat(MAX_LINE + 10);
        let input = format!("one\r\n{long}\ntwo\nno-newline-at-end");
        let mut lines = Vec::new();
        for_each_line(input.as_bytes(), |l| lines.push(l.to_owned()));
        assert_eq!(
            lines,
            ["one", "two"],
            "over-long line skipped; unterminated tail dropped"
        );
    }

    #[test]
    fn failure_reason_prefers_the_last_error_line() {
        let stderr = "WARNING: something\nERROR: [youtube] abc: Video unavailable\n\n";
        assert_eq!(
            failure_reason(stderr, "exit status: 1"),
            "Video unavailable"
        );
        assert_eq!(failure_reason("just noise\nlast line", "x"), "last line");
        assert_eq!(
            failure_reason("  \n", "exit status: 2"),
            "yt-dlp exited with exit status: 2"
        );
    }

    #[test]
    fn failure_reasons_drop_the_extractor_and_id_lead_in() {
        for (line, want) in [
            (
                "ERROR: [youtube] dQw4w9WgXcQ: Video unavailable",
                "Video unavailable",
            ),
            (
                "ERROR: [youtube] abc: Private video. Sign in if you've been granted access",
                "Private video. Sign in if you've been granted access",
            ),
            (
                "ERROR: [generic] Unsupported URL: https://e.test/x",
                "Unsupported URL: https://e.test/x",
            ),
            (
                "ERROR: [download] Got error: HTTP Error 403: Forbidden",
                "Got error: HTTP Error 403: Forbidden",
            ),
            ("ERROR: [youtube] abc:", "[youtube] abc:"), // nothing left: keep it all
            ("ERROR: [broken", "[broken"),
            ("ERROR: no prefix: here", "no prefix: here"),
            (
                "ERROR: [x] 日本語: 動画は利用できません",
                "動画は利用できません",
            ),
            ("WARNING: cleanup\nERROR:   \n", "yt-dlp exited with x"),
            ("ERROR: real reason\nERROR:\n", "real reason"),
        ] {
            assert_eq!(failure_reason(line, "x"), want, "{line}");
        }
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
