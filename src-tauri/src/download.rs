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
use std::sync::atomic::{AtomicU32, Ordering};
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
/// A hard link is atomic and no-clobber. Across volumes (or on FAT, which has no hard links) the
/// file is copied to a temporary name next to `dest` and then renamed into place without
/// replacing anything, so a crash or a quit mid-copy never leaves a cut-off file under the real
/// name (#72). `fs::rename` isn't used to publish: it replaces existing files.
pub fn place(src: &Path, dest: &Path) -> io::Result<()> {
    place_via(src, dest, &STEPS)
}

/// `place`'s fallback on its own: copy, publish as `dest` (which must not exist yet), then remove
/// `src`. A failed copy leaves nothing behind and keeps `src`.
pub fn copy_no_clobber(src: &Path, dest: &Path) -> io::Result<()> {
    copy_via(src, dest, &STEPS)
}

/// The filesystem steps of [`place`], separate so tests can make any one of them fail (#42).
#[derive(Clone, Copy)]
struct Steps {
    link: fn(&Path, &Path) -> io::Result<()>,
    copy: fn(&mut fs::File, &mut fs::File) -> io::Result<u64>,
    sync: fn(&fs::File) -> io::Result<()>,
    /// Renames without ever replacing the destination ([`rename_no_replace`]).
    publish: fn(&Path, &Path) -> io::Result<()>,
    /// Runs once the fallback's claim on a name is made and recorded (tests race it here).
    claimed: fn(&Path),
}

const STEPS: Steps = Steps {
    link: |src, dest| fs::hard_link(src, dest),
    copy: copy_unless_quitting,
    sync: fs::File::sync_all,
    publish: rename_no_replace,
    claimed: |_| {},
};

fn place_via(src: &Path, dest: &Path, steps: &Steps) -> io::Result<()> {
    place_first(src, [dest.to_path_buf()], steps).map(|_| ())
}

fn copy_via(src: &Path, dest: &Path, steps: &Steps) -> io::Result<()> {
    let no_link = Steps {
        link: |_, _| Err(io::ErrorKind::Unsupported.into()),
        ..*steps
    };
    place_via(src, dest, &no_link)
}

/// Places `src` under the first free name in `candidates`, which are all in one folder. Each is
/// tried as a hard link until a link fails for a reason other than the name being taken; then
/// `src` is copied once ([`Staged`]) and published under the first name still free. Every claim
/// is atomic and no-clobber, so two downloads finishing at once can't overwrite each other.
fn place_first(
    src: &Path,
    candidates: impl IntoIterator<Item = PathBuf>,
    steps: &Steps,
) -> io::Result<PathBuf> {
    let mut staged: Option<Staged> = None;
    for dest in candidates {
        if staged.is_none() {
            match (steps.link)(src, &dest) {
                Ok(()) => return fs::remove_file(src).map(|()| dest),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(_) => {}
            }
            // Don't copy just to find the name taken; publishing still checks atomically.
            if fs::symlink_metadata(&dest).is_ok() {
                continue;
            }
            let dir = dest.parent().unwrap_or(Path::new("."));
            staged = Some(Staged::copy(src, dir, steps)?);
        }
        let Some(copy) = staged.as_mut() else {
            unreachable!("staged above")
        };
        match copy.publish(&dest, steps) {
            Ok(()) => return fs::remove_file(src).map(|()| dest),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::ErrorKind::AlreadyExists.into())
}

/// Start of a temporary copy's name in the download folder (#72).
const PARTIAL: &str = ".queue-partial-";
/// Start of the note, in the download's work folder, that records a temporary copy.
const PARTIAL_NOTE: &str = "queue-partial-";

/// A copy under a temporary name in the destination folder (#72). A note next to the source (in
/// the download's work folder, in the app cache) records it first, so if Queue crashes or a quit
/// cuts the copy off, the next launch removes it ([`remove_stale`]). Dropped, it removes the
/// copy if it's still there, and the note.
struct Staged {
    temp: PathBuf,
    note: PathBuf,
}

impl Staged {
    /// Copies `src` into a new temporary file in `dir`, synced to disk. Copies in 1 MiB chunks,
    /// stopping if Queue starts quitting (#67).
    fn copy(src: &Path, dir: &Path, steps: &Steps) -> io::Result<Self> {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let id = format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let temp = dir.join(format!("{PARTIAL}{id}"));
        let note = src.with_file_name(format!("{PARTIAL_NOTE}{id}.txt"));
        fs::write(&note, partial_note(&temp, None))?;
        let mut to = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(to) => to,
            Err(e) => {
                let _ = fs::remove_file(&note);
                return Err(e);
            }
        };
        let staged = Staged { temp, note };
        fs::File::open(src)
            .and_then(|mut from| (steps.copy)(&mut from, &mut to))
            .and_then(|_| (steps.sync)(&to))?;
        Ok(staged)
    }

    /// Renames the copy to `dest` without replacing anything (`AlreadyExists` if it's taken).
    fn publish(&mut self, dest: &Path, steps: &Steps) -> io::Result<()> {
        match (steps.publish)(&self.temp, dest) {
            Err(e) if unsupported(&e) => self.claim_and_rename(dest, steps),
            result => result,
        }
    }

    /// For a filesystem without a no-replace rename: claim `dest` with an empty file
    /// (`create_new` is atomic and never replaces), then rename the finished copy over that
    /// claim. Only the empty claim can be left behind. It's recorded in the note only once it's
    /// ours, so the next launch never removes a file someone else made; a crash in the instant
    /// between creating and recording it leaves our own empty file instead.
    ///
    /// Right before the rename, the claim must still be our own empty file (same file ID): if
    /// another program replaced it, it's left alone and the next name is tried. A replacement in
    /// the microseconds between that check and the rename would still be overwritten; the owner
    /// accepted that remaining risk (#72 review, D-014). Windows has no stable file ID in Rust,
    /// so there only emptiness is checked, but it never needs this fallback.
    fn claim_and_rename(&mut self, dest: &Path, steps: &Steps) -> io::Result<()> {
        let claim = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dest)?;
        let ours = file_id(&claim.metadata()?);
        drop(claim);
        let still_ours = || {
            fs::symlink_metadata(dest)
                .is_ok_and(|m| m.is_file() && m.len() == 0 && file_id(&m) == ours)
        };
        // Each note update replaces the note atomically: a crash mid-update keeps the old one, so
        // the temporary copy is never left without a record.
        let forget_claim = |note: &Path| {
            let _ = crate::settings::write_atomic(note, &partial_note(&self.temp, None));
        };
        let unclaim = |note: &Path| {
            if still_ours() {
                let _ = fs::remove_file(dest);
            }
            forget_claim(note);
        };
        // Without a file ID (Windows, which never needs this fallback) the claim isn't recorded:
        // a launch couldn't tell it from someone else's file, so a crash leaves it in place.
        let recorded = match ours {
            Some(id) => crate::settings::write_atomic(
                &self.note,
                &partial_note(&self.temp, Some((dest, id))),
            ),
            None => Ok(()),
        };
        if let Err(e) = recorded {
            unclaim(&self.note);
            return Err(e);
        }
        (steps.claimed)(dest);
        if !still_ours() {
            forget_claim(&self.note);
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        fs::rename(&self.temp, dest).inspect_err(|_| unclaim(&self.note))
    }
}

/// What identifies a file on its volume (device and inode), where Rust can read it: not on
/// Windows.
pub fn file_id(meta: &fs::Metadata) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        // After a publish the temporary name is gone and this finds nothing.
        let _ = fs::remove_file(&self.temp);
        let _ = fs::remove_file(&self.note);
    }
}

/// The text of a note recording a temporary copy and, once Queue has made it, its claim on the
/// final name (#72): one line each, as hex of the path's exact bytes (UTF-16 units on Windows),
/// so any folder or file name survives, newlines and non-UTF-8 names included.
///
/// The claim's line is followed by its file ID ([`file_id`]): a launch removes the claim only if
/// it's still that very file, never one another program put there while Queue was closed.
pub fn partial_note(temp: &Path, claim: Option<(&Path, (u64, u64))>) -> Vec<u8> {
    let mut text = to_hex(temp);
    if let Some((claim, (dev, ino))) = claim {
        text.push_str(&format!("\n{}\n{dev} {ino}", to_hex(claim)));
    }
    text.into_bytes()
}

fn to_hex(path: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str()
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .map(|u| format!("{u:04x}"))
            .collect()
    }
}

/// The path in one line of a note, or `None` if the line isn't one [`to_hex`] wrote.
fn from_hex(line: &str) -> Option<PathBuf> {
    let units = |width: usize| -> Option<Vec<u16>> {
        if line.is_empty() || !line.is_ascii() || !line.len().is_multiple_of(width) {
            return None;
        }
        (0..line.len())
            .step_by(width)
            .map(|i| u16::from_str_radix(&line[i..i + width], 16).ok())
            .collect()
    };
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let bytes = units(2)?.into_iter().map(|b| b as u8).collect();
        Some(OsString::from_vec(bytes).into())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        Some(OsString::from_wide(&units(4)?).into())
    }
}

/// A rename the filesystem can't do without replacing (it doesn't support the flag).
fn unsupported(e: &io::Error) -> bool {
    #[cfg(unix)]
    if matches!(
        e.raw_os_error(),
        Some(libc::EINVAL | libc::ENOSYS | libc::ENOTSUP)
    ) {
        return true;
    }
    e.kind() == io::ErrorKind::Unsupported
}

/// Renames `from` to `to` without ever replacing `to`: `AlreadyExists` if the name is taken, even
/// by a folder or a symlink. Some filesystems don't support it (FAT and network drives on macOS,
/// some FUSE ones on Linux); [`Staged::publish`] then falls back.
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(unix)]
    let c = |p: &Path| {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::CString::new(p.as_os_str().as_bytes())
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
    };
    #[cfg(target_os = "macos")]
    {
        let (from, to) = (c(from)?, c(to)?);
        // SAFETY: two NUL-terminated paths that outlive the call.
        if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } == 0 {
            return Ok(());
        }
        Err(io::Error::last_os_error())
    }
    #[cfg(target_os = "linux")]
    {
        let (from, to) = (c(from)?, c(to)?);
        // The raw system call, so any glibc works (its wrapper is 2.28+).
        // SAFETY: two NUL-terminated paths that outlive the call; AT_FDCWD resolves them as given.
        let done = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if done == 0 {
            return Ok(());
        }
        Err(io::Error::last_os_error())
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
        let (from, to) = (verbatim(from)?, verbatim(to)?);
        // No MOVEFILE_REPLACE_EXISTING: an existing `to` fails with ERROR_ALREADY_EXISTS.
        // SAFETY: two NUL-terminated UTF-16 paths that outlive the call.
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } != 0 {
            return Ok(());
        }
        Err(io::Error::last_os_error())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        let _ = (from, to);
        Err(io::ErrorKind::Unsupported.into())
    }
}

/// `path` as a NUL-terminated `\\?\` path, which Windows doesn't limit to 260 characters (a
/// folder plus a 255-character name can be longer), as `std::fs` does for its own calls.
#[cfg(windows)]
fn verbatim(path: &Path) -> io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    // Absolute, with `/`, `.` and `..` resolved: a `\\?\` path is used exactly as written.
    let full: Vec<u16> = std::path::absolute(path)?
        .as_os_str()
        .encode_wide()
        .collect();
    let text = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    let mut out = if full.starts_with(&text(r"\\?\")) {
        Vec::new()
    } else if full.starts_with(&text(r"\\")) {
        let mut unc = text(r"\\?\UNC");
        unc.extend_from_slice(&full[1..]);
        return Ok(unc.into_iter().chain([0]).collect());
    } else {
        text(r"\\?\")
    };
    out.extend(full);
    out.push(0);
    Ok(out)
}

/// Copies in 1 MiB chunks, stopping if Queue starts quitting (#67): a quit mid-copy then
/// removes the temporary copy.
fn copy_unless_quitting(from: &mut fs::File, to: &mut fs::File) -> io::Result<u64> {
    use std::io::{Read, Write};
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0;
    loop {
        if proc::stopping() {
            return Err(io::Error::other(QUITTING));
        }
        let n = match from.read(&mut buf) {
            Ok(0) => return Ok(total),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        to.write_all(&buf[..n])?;
        total += n as u64;
    }
}

/// Why a download or fetch stopped early: Queue is quitting (#67). The row isn't marked failed;
/// it comes back after a restart like any other interrupted download.
pub const QUITTING: &str = "Queue is quitting";

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
/// candidate is claimed atomically and without replacing anything ([`place`]), so even two
/// downloads finishing at once can't overwrite each other or an existing file. Across volumes the
/// file is copied only once, however many names are taken.
pub fn place_unique(src: &Path, out_dir: &Path, name: &OsStr) -> io::Result<PathBuf> {
    let candidates = (1..=MAX_COPIES).map(|n| match n {
        1 => out_dir.join(name),
        n => out_dir.join(numbered(name, n)),
    });
    place_first(src, candidates, &STEPS)
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
        if proc::stopping() {
            tree.terminate(); // Queue is quitting (#67)
            return Err(QUITTING.to_string().into());
        }
    }
    // stdout closed, but yt-dlp may still be running: wait for it, unless Queue quits.
    let status = match status {
        Some(status) => status,
        None => match tree.wait_unless_quitting() {
            Ok(Some(status)) => status,
            Ok(None) => {
                tree.terminate();
                return Err(QUITTING.to_string().into());
            }
            Err(e) => {
                tree.terminate();
                return Err(format!("Couldn't wait for yt-dlp: {e}").into());
            }
        },
    };
    let drain_until = drain_until.unwrap_or_else(|| Instant::now() + DRAIN_GRACE);
    let stderr = match proc::recv_until(&err, drain_until) {
        proc::Waited::Got(stderr) => stderr,
        proc::Waited::TimedOut => {
            tree.terminate(); // a descendant still holds stderr open
            err.recv_timeout(Duration::from_secs(1)).unwrap_or_default()
        }
        proc::Waited::Quitting => {
            tree.terminate();
            return Err(QUITTING.to_string().into());
        }
    };
    tree.finish(); // yt-dlp is done: stop anything it left running
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
/// is accepted (#45, D-014). First it removes what an interrupted copy left in the download
/// folder ([`remove_partials`]).
pub fn remove_stale(dir: &Path) -> io::Result<()> {
    let real_dir = |p: &Path| fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_dir());
    let parent = dir.parent().unwrap_or(dir);
    if !real_dir(parent) || !real_dir(dir) {
        return Err(io::Error::other(format!(
            "{} isn't a real folder anymore; not deleting it",
            dir.display()
        )));
    }
    remove_partials(dir);
    fs::remove_dir_all(dir)
}

/// Removes what an interrupted copy left in the download folder (#72), as the notes in a stale
/// work folder record it ([`partial_note`]): the temporary copy, only if its name starts with
/// Queue's partial prefix, and its claim on the final name, only if it's still the same empty
/// file (same file ID) in the same folder. Symlinks and folders are never removed. Nothing else in the download folder is looked at. A copy on a drive that's no longer
/// connected stays where it is.
fn remove_partials(work_dir: &Path) {
    let Ok(entries) = fs::read_dir(work_dir) else {
        return;
    };
    let remove_file_if = |path: &Path, keep: fn(&fs::Metadata) -> bool| {
        if fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && !keep(&m)) {
            let _ = fs::remove_file(path);
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let is_note = name
            .to_str()
            .is_some_and(|n| n.starts_with(PARTIAL_NOTE) && n.ends_with(".txt"));
        if !is_note || !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let mut lines = text.lines();
        let Some(temp) = lines.next().and_then(from_hex) else {
            continue;
        };
        let ours = temp
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|n| n.starts_with(PARTIAL));
        if !ours {
            continue;
        }
        remove_file_if(&temp, |_| false);
        let claim = lines.next().and_then(from_hex);
        let id = lines.next().and_then(|l| {
            let (dev, ino) = l.split_once(' ')?;
            Some((dev.parse::<u64>().ok()?, ino.parse::<u64>().ok()?))
        });
        if let (Some(claim), Some(id)) = (claim, id)
            && claim.parent() == temp.parent()
            && fs::symlink_metadata(&claim)
                .is_ok_and(|m| m.is_file() && m.len() == 0 && file_id(&m) == Some(id))
        {
            let _ = fs::remove_file(&claim);
        }
    }
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

    /// The real steps, except a link that fails the way `hard_link` does across volumes
    /// (EXDEV) or on FAT.
    const CROSS_VOLUME: Steps = Steps {
        link: |_, _| {
            Err(io::Error::new(
                io::ErrorKind::CrossesDevices,
                "cross-device link",
            ))
        },
        ..STEPS
    };

    #[test]
    fn place_falls_back_to_a_copy_when_hard_links_are_unavailable() {
        let dir = temp("cross");
        let (src, dest) = (dir.join("src.webm"), dir.join("dest.webm"));
        fs::write(&src, b"new").unwrap();
        place_via(&src, &dest, &CROSS_VOLUME).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"new");
        assert!(!src.exists(), "the copy moves the file");
        fs::write(&src, b"newer").unwrap();
        let err = place_via(&src, &dest, &CROSS_VOLUME).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&dest).unwrap(), b"new", "existing file untouched");
        assert!(src.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_link_that_finds_the_name_taken_never_falls_back_to_a_copy() {
        let dir = temp("taken");
        let (src, dest) = (dir.join("src.webm"), dir.join("dest.webm"));
        fs::write(&src, b"new").unwrap();
        let taken = Steps {
            link: |_, _| Err(io::Error::from(io::ErrorKind::AlreadyExists)),
            copy: |_, _| panic!("must not copy"),
            ..STEPS
        };
        let err = place_via(&src, &dest, &taken).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(src.exists() && !dest.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_copy_failing_midway_or_at_sync_removes_the_partial_file_and_keeps_the_source() {
        use std::io::{Read, Write};
        let dir = temp("midcopy");
        let src = dir.join("src.webm");
        fs::write(&src, vec![7u8; 64 * 1024]).unwrap();
        let mid_copy = Steps {
            copy: |from, to| {
                let mut half = vec![0u8; 32 * 1024];
                from.read_exact(&mut half)?;
                to.write_all(&half)?;
                Err(io::Error::other("disk full"))
            },
            ..CROSS_VOLUME
        };
        // Only the sync fails: the real copy runs first, and its sync result must be checked.
        let at_sync = Steps {
            sync: |_| Err(io::Error::other("sync failed")),
            ..CROSS_VOLUME
        };
        for (what, name, steps, reason) in [
            ("mid-copy", "a.webm", &mid_copy, "disk full"),
            ("sync", "b.webm", &at_sync, "sync failed"),
        ] {
            let err = place_via(&src, &dir.join(name), steps).unwrap_err();
            assert_eq!(err.to_string(), reason, "{what}: the failure is reported");
            assert!(
                !dir.join(name).exists(),
                "{what}: no partial destination left"
            );
            assert_eq!(
                fs::read(&src).unwrap().len(),
                64 * 1024,
                "{what}: source kept"
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }

    /// A work folder and a download folder, as a real download has them.
    fn work_and_out(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let dir = temp(tag);
        let (work, out) = (dir.join("work"), dir.join("out"));
        fs::create_dir(&work).unwrap();
        fs::create_dir(&out).unwrap();
        (dir, work, out)
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    thread_local! {
        /// What a copy step saw in the download folder while it ran (#72).
        static SEEN: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
        static OUT: std::cell::RefCell<PathBuf> = const { std::cell::RefCell::new(PathBuf::new()) };
    }

    /// The real copy, recording the download folder's contents first.
    const WATCHED: Steps = Steps {
        copy: |from, to| {
            SEEN.with(|seen| *seen.borrow_mut() = OUT.with(|out| names(&out.borrow())));
            io::copy(from, to)
        },
        ..CROSS_VOLUME
    };

    #[test]
    fn a_copy_runs_under_a_temporary_name_and_appears_whole_under_the_real_one() {
        let (dir, work, out) = work_and_out("staged");
        let src = work.join("Title.webm");
        fs::write(&src, b"whole file").unwrap();
        OUT.with(|o| *o.borrow_mut() = out.clone());
        place_via(&src, &out.join("Title.webm"), &WATCHED).unwrap();
        let seen = SEEN.with(|s| s.borrow().clone());
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert!(
            seen[0].starts_with(PARTIAL),
            "only the temporary copy: {seen:?}"
        );
        assert_eq!(names(&out), ["Title.webm"], "the temporary name is gone");
        assert_eq!(fs::read(out.join("Title.webm")).unwrap(), b"whole file");
        assert!(names(&work).is_empty(), "the source and its note are gone");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_copy_leaves_nothing_in_either_folder_but_the_source() {
        use std::io::{Read, Write};
        let (dir, work, out) = work_and_out("staged-fail");
        let src = work.join("Title.webm");
        fs::write(&src, vec![7u8; 64 * 1024]).unwrap();
        let mid_copy = Steps {
            copy: |from, to| {
                let mut half = vec![0u8; 32 * 1024];
                from.read_exact(&mut half)?;
                to.write_all(&half)?;
                Err(io::Error::other("disk full"))
            },
            ..CROSS_VOLUME
        };
        let err = place_via(&src, &out.join("Title.webm"), &mid_copy).unwrap_err();
        assert_eq!(err.to_string(), "disk full");
        assert!(names(&out).is_empty(), "{:?}", names(&out));
        assert_eq!(names(&work), ["Title.webm"], "the note is gone too");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn numbered_names_across_volumes_copy_the_file_only_once() {
        static COPIES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let (dir, work, out) = work_and_out("staged-once");
        let src = work.join("Title.webm");
        fs::write(&src, b"new").unwrap();
        fs::write(out.join("Title.webm"), b"first").unwrap();
        let counted = Steps {
            copy: |from, to| {
                COPIES.fetch_add(1, Ordering::Relaxed);
                io::copy(from, to)
            },
            // Each name looks free beforehand but is taken when publishing, except the third.
            publish: |from, to| {
                if to.to_string_lossy().ends_with("(3).webm") {
                    rename_no_replace(from, to)
                } else {
                    Err(io::ErrorKind::AlreadyExists.into())
                }
            },
            ..CROSS_VOLUME
        };
        let names_tried = (1..=4).map(|n| match n {
            1 => out.join("Title.webm"),
            n => out.join(numbered(OsStr::new("Title.webm"), n)),
        });
        let dest = place_first(&src, names_tried, &counted).unwrap();
        assert_eq!(dest, out.join("Title (3).webm"));
        assert_eq!(COPIES.load(Ordering::Relaxed), 1);
        assert_eq!(fs::read(out.join("Title.webm")).unwrap(), b"first");
        assert_eq!(names(&out), ["Title (3).webm", "Title.webm"]);
        fs::remove_dir_all(dir).unwrap();
    }

    /// A filesystem without a no-replace rename (FAT on macOS, some FUSE drives on Linux).
    const NO_EXCL: Steps = Steps {
        publish: |_, _| Err(io::ErrorKind::Unsupported.into()),
        ..CROSS_VOLUME
    };

    #[test]
    fn without_a_no_replace_rename_an_empty_claim_is_renamed_over_and_never_an_existing_file() {
        let (dir, work, out) = work_and_out("claim");
        let src = work.join("Title.webm");
        fs::write(&src, b"new").unwrap();
        place_via(&src, &out.join("Title.webm"), &NO_EXCL).unwrap();
        assert_eq!(fs::read(out.join("Title.webm")).unwrap(), b"new");
        assert_eq!(names(&out), ["Title.webm"]);
        assert!(names(&work).is_empty());
        // Taken between the early check and the claim: the claim fails, nothing is replaced.
        fs::write(&src, b"newer").unwrap();
        let racing = Steps {
            publish: |_, to| {
                fs::write(to, b"someone else's").unwrap();
                Err(io::ErrorKind::Unsupported.into())
            },
            ..CROSS_VOLUME
        };
        let err = place_via(&src, &out.join("Other.webm"), &racing).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(out.join("Other.webm")).unwrap(), b"someone else's");
        assert_eq!(names(&out), ["Other.webm", "Title.webm"]);
        assert_eq!(names(&work), ["Title.webm"]);
        fs::remove_dir_all(dir).unwrap();
    }

    /// #72 test FAIL 1: a claim `create_new` refused (another writer's empty file got there
    /// first) must never be recorded, or a launch after a crash would delete that file.
    #[test]
    fn a_lost_claim_is_never_recorded_so_a_launch_keeps_the_winners_empty_file() {
        let (dir, work, out) = work_and_out("lost-claim");
        let src = work.join("Title.webm");
        fs::write(&src, b"new").unwrap();
        let mut staged = Staged::copy(&src, &out, &NO_EXCL).unwrap();
        let taken = out.join("Title.webm");
        fs::write(&taken, b"").unwrap();
        let err = staged.publish(&taken, &NO_EXCL).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read(&staged.note).unwrap(),
            partial_note(&staged.temp, None)
        );
        // Queue crashes here: no destructor runs. The next launch removes only the copy.
        std::mem::forget(staged);
        let downloads = dir.join("downloads");
        fs::create_dir(&downloads).unwrap();
        let stale = downloads.join("item-1");
        fs::rename(&work, &stale).unwrap();
        remove_stale(&stale).unwrap();
        assert_eq!(names(&out), ["Title.webm"]);
        assert!(
            fs::read(&taken).unwrap().is_empty(),
            "the winner's file is kept"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn note_paths_survive_newlines_and_any_characters() {
        let dir = Path::new("out\n日本語 e\u{301} \\ ;");
        let (temp, claim) = (dir.join(".queue-partial-1-0"), dir.join("Title\n(2).webm"));
        let id = (u64::MAX, 7);
        let text = String::from_utf8(partial_note(&temp, Some((&claim, id)))).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text:?}");
        assert_eq!(from_hex(lines[0]), Some(temp));
        assert_eq!(from_hex(lines[1]), Some(claim));
        assert_eq!(lines[2], format!("{} 7", u64::MAX));
        for bad in ["", "abc", "zz", "out/.queue-partial-1-0"] {
            assert_eq!(from_hex(bad), None, "{bad:?}");
        }
    }

    /// #72 review P1: another program replacing the fallback's claim before the rename keeps its
    /// file, whether it's empty or not; the download takes the next name.
    #[test]
    fn a_claim_replaced_before_the_rename_is_left_alone_and_the_next_name_is_used() {
        let mut cases = vec![("full", b"someone else's".as_slice())];
        // Windows has no file ID in Rust: an empty replacement looks like our own claim there.
        if cfg!(unix) {
            cases.push(("empty", b"".as_slice()));
        }
        for (tag, bytes) in cases {
            let (dir, work, out) = work_and_out(&format!("claim-swapped-{tag}"));
            let src = work.join("Title.webm");
            fs::write(&src, b"new").unwrap();
            fs::write(dir.join("theirs"), bytes).unwrap();
            // Replaced by rename, as a sync client does: a new file under the same name.
            let swap = |name: &Path| {
                if name.ends_with("Title.webm") {
                    let theirs = name.parent().unwrap().parent().unwrap().join("theirs");
                    fs::rename(theirs, name).unwrap();
                }
            };
            let swapped = Steps {
                claimed: swap,
                ..NO_EXCL
            };
            let candidates = [out.join("Title.webm"), out.join("Title (2).webm")];
            let dest = place_first(&src, candidates, &swapped).unwrap();
            assert_eq!(dest, out.join("Title (2).webm"), "{tag}");
            assert_eq!(fs::read(&dest).unwrap(), b"new", "{tag}");
            assert_eq!(
                fs::read(out.join("Title.webm")).unwrap(),
                bytes,
                "{tag}: theirs is kept"
            );
            assert!(names(&work).is_empty(), "{tag}: {:?}", names(&work));
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn the_real_no_replace_rename_refuses_files_folders_and_symlinks() {
        let dir = temp("rename-excl");
        let from = dir.join("from");
        fs::write(&from, b"new").unwrap();
        let file = dir.join("file");
        fs::write(&file, b"keep").unwrap();
        let folder = dir.join("folder");
        fs::create_dir(&folder).unwrap();
        #[cfg_attr(not(unix), allow(unused_mut))] // only Unix adds a dangling symlink
        let mut taken = vec![file.clone(), folder.clone()];
        #[cfg(unix)]
        {
            let dangling = dir.join("dangling");
            std::os::unix::fs::symlink(dir.join("missing"), &dangling).unwrap();
            taken.push(dangling);
        }
        for to in &taken {
            let err = rename_no_replace(&from, to).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::AlreadyExists, "{}", to.display());
        }
        assert_eq!(fs::read(&file).unwrap(), b"keep");
        assert_eq!(fs::read(&from).unwrap(), b"new");
        // Longer than Windows' old 260-character limit, so `\\?\` paths are needed there.
        let deep = dir.join("d".repeat(200));
        fs::create_dir(&deep).unwrap();
        let long = deep.join(format!("{}.webm", "日".repeat(80)));
        let units = std::path::absolute(&long)
            .unwrap()
            .as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .count();
        assert!(units > 260, "{units}");
        rename_no_replace(&from, &long).unwrap();
        assert_eq!(fs::read(&long).unwrap(), b"new");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_launch_removes_only_what_an_interrupted_copy_recorded() {
        let (dir, work, out) = work_and_out("leftovers");
        let downloads = dir.join("downloads");
        fs::create_dir(&downloads).unwrap();
        let stale = downloads.join("item-1");
        fs::rename(&work, &stale).unwrap();
        let elsewhere = dir.join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        let put = |path: &Path, bytes: &[u8]| fs::write(path, bytes).unwrap();
        // A copy cut off mid-way, and one whose empty claim was left behind.
        put(&out.join(".queue-partial-1-0"), b"cut off");
        put(&out.join(".queue-partial-1-1"), b"whole");
        put(&out.join("Claimed.webm"), b"");
        put(&out.join("Finished.webm"), b"renamed over its claim");
        put(&elsewhere.join("Empty.webm"), b"");
        put(&out.join("Mine.webm"), b"a user file");
        // Notes as Queue writes them: each claim with the file ID it had when Queue made it.
        let note = |n: u32, temp: &Path, claim: Option<&Path>| {
            let id = claim.map(|c| file_id(&fs::symlink_metadata(c).unwrap()));
            let claim = claim.zip(id.flatten());
            let name = format!("queue-partial-1-{n}.txt");
            put(&stale.join(name), &partial_note(temp, claim));
        };
        note(0, &out.join(".queue-partial-1-0"), None);
        note(
            1,
            &out.join(".queue-partial-1-1"),
            Some(&out.join("Claimed.webm")),
        );
        note(
            2,
            &out.join(".queue-partial-1-2"),
            Some(&out.join("Finished.webm")),
        );
        note(
            3,
            &out.join(".queue-partial-1-3"),
            Some(&elsewhere.join("Empty.webm")),
        );
        // #72 review round 2: after the crash, another program replaced the claim with its own
        // empty file. It's a different file, so it's kept.
        put(&out.join("Replaced.webm"), b"");
        note(
            6,
            &out.join(".queue-partial-1-6"),
            Some(&out.join("Replaced.webm")),
        );
        put(&dir.join("theirs"), b"");
        fs::rename(dir.join("theirs"), out.join("Replaced.webm")).unwrap();
        // A note that names something that isn't a Queue copy is ignored entirely, and so is
        // one in another format.
        note(4, &out.join("Mine.webm"), None);
        put(&out.join(".queue-partial-1-9"), b"named only in plain text");
        let plain = out
            .join(".queue-partial-1-9")
            .to_string_lossy()
            .into_owned();
        put(&stale.join("queue-partial-1-5.txt"), plain.as_bytes());
        remove_stale(&stale).unwrap();
        assert!(!stale.exists());
        // Without file IDs (Windows) no claim can be recorded, so none is ever removed.
        let mut kept = vec![
            ".queue-partial-1-9",
            "Finished.webm",
            "Mine.webm",
            "Replaced.webm",
        ];
        if cfg!(not(unix)) {
            kept.insert(1, "Claimed.webm");
        }
        assert_eq!(names(&out), kept);
        assert_eq!(
            names(&elsewhere),
            ["Empty.webm"],
            "a claim outside the copy's folder"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    /// A real cross-volume move where the machine has one: `QUEUE_QA_OTHER_VOLUME` (a folder on
    /// another drive, e.g. an exFAT USB drive, which also has no hard links), or else Linux's
    /// /dev/shm, usually tmpfs apart from the temp folder. Elsewhere the injected tests above
    /// cover the fallback.
    #[cfg(unix)]
    #[test]
    fn place_moves_across_real_volumes_when_available() {
        use std::os::unix::fs::MetadataExt;
        let chosen = std::env::var_os("QUEUE_QA_OTHER_VOLUME").map(PathBuf::from);
        let volume = chosen.clone().unwrap_or_else(|| PathBuf::from("/dev/shm"));
        let dir = temp("realcross");
        let dev = |p: &Path| fs::metadata(p).map(|m| m.dev());
        if !volume.is_dir() || dev(&volume).ok() == dev(&dir).ok() {
            assert!(
                chosen.is_none(),
                "QUEUE_QA_OTHER_VOLUME must be a folder on another drive"
            );
            eprintln!("no second volume here; skipped");
            return fs::remove_dir_all(dir).unwrap();
        }
        // A folder of our own there; a read-only /dev/shm skips rather than fails.
        let other = volume.join(format!("queue-place-{}", std::process::id()));
        if let Err(e) = fs::create_dir(&other) {
            assert!(
                chosen.is_none(),
                "can't write to QUEUE_QA_OTHER_VOLUME: {e}"
            );
            eprintln!("can't write to {}: {e}; skipped", volume.display());
            return fs::remove_dir_all(dir).unwrap();
        }
        let src = dir.join("src.webm");
        fs::write(&src, b"across").unwrap();
        let dest = other.join("dest.webm");
        place(&src, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"across");
        assert!(!src.exists());
        // #72: no temporary copy or claim is left (on exFAT, macOS has no no-replace rename, so
        // this goes through the empty claim). `._` files are macOS's own extended-attribute
        // files on volumes like exFAT.
        let left: Vec<_> = names(&other)
            .into_iter()
            .filter(|n| !n.starts_with("._"))
            .collect();
        assert_eq!(left, ["dest.webm"]);
        assert!(names(&dir).is_empty(), "the source and its note are gone");
        fs::remove_dir_all(other).unwrap();
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
