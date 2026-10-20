//! #4 / D2: opt-in tests using the unmodified bundled yt-dlp, with loopback-only fixtures: a
//! WAV page, and separate video and audio streams for real merges (#70, tests/fixtures/merge).
//! Set QUEUE_QA_REAL_SIDECARS to src-tauri/binaries, then run:
//! cargo test --locked --test download_real_d2 -- --ignored --test-threads=1
//! These tests do not contact external sites or write into the user's Downloads folder.
use queue_lib::{download, sidecar::Sidecar};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

static NEXT: AtomicUsize = AtomicUsize::new(0);

#[path = "support/merge_70_qa.rs"]
mod merge_70_qa;

struct LocalDownload {
    root: PathBuf,
    bin: PathBuf,
    out: PathBuf,
    url: String,
    stop: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
}

/// #8 / D4: verifies the actual JSON templates against official yt-dlp using local HTTP.
#[test]
#[ignore = "requires installed pinned sidecars; loopback HTTP only"]
fn d4_real_loopback_download_streams_parseable_progress_and_keeps_both_copies() {
    let f = LocalDownload::new("日本語 &amp; messy title");
    let original = PathBuf::from(f.run().unwrap());
    let original_bytes = fs::read(&original).unwrap();
    let work = f.root.join("work-progress");
    let mut progress = Vec::new();
    let result = download::run_with_retries_events(
        &f.bin,
        &work,
        &f.out,
        &f.url,
        None,
        |_| panic!("loopback fixture must not retry"),
        |_| panic!("loopback fixture must not retry"),
        |event| {
            if let download::Event::Progress(p) = event {
                progress.push(p);
            }
        },
    );
    let copy = PathBuf::from(result.unwrap());
    assert_ne!(copy, original);
    assert_eq!(fs::read(&original).unwrap(), original_bytes);
    assert_eq!(fs::read(&copy).unwrap(), silence_wav());
    assert!(!work.exists());
    assert!(
        !progress.is_empty(),
        "real yt-dlp must emit valid template JSON"
    );
    assert!(progress.iter().all(|p| p.part == 1));
    assert!(progress.iter().any(|p| p.percent == Some(100.0)));
    assert!(
        progress.iter().any(|p| p.speed.is_some_and(|s| s > 0.0)),
        "live speed is parsed"
    );
    assert!(
        progress.iter().any(|p| p.eta.is_some()),
        "live ETA is parsed"
    );
}

/// #11 / D2: the audio and "most compatible" formats with the real yt-dlp. The WAV fixture has
/// no video, no H.264 and no AAC: Audio keeps it as is (no re-encode), and the compatible
/// formats fall back honestly with a notice instead of failing or converting.
#[test]
#[ignore = "requires installed pinned sidecars; loopback HTTP only"]
fn d11_real_formats_download_without_reencoding_and_flag_honest_fallbacks() {
    for (format, notice) in [
        (download::Format::Audio, false),
        (download::Format::CompatibleAudio, true),
        (download::Format::CompatibleVideo, true),
    ] {
        let f = LocalDownload::new(&format!("format {format:?}"));
        let work = f.root.join("work-format");
        // The production path: the codecs yt-dlp reports, then notice_for (as complete_download).
        let saved = download::run_with_retries_saved(
            &f.bin,
            &work,
            &f.out,
            &f.url,
            None,
            format,
            |_| panic!("loopback fixture must not retry"),
            |_| panic!("loopback fixture must not retry"),
            |_| {},
        )
        .unwrap_or_else(|e| panic!("{format:?}: {e}"));
        // yt-dlp's own report for this audio-only page: no video, audio codec unknown.
        assert_eq!(
            saved.codecs.video.as_deref(),
            Some("none"),
            "{format:?}: {saved:?}"
        );
        let path = PathBuf::from(&saved.path);
        assert_eq!(
            fs::read(&path).unwrap(),
            silence_wav(),
            "{format:?}: the same bytes, not re-encoded"
        );
        assert_eq!(path.extension().unwrap(), "wav");
        assert_eq!(
            format.notice_for(&path, &saved.codecs).is_some(),
            notice,
            "{format:?}: {saved:?}"
        );
        assert!(!work.exists());
    }
}

/// A file the loopback server serves: its path, MIME type and bytes. Media is paced.
type Route = (&'static str, &'static str, Vec<u8>);

/// #70: a real video+audio merge with the bundled ffmpeg, over loopback, for every format. The
/// page is a DASH manifest of separate streams (fixtures/merge/README.md), and yt-dlp takes their
/// codecs from it. "full" offers a larger MJPEG video and FLAC audio alongside a small H.264 and
/// AAC; "noh264" has only MJPEG and FLAC, so the compatible formats must fall back honestly.
#[test]
#[ignore = "requires installed pinned sidecars; loopback HTTP only"]
fn d70_real_merge_picks_each_formats_streams_and_reports_processing() {
    use download::Format::*;
    // (page, format, saved extension, streams ffprobe finds, codecs yt-dlp reports, notice)
    let cases = [
        ("full", Default, "mp4", "mjpeg,flac", "mp4v.6C|flac", None),
        (
            "full",
            CompatibleVideo,
            "mp4",
            "h264,aac",
            "avc1.42c00a|mp4a.40.2",
            None,
        ),
        ("full", Audio, "m4a", "flac", "none|flac", None),
        (
            "full",
            CompatibleAudio,
            "m4a",
            "aac",
            "none|mp4a.40.2",
            None,
        ),
        ("noh264", Default, "mp4", "mjpeg,flac", "mp4v.6C|flac", None),
        // `--merge-output-format mp4/webm/mkv` checks codecs too: FLAC goes to .mkv.
        (
            "noh264",
            CompatibleVideo,
            "mkv",
            "mjpeg,flac",
            "mp4v.6C|flac",
            Some("No H.264 version was available"),
        ),
        ("noh264", Audio, "m4a", "flac", "none|flac", None),
        // Saved as .m4a, so only the reported codec shows it isn't AAC.
        (
            "noh264",
            CompatibleAudio,
            "m4a",
            "flac",
            "none|flac",
            Some("No AAC version was available, so it's FLAC audio"),
        ),
    ];
    for page in ["full", "noh264"] {
        let f = LocalDownload::serve(
            "Merge 日本語",
            r#"<video controls><source src="/media.mpd" type="application/dash+xml"></video>"#,
            |bin| merge_routes(bin, page == "full"),
        );
        for (_, format, ext, streams, codecs, notice) in cases.iter().filter(|c| c.0 == page) {
            let case = format!("{page} {format:?}");
            let work = f.root.join(format!("work-{page}-{format:?}"));
            let mut events = Vec::new();
            let saved = download::run_with_retries_saved(
                &f.bin,
                &work,
                &f.out,
                &f.url,
                None,
                *format,
                |_| panic!("loopback fixture must not retry"),
                |_| panic!("loopback fixture must not retry"),
                |event| events.push(event),
            )
            .unwrap_or_else(|e| panic!("{case}: {e}"));
            let path = PathBuf::from(&saved.path);
            assert_eq!(path.parent(), Some(f.out.as_path()), "{case}");
            assert!(
                path.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("Merge 日本語"),
                "{case}: {saved:?}"
            );
            assert_eq!(path.extension().unwrap(), *ext, "{case}: {saved:?}");
            assert_eq!(
                probe_streams(&f.bin, &path),
                *streams,
                "{case}: the streams chosen, copied without re-encoding"
            );
            assert_eq!(saved.codecs, download::Codecs::parse(codecs), "{case}");
            let shown = format.notice_for(&path, &saved.codecs);
            match notice {
                None => assert_eq!(shown, None, "{case}"),
                Some(start) => assert!(
                    shown.as_deref().is_some_and(|n| n.starts_with(start)),
                    "{case}: {shown:?}"
                ),
            }
            let processing: Vec<_> = events
                .iter()
                .filter_map(|e| match e {
                    download::Event::Processing(name) => Some(name.as_str()),
                    _ => None,
                })
                .collect();
            let parts = events
                .iter()
                .filter_map(|e| match e {
                    download::Event::Progress(p) => Some(p.part),
                    _ => None,
                })
                .max();
            if streams.contains(',') {
                assert!(processing.contains(&"Merger"), "{case}: {processing:?}");
                assert_eq!(parts, Some(2), "{case}: video, then audio");
            } else {
                assert!(
                    processing.contains(&"ExtractAudio"),
                    "{case}: {processing:?}"
                );
                assert_eq!(parts, Some(1), "{case}");
            }
            assert!(!work.exists(), "{case}");
            assert_eq!(
                fs::read_dir(&f.out).unwrap().count(),
                1,
                "{case}: only the finished file"
            );
            fs::remove_file(&path).unwrap();
        }
    }
}

/// The DASH manifest and streams for [`d70_real_merge_picks_each_formats_streams_and_reports_processing`].
/// The FLAC stream is made here from [`silence_wav`] by the bundled ffmpeg.
fn merge_routes(bin: &Path, full: bool) -> Vec<Route> {
    let fixture = |name: &str, size: u64| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/merge")
            .join(name);
        let bytes = fs::read(&path).unwrap();
        // A changed size means a damaged checkout or a rebuilt fixture (see its README).
        assert_eq!(bytes.len() as u64, size, "{name}");
        bytes
    };
    let wav = bin.join("silence.wav");
    let flac = bin.join("flac.mp4");
    fs::write(&wav, silence_wav()).unwrap();
    let made = std::process::Command::new(bin.join(Sidecar::Ffmpeg.file_name()))
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(&wav)
        .args(["-c:a", "flac", "-f", "mp4"])
        .arg(&flac)
        .output()
        .unwrap();
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let representation = |id: &str, mime: &str, codecs: &str, extra: &str| {
        format!(
            r#"<Representation id="{id}" mimeType="{mime}" codecs="{codecs}" {extra}><BaseURL>/{id}</BaseURL></Representation>"#
        )
    };
    let mut video = representation(
        "v_mjpeg.mp4",
        "video/mp4",
        "mp4v.6C",
        r#"bandwidth="40000" width="128" height="96" frameRate="10""#,
    );
    let mut audio = representation(
        "a_flac.mp4",
        "audio/mp4",
        "flac",
        r#"bandwidth="64000" audioSamplingRate="8000""#,
    );
    let mut routes = vec![
        ("/v_mjpeg.mp4", "video/mp4", fixture("v_mjpeg.mp4", 3578)),
        ("/a_flac.mp4", "audio/mp4", fs::read(&flac).unwrap()),
    ];
    if full {
        video += &representation(
            "v_h264.mp4",
            "video/mp4",
            "avc1.42c00a",
            r#"bandwidth="20000" width="64" height="48" frameRate="10""#,
        );
        audio += &representation(
            "a_aac.m4a",
            "audio/mp4",
            "mp4a.40.2",
            r#"bandwidth="32000" audioSamplingRate="8000""#,
        );
        routes.push(("/v_h264.mp4", "video/mp4", fixture("v_h264.mp4", 1630)));
        routes.push(("/a_aac.m4a", "audio/mp4", fixture("a_aac.m4a", 4136)));
    }
    let mpd = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT1S" minBufferTime="PT1S" profiles="urn:mpeg:dash:profile:isoff-on-demand:2011"><Period><AdaptationSet contentType="video">{video}</AdaptationSet><AdaptationSet contentType="audio" lang="en">{audio}</AdaptationSet></Period></MPD>"#
    );
    routes.push(("/media.mpd", "application/dash+xml", mpd.into_bytes()));
    routes
}

/// The codec of each stream in `file`, as the bundled ffprobe sees it: "h264,aac".
fn probe_streams(bin: &Path, file: &Path) -> String {
    let out = std::process::Command::new(
        bin.join(format!("queue-ffprobe{}", std::env::consts::EXE_SUFFIX)),
    )
    .args([
        "-v",
        "error",
        "-show_entries",
        "stream=codec_name",
        "-of",
        "csv=p=0",
    ])
    .arg(file)
    .output()
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(",")
}

impl LocalDownload {
    /// A page titled `title` with one WAV `<audio>`.
    fn new(title: &str) -> Self {
        Self::serve(
            title,
            r#"<audio controls src="/media.wav"></audio>"#,
            |_| vec![("/media.wav", "audio/wav", silence_wav())],
        )
    }

    /// A page titled `title` whose body is `media`, serving the files `routes` makes; it gets
    /// the tools folder, so it can make fixtures with the bundled ffmpeg. Any other path is the page.
    fn serve(title: &str, media: &str, routes: impl FnOnce(&Path) -> Vec<Route>) -> Self {
        let root = std::env::temp_dir().join(format!(
            "queue-real-d2-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let bin = root.join("tools");
        let out = root.join("Downloads 日本語");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&out).unwrap();
        let source = PathBuf::from(
            std::env::var_os("QUEUE_QA_REAL_SIDECARS")
                .expect("set QUEUE_QA_REAL_SIDECARS to the pinned sidecar directory"),
        );
        let triple = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            "aarch64-apple-darwin"
        } else if cfg!(target_os = "macos") {
            "x86_64-apple-darwin"
        } else if cfg!(target_os = "windows") {
            "x86_64-pc-windows-msvc"
        } else {
            "x86_64-unknown-linux-gnu"
        };
        for sidecar in [Sidecar::YtDlp, Sidecar::Ffmpeg, Sidecar::Deno] {
            let name = sidecar.file_name();
            let stem = name.trim_end_matches(std::env::consts::EXE_SUFFIX);
            fs::copy(
                source.join(format!("{stem}-{triple}{}", std::env::consts::EXE_SUFFIX)),
                bin.join(name),
            )
            .unwrap();
        }
        // yt-dlp finds ffprobe next to ffmpeg, as in the app (sidecar.rs).
        let ffprobe = format!("queue-ffprobe{}", std::env::consts::EXE_SUFFIX);
        fs::copy(
            source.join(format!(
                "queue-ffprobe-{triple}{}",
                std::env::consts::EXE_SUFFIX
            )),
            bin.join(ffprobe),
        )
        .unwrap();
        let routes = routes(&bin);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let html = format!(
            "<!doctype html><html><head><title>{title}</title></head><body>{media}</body></html>"
        )
        .into_bytes();
        let server = thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => respond(stream, &html, &routes),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("fixture HTTP accept failed: {e}"),
                }
            }
        });
        Self {
            root,
            bin,
            out,
            url: format!("http://{address}/watch"),
            stop,
            server: Some(server),
        }
    }

    fn run(&self) -> Result<String, String> {
        let work = self
            .root
            .join(format!("work-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
        download::run(&self.bin, &work, &self.out, &self.url)
    }
}

impl Drop for LocalDownload {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn silence_wav() -> Vec<u8> {
    // One second of real, mono 16-bit PCM at 8 kHz; no encoder or external fixture dependency.
    let mut data = Vec::new();
    data.extend(b"RIFF");
    data.extend(16036_u32.to_le_bytes());
    data.extend(b"WAVEfmt ");
    data.extend(16_u32.to_le_bytes());
    data.extend(1_u16.to_le_bytes());
    data.extend(1_u16.to_le_bytes());
    data.extend(8000_u32.to_le_bytes());
    data.extend(16000_u32.to_le_bytes());
    data.extend(2_u16.to_le_bytes());
    data.extend(16_u16.to_le_bytes());
    data.extend(b"data");
    data.extend(16000_u32.to_le_bytes());
    data.resize(16044, 0);
    data
}

fn respond(mut stream: TcpStream, html: &[u8], routes: &[Route]) {
    // On Windows a socket accepted from a non-blocking listener is non-blocking too (Linux and
    // macOS hand back a blocking one), so the first read would fail with WouldBlock and drop the
    // connection: yt-dlp then saw WSAECONNABORTED (10053).
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut request = Vec::new();
    let mut buf = [0; 4096];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") && request.len() < 32768 {
        let Ok(n) = stream.read(&mut buf) else {
            return;
        };
        if n == 0 {
            return;
        }
        request.extend_from_slice(&buf[..n]);
    }
    let text = String::from_utf8_lossy(&request);
    let first = text.lines().next().unwrap_or("");
    let path = first.split(' ').nth(1).unwrap_or("");
    let route = routes.iter().find(|(p, _, _)| *p == path);
    let media = route.is_some();
    let (mime, body) = route.map_or(("text/html; charset=utf-8", html), |(_, mime, body)| {
        (*mime, body.as_slice())
    });
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if stream.write_all(headers.as_bytes()).is_ok() && !first.starts_with("HEAD ") {
        // Media is paced (the WAV takes about 0.3 s) so yt-dlp has elapsed time to report speed and ETA;
        // an instant loopback transfer leaves both null.
        let chunk = if media { 1024 } else { body.len().max(1) };
        for part in body.chunks(chunk) {
            if stream.write_all(part).and_then(|_| stream.flush()).is_err() {
                return;
            }
            if media {
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

#[test]
#[ignore = "requires installed pinned sidecars; loopback HTTP only"]
fn d2_real_download_sanitizes_messy_unicode_and_reserved_titles() {
    for title in [
        "CON",
        "NUL",
        "COM1",
        "LPT9",
        " 日本語 🎵 &amp; é : * ? &lt;noise&gt; (OFFICIAL VIDEO) ",
    ] {
        let f = LocalDownload::new(title);
        let path = PathBuf::from(f.run().unwrap());
        assert_eq!(path.parent(), Some(f.out.as_path()));
        assert_eq!(fs::read(&path).unwrap(), silence_wav());
        let file = path.file_name().unwrap().to_str().unwrap();
        assert!(
            file.ends_with(".wav"),
            "native WAV must not be re-encoded: {file}"
        );
        assert!(!file.contains(['<', '>', ':', '"', '/', '\\', '|', '?', '*']));
        let stem = file.split('.').next().unwrap().to_uppercase();
        assert!(
            !["CON", "NUL", "COM1", "LPT9"].contains(&stem.as_str()),
            "Windows reserved stem: {file}"
        );
        if title.contains("日本語") {
            assert!(file.contains("日本語"));
        }
    }
}

#[test]
#[ignore = "requires installed pinned sidecars; loopback HTTP only"]
fn d2_real_recent_collision_is_not_done_and_never_overwrites() {
    let f = LocalDownload::new("Collision title");
    let path = PathBuf::from(f.run().unwrap());
    // Distinguishable original data: --no-overwrites must leave it byte-for-byte intact.
    fs::write(&path, b"keep the existing file").unwrap();
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::now())
        .unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let result = f.run();
    assert_eq!(fs::read(&path).unwrap(), b"keep the existing file");
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
    // N2: keep both. The second download lands as " (2)"; the existing file is untouched.
    let second = PathBuf::from(result.unwrap());
    let expected = path.with_file_name(download::numbered(path.file_name().unwrap(), 2));
    assert_eq!(second, expected);
    assert_eq!(fs::read(&second).unwrap(), silence_wav());
}

#[test]
#[ignore = "requires installed pinned sidecars; loopback HTTP only"]
fn d2_real_directory_collision_cannot_be_done() {
    let f = LocalDownload::new("Directory title");
    let path = PathBuf::from(f.run().unwrap());
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    let result = f.run();
    assert!(path.is_dir());
    // A folder in the way is never replaced; the download takes the next free name.
    let expected = path.with_file_name(download::numbered(path.file_name().unwrap(), 2));
    assert_eq!(PathBuf::from(result.unwrap()), expected);
}

/// N2 / #16: test numbering gaps after upstream sanitizes reserved and messy titles.
#[test]
#[ignore = "requires installed pinned sidecars; loopback HTTP only"]
fn n2_real_sanitized_titles_take_first_gap_and_preserve_all_existing_bytes() {
    for title in [
        "CON",
        "NUL",
        "COM1",
        "LPT9",
        " 日本語 🎵 é : * ? &lt;noise&gt; ",
    ] {
        let f = LocalDownload::new(title);
        let first = PathBuf::from(f.run().unwrap());
        fs::write(&first, b"original").unwrap();
        let name = first.file_name().unwrap();
        // Independent expected suffix construction, rather than calling numbered().
        let text = name.to_str().unwrap();
        let dot = text.rfind('.').unwrap();
        let nth = |n| {
            f.out
                .join(format!("{} ({n}){}", &text[..dot], &text[dot..]))
        };
        fs::write(nth(2), b"occupied second").unwrap();
        fs::write(nth(4), b"occupied fourth").unwrap();
        let third = PathBuf::from(f.run().unwrap());
        assert_eq!(third, nth(3));
        assert_eq!(fs::read(&third).unwrap(), silence_wav());
        assert_eq!(fs::read(&first).unwrap(), b"original");
        assert_eq!(fs::read(nth(2)).unwrap(), b"occupied second");
        assert_eq!(fs::read(nth(4)).unwrap(), b"occupied fourth");
        assert_eq!(fs::read_dir(&f.out).unwrap().count(), 4);
    }
}

/// N2 / #16: a title accepted by real yt-dlp must also support a second download.
#[test]
#[ignore = "requires installed pinned sidecars; loopback HTTP only"]
fn n2_real_long_title_collision_still_saves_a_numbered_copy() {
    let f = LocalDownload::new(&"a".repeat(250));
    let first = PathBuf::from(f.run().unwrap());
    fs::write(&first, b"original long title").unwrap();
    let result = f.run();
    assert_eq!(fs::read(&first).unwrap(), b"original long title");
    let second = PathBuf::from(result.unwrap_or_else(|reason| {
        panic!(
            "N2: a valid first filename {:?} must permit a numbered second copy: {reason}",
            first.file_name().unwrap()
        )
    }));
    assert_ne!(first, second);
    assert!(
        second
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with(" (2).wav")
    );
    assert_eq!(fs::read(second).unwrap(), silence_wav());
}

/// #48 / D-017: exercise the new saved-info source with upstream yt-dlp, including its
/// Windows filename sanitization. Loopback only; no external media or user output folder.
#[test]
#[ignore = "requires installed pinned sidecars; loopback HTTP only"]
fn d017_real_saved_info_sanitizes_titles_and_never_overwrites() {
    for title in ["CON", "NUL", " 日本語 🎵 é : * ? &lt;noise&gt; "] {
        let f = LocalDownload::new(title);
        let (_, raw) = queue_lib::meta::fetch_raw(&f.bin, &f.url, Duration::from_secs(30)).unwrap();
        let info = f.root.join("saved info 日本語.json");
        fs::write(&info, raw).unwrap();
        let run = |suffix: &str| {
            let mut retries = Vec::new();
            let result = download::run_with_retries(
                &f.bin,
                &f.root.join(suffix),
                &f.out,
                &f.url,
                Some(&info),
                |_| panic!("loopback success/collision must not back off"),
                |attempt| retries.push(attempt),
            );
            assert!(retries.is_empty());
            assert!(!f.root.join(suffix).exists());
            result
        };
        let path = PathBuf::from(run("first").unwrap());
        assert_eq!(fs::read(&path).unwrap(), silence_wav());
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(!name.contains(['<', '>', ':', '"', '/', '\\', '|', '?', '*']));
        assert!(!["CON", "NUL"].contains(&name.split('.').next().unwrap().to_uppercase().as_str()));
        if title.contains("日本語") {
            assert!(name.contains("日本語"));
        }
        fs::write(&path, b"original bytes").unwrap();
        let second = PathBuf::from(run("collision").unwrap()); // N2: keep both
        assert_ne!(second, path);
        assert_eq!(fs::read(&second).unwrap(), silence_wav());
        assert_eq!(fs::read(path).unwrap(), b"original bytes");
    }
}

/// Opt-in network check: a real YouTube DASH download (video + audio merged by our ffmpeg)
/// through the staged flow. Set QUEUE_QA_NETWORK=1 as well as QUEUE_QA_REAL_SIDECARS.
#[test]
#[ignore = "requires pinned sidecars and network (QUEUE_QA_NETWORK=1)"]
fn d2_real_youtube_merge_lands_in_out_dir_and_leaves_no_temp_files() {
    if std::env::var_os("QUEUE_QA_NETWORK").is_none() {
        return;
    }
    let mut fixture = LocalDownload::new("unused");
    fixture.url = "https://www.youtube.com/watch?v=jNQXAC9IVRw".into();
    let path = fixture.run().unwrap();
    assert!(
        path.ends_with("Me at the zoo.webm") || path.ends_with("Me at the zoo.mp4"),
        "{path}"
    );
    let names: Vec<_> = fs::read_dir(&fixture.out)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        names.len(),
        1,
        "only the finished file, no .part/.temp/.fNNN: {names:?}"
    );
}

/// #11 / D-015 with the real site: "Most compatible video" on a YouTube video that has H.264
/// gives H.264 + AAC in .mp4, with no notice; the codecs come from yt-dlp's own report.
/// Set QUEUE_QA_NETWORK=1 as well as QUEUE_QA_REAL_SIDECARS.
#[test]
#[ignore = "requires pinned sidecars and network (QUEUE_QA_NETWORK=1)"]
fn d11_real_youtube_compatible_video_is_h264_aac_mp4() {
    if std::env::var_os("QUEUE_QA_NETWORK").is_none() {
        return;
    }
    let f = LocalDownload::new("unused");
    let work = f.root.join("work-yt-compatible");
    let saved = download::run_with_retries_saved(
        &f.bin,
        &work,
        &f.out,
        "https://www.youtube.com/watch?v=jNQXAC9IVRw",
        None,
        download::Format::CompatibleVideo,
        std::thread::sleep,
        |_| {},
        |_| {},
    )
    .unwrap();
    let path = PathBuf::from(&saved.path);
    assert_eq!(path.extension().unwrap(), "mp4", "{saved:?}");
    assert!(
        saved
            .codecs
            .video
            .as_deref()
            .unwrap_or("")
            .starts_with("avc1"),
        "{saved:?}"
    );
    assert!(
        saved
            .codecs
            .audio
            .as_deref()
            .unwrap_or("")
            .starts_with("mp4a"),
        "{saved:?}"
    );
    assert_eq!(
        download::Format::CompatibleVideo.notice_for(&path, &saved.codecs),
        None
    );
    assert!(!work.exists());
}
