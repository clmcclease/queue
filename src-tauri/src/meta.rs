//! Item metadata from `yt-dlp -J` (Q3, #6). Parsing is pure and tested; running yt-dlp is in
//! [`fetch`]. Every field is untrusted text from the site and is only ever displayed as text.
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::download::failure_reason;
use crate::proc;
use crate::sidecar::{self, Sidecar};

/// What the queue row shows before downloading. Field names follow yt-dlp's info dict.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub title: Option<String>,
    /// `channel`, else `uploader` (not every extractor has channels).
    pub channel: Option<String>,
    /// Seconds; absent for live streams and some sites.
    pub duration: Option<f64>,
    /// An `http(s)` image URL, or none.
    pub thumbnail: Option<String>,
    /// yt-dlp's extractor key, e.g. `Youtube`, `Vimeo`.
    pub site: Option<String>,
    /// For naming (#14): `YYYYMMDD`, when the site gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_date: Option<String>,
    /// Music: `artists` joined with ", " (else `artist`) and `track`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    /// Shows: the series and its season/episode numbers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season_number: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_number: Option<u32>,
    /// The start of the description, for the AI naming prompt (M4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Deserialize)]
struct Info {
    #[serde(rename = "_type")]
    kind: Option<String>,
    title: Option<String>,
    channel: Option<String>,
    uploader: Option<String>,
    duration: Option<f64>,
    thumbnail: Option<String>,
    extractor_key: Option<String>,
    // Naming fields (#14). Types vary between extractors, so the loose ones are read as JSON
    // values: an odd value is dropped instead of failing the whole fetch.
    upload_date: Option<serde_json::Value>,
    artists: Option<serde_json::Value>,
    artist: Option<serde_json::Value>,
    track: Option<serde_json::Value>,
    series: Option<serde_json::Value>,
    season_number: Option<serde_json::Value>,
    episode_number: Option<serde_json::Value>,
    description: Option<serde_json::Value>,
}

/// Longest title/channel kept; yt-dlp metadata can be arbitrarily long.
const MAX_TEXT: usize = 500;
/// Longest description kept, for the AI prompt (#14, D-029).
const MAX_DESCRIPTION: usize = 300;
/// Longest thumbnail URL kept. URLs are never truncated (that breaks signed CDN links); an
/// absurdly long one is dropped instead.
const MAX_URL: usize = 8192;

fn clean(text: Option<String>) -> Option<String> {
    let text = text?;
    let text: String = text
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_TEXT)
        .collect();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// A JSON string (or a number, which some extractors use for text fields) as clean text.
fn text(value: Option<serde_json::Value>) -> Option<String> {
    match value? {
        serde_json::Value::String(s) => clean(Some(s)),
        serde_json::Value::Number(n) => clean(Some(n.to_string())),
        _ => None,
    }
}

/// A small whole number (season/episode), from a number or a numeric string.
fn number(value: Option<serde_json::Value>) -> Option<u32> {
    match value? {
        serde_json::Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// `artists` (a list) joined with ", ", else the older `artist` text.
fn artists(list: Option<serde_json::Value>, single: Option<serde_json::Value>) -> Option<String> {
    let joined = match list {
        Some(serde_json::Value::Array(items)) => {
            let names: Vec<String> = items.into_iter().filter_map(|v| text(Some(v))).collect();
            clean(Some(names.join(", ")))
        }
        _ => None,
    };
    joined.or_else(|| text(single))
}

/// Parses `yt-dlp -J` output. Playlists are refused until #27 expands them into items.
pub fn parse(json: &[u8]) -> Result<Meta, String> {
    let info: Info = serde_json::from_slice(json)
        .map_err(|e| format!("yt-dlp returned unreadable info: {e}"))?;
    if matches!(info.kind.as_deref(), Some("playlist" | "multi_video")) {
        return Err("Playlists aren't supported yet; add a single video link.".into());
    }
    let thumbnail = info.thumbnail.map(|t| t.trim().to_owned()).filter(|t| {
        let lower = t.to_ascii_lowercase();
        t.len() <= MAX_URL
            && !t.chars().any(|c| c.is_control() || c.is_whitespace())
            && (lower.starts_with("https://") || lower.starts_with("http://"))
    });
    Ok(Meta {
        title: clean(info.title),
        channel: clean(info.channel).or_else(|| clean(info.uploader)),
        duration: info.duration.filter(|d| d.is_finite() && *d >= 0.0),
        thumbnail,
        site: clean(info.extractor_key),
        upload_date: text(info.upload_date)
            .filter(|d| d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit())),
        artist: artists(info.artists, info.artist),
        track: text(info.track),
        series: text(info.series),
        season_number: number(info.season_number),
        episode_number: number(info.episode_number),
        description: text(info.description)
            .map(|d| {
                d.chars()
                    .take(MAX_DESCRIPTION)
                    .collect::<String>()
                    .trim()
                    .to_owned()
            })
            .filter(|d| !d.is_empty()),
    })
}

/// The yt-dlp argv for metadata. `url` is untrusted and goes after `--`.
pub fn args(sidecar_dir: &Path, url: &str) -> Vec<OsString> {
    let mut args = sidecar::ytdlp_base_args(sidecar_dir);
    // --flat-playlist: a playlist-only link returns its summary at once (refused below until
    // #27) instead of extracting every video and hitting the time limit. Single videos and
    // `watch?v=…&list=…` (with --no-playlist) are unaffected.
    args.extend(["--no-playlist", "--flat-playlist", "-J", "--"].map(OsString::from));
    args.push(url.into());
    args
}

/// Most `-J` output accepted (YouTube's is ~1–2 MB); more is treated as a failure.
const MAX_STDOUT: usize = 32 * 1024 * 1024;
/// Stderr kept for the failure reason: the tail is where yt-dlp's `ERROR:` line is.
const STDERR_TAIL: usize = 64 * 1024;

/// Runs `yt-dlp -J` with one time limit covering the whole operation, including reading its
/// output, and with bounded memory. On timeout, overflow, or error the whole process tree is
/// stopped (see [`proc`]).
pub fn fetch(sidecar_dir: &Path, url: &str, limit: Duration) -> Result<Meta, String> {
    fetch_raw(sidecar_dir, url, limit).map(|(meta, _)| meta)
}

/// [`fetch`], also returning yt-dlp's raw `-J` JSON so a download can reuse it (D-017).
pub fn fetch_raw(
    sidecar_dir: &Path,
    url: &str,
    limit: Duration,
) -> Result<(Meta, Vec<u8>), String> {
    let deadline = Instant::now() + limit;
    let timed_out = || {
        format!(
            "Getting the video's info took longer than {} s",
            limit.as_secs()
        )
    };
    let mut cmd = Command::new(Sidecar::YtDlp.path_in(sidecar_dir));
    cmd.args(args(sidecar_dir, url))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut tree = proc::spawn_tree(&mut cmd).map_err(|e| format!("Couldn't start yt-dlp: {e}"))?;
    let stdout = tree.child.stdout.take().expect("piped");
    let stderr = tree.child.stderr.take().expect("piped");
    // Readers hand their results back over channels, so waiting for them shares the deadline.
    let (out, err) = match (
        reader("stdout", move || proc::read_capped(stdout, MAX_STDOUT)),
        reader("stderr", move || proc::read_tail(stderr, STDERR_TAIL)),
    ) {
        (Ok(out), Ok(err)) => (out, err),
        (Err(e), _) | (_, Err(e)) => {
            tree.terminate();
            return Err(format!("Couldn't read yt-dlp's output: {e}"));
        }
    };
    let mut stdout_result = None; // filled as soon as the reader finishes
    let status = loop {
        match tree.child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                tree.terminate();
                return Err(timed_out());
            }
            Ok(None) if proc::stopping() => {
                tree.terminate(); // Queue is quitting (#67)
                return Err(crate::download::QUITTING.to_string());
            }
            Ok(None) => {
                // Stop early if stdout overflowed instead of waiting out the deadline.
                if stdout_result.is_none()
                    && let Ok(result) = out.try_recv()
                {
                    if let Err(e) = &result {
                        tree.terminate();
                        return Err(format!("yt-dlp produced too much output ({e})"));
                    }
                    stdout_result = Some(result);
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                tree.terminate();
                return Err(format!("Couldn't wait for yt-dlp: {e}"));
            }
        }
    };
    // From here on, every early return stops the tree: the leader has exited, but a descendant
    // may still hold the pipes (and keep the process group alive). A quit cuts the wait short.
    let stdout =
        match stdout_result.map_or_else(|| proc::recv_until(&out, deadline), proc::Waited::Got) {
            proc::Waited::Got(Ok(stdout)) => stdout,
            proc::Waited::Got(Err(e)) => {
                tree.terminate(); // overflow: don't wait on stderr first
                return Err(format!("yt-dlp produced too much output ({e})"));
            }
            proc::Waited::TimedOut => {
                tree.terminate(); // a descendant still holds stdout open
                return Err(timed_out());
            }
            proc::Waited::Quitting => {
                tree.terminate();
                return Err(crate::download::QUITTING.to_string());
            }
        };
    let stderr = match proc::recv_until(&err, deadline) {
        proc::Waited::Got(stderr) => stderr,
        proc::Waited::TimedOut => {
            tree.terminate(); // a descendant still holds stderr open
            return Err(timed_out());
        }
        proc::Waited::Quitting => {
            tree.terminate();
            return Err(crate::download::QUITTING.to_string());
        }
    };
    tree.finish(); // yt-dlp is done: stop anything it left running
    let stderr = stderr.unwrap_or_default();
    if !status.success() {
        return Err(failure_reason(
            &String::from_utf8_lossy(&stderr),
            &status.to_string(),
        ));
    }
    parse(&stdout).map(|meta| (meta, stdout))
}

/// A per-run folder for saved `-J` files (D-017). Item ids restart at 1 in every run, and a dev
/// build and the installed app share the cache dir, so each run needs its own namespace.
pub fn session_dir(cache_dir: &Path) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    // A counter too: two calls can see the same timestamp on a coarse clock.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    cache_dir
        .join("info")
        .join(format!("session-{}-{stamp}-{n}", std::process::id()))
}

/// Saves an item's raw `-J` as `<session>/item-<id>.json`, never overwriting an existing file.
pub fn save_info(session: &Path, id: u64, raw: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    std::fs::create_dir_all(session)?;
    let path = session.join(format!("item-{id}.json"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    if let Err(e) = file.write_all(raw).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    Ok(path)
}

/// Runs `read` on a named thread and returns a channel for its result. Thread creation can
/// fail (resource limits); that is reported instead of panicking the worker.
fn reader(
    name: &str,
    read: impl FnOnce() -> std::io::Result<Vec<u8>> + Send + 'static,
) -> std::io::Result<mpsc::Receiver<std::io::Result<Vec<u8>>>> {
    let (done, result) = mpsc::channel();
    std::thread::Builder::new()
        .name(format!("yt-dlp {name}"))
        .spawn(move || {
            let _ = done.send(read());
        })?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_fields_the_row_shows() {
        let json = br#"{"_type":"video","title":"Me at the zoo","channel":"jawed","uploader":"x",
            "duration":19,"thumbnail":"https://i.ytimg.com/vi/jNQXAC9IVRw/hq.jpg",
            "extractor_key":"Youtube","formats":[{"format_id":"18"}]}"#;
        assert_eq!(
            parse(json).unwrap(),
            Meta {
                title: Some("Me at the zoo".into()),
                channel: Some("jawed".into()),
                duration: Some(19.0),
                thumbnail: Some("https://i.ytimg.com/vi/jNQXAC9IVRw/hq.jpg".into()),
                site: Some("Youtube".into()),
                ..Default::default()
            }
        );
    }

    /// #14: the structured naming fields, read loosely; odd values are dropped, never fatal.
    #[test]
    fn parses_naming_fields_and_drops_odd_values() {
        let json = br#"{"title":"Get Lucky","upload_date":"20130419",
            "artists":["Daft Punk","Pharrell Williams"],"artist":"ignored","track":"Get Lucky",
            "series":"The Office","season_number":2,"episode_number":"7",
            "description":"  A song.  "}"#;
        let meta = parse(json).unwrap();
        assert_eq!(meta.upload_date.as_deref(), Some("20130419"));
        assert_eq!(meta.artist.as_deref(), Some("Daft Punk, Pharrell Williams"));
        assert_eq!(meta.track.as_deref(), Some("Get Lucky"));
        assert_eq!(meta.series.as_deref(), Some("The Office"));
        assert_eq!(
            (meta.season_number, meta.episode_number),
            (Some(2), Some(7))
        );
        assert_eq!(meta.description.as_deref(), Some("A song."));

        let odd = br#"{"upload_date":"2013-04-19","artists":"not a list","artist":"Solo",
            "season_number":-1,"episode_number":{"x":1},"track":["a"],"description":"x"}"#;
        let meta = parse(odd).unwrap();
        assert_eq!(meta.upload_date, None, "only YYYYMMDD");
        assert_eq!(meta.artist.as_deref(), Some("Solo"), "falls back to artist");
        assert_eq!(
            (meta.season_number, meta.episode_number, meta.track),
            (None, None, None)
        );

        let long = format!(r#"{{"description":"{}"}}"#, "é".repeat(1000));
        let meta = parse(long.as_bytes()).unwrap();
        assert_eq!(meta.description.unwrap().chars().count(), 300);
    }

    /// #14: a `queue.json` saved before the naming fields existed still loads, and a row
    /// without them serializes exactly as before.
    #[test]
    fn naming_fields_are_optional_in_saved_and_sent_json() {
        let old: Meta = serde_json::from_str(
            r#"{"title":"T","channel":null,"duration":null,
            "thumbnail":null,"site":null}"#,
        )
        .unwrap();
        assert_eq!(old.title.as_deref(), Some("T"));
        assert_eq!(
            serde_json::to_string(&old).unwrap(),
            r#"{"title":"T","channel":null,"duration":null,"thumbnail":null,"site":null}"#
        );
    }

    #[test]
    fn falls_back_to_uploader_and_tolerates_missing_fields() {
        let meta = parse(br#"{"title":"T","uploader":"U"}"#).unwrap();
        assert_eq!(meta.channel.as_deref(), Some("U"));
        assert_eq!(
            (meta.duration, meta.thumbnail, meta.site),
            (None, None, None)
        );
        assert_eq!(parse(b"{}").unwrap(), Meta::default());
    }

    #[test]
    fn untrusted_values_are_bounded_and_filtered() {
        let long = "日".repeat(2000);
        let json = format!(
            r#"{{"title":"{long}","channel":"  a\u0000b\u001b[31m  ","duration":-5,
            "thumbnail":"javascript:alert(1)"}}"#
        );
        let meta = parse(json.as_bytes()).unwrap();
        assert_eq!(meta.title.unwrap().chars().count(), MAX_TEXT);
        assert_eq!(meta.channel.as_deref(), Some("ab[31m"));
        assert_eq!(meta.duration, None, "negative duration dropped");
        assert_eq!(meta.thumbnail, None, "only http(s) thumbnails");
        assert_eq!(
            parse(br#"{"duration":1e999}"#)
                .ok()
                .and_then(|m| m.duration),
            None
        );
    }

    #[test]
    fn saved_info_is_namespaced_per_run_and_never_overwritten() {
        let cache = std::env::temp_dir().join(format!("queue-cache-{}", std::process::id()));
        let (a, b) = (session_dir(&cache), session_dir(&cache));
        assert_ne!(
            a, b,
            "two runs (e.g. dev + installed app) never share a folder"
        );
        let pa = save_info(&a, 1, b"video A").unwrap();
        let pb = save_info(&b, 1, b"video B").unwrap();
        assert_ne!(pa, pb);
        assert_eq!(std::fs::read(&pa).unwrap(), b"video A");
        assert!(
            save_info(&a, 1, b"again").is_err(),
            "create_new: no overwrite"
        );
        assert_eq!(std::fs::read(&pa).unwrap(), b"video A");
        std::fs::remove_dir_all(cache).unwrap();
    }

    #[test]
    fn playlists_and_garbage_are_errors() {
        assert!(
            parse(br#"{"_type":"playlist","title":"Mix"}"#)
                .unwrap_err()
                .contains("Playlists")
        );
        assert!(parse(b"not json").unwrap_err().contains("unreadable"));
    }

    #[test]
    fn url_goes_last_after_double_dash_with_no_playlist() {
        let a = args(Path::new("bin"), "--exec=evil");
        let n = a.len();
        assert_eq!(
            (a[n - 2].to_str(), a[n - 1].to_str()),
            (Some("--"), Some("--exec=evil"))
        );
        assert!(a.iter().any(|x| x == "-J") && a.iter().any(|x| x == "--no-playlist"));
        assert!(
            a.iter().any(|x| x == "--flat-playlist"),
            "playlists must not extract every video"
        );
    }
}
