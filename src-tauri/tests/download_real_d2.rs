//! #4 / D2: opt-in tests using the unmodified bundled yt-dlp, with loopback-only WAV fixtures.
//! Set QUEUE_QA_REAL_SIDECARS to src-tauri/binaries, then run:
//! cargo test --locked --test download_real_d2 -- --ignored --test-threads=1
//! These tests do not contact external sites or write into the user's Downloads folder.
use queue_lib::{download, sidecar::Sidecar};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct LocalDownload {
    root: PathBuf,
    bin: PathBuf,
    out: PathBuf,
    url: String,
    stop: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
}

impl LocalDownload {
    fn new(title: &str) -> Self {
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
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let html = format!("<!doctype html><html><head><title>{title}</title></head><body><audio controls src=\"/media.wav\"></audio></body></html>").into_bytes();
        let wav = silence_wav();
        let server = thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => respond(stream, &html, &wav),
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

fn respond(mut stream: TcpStream, html: &[u8], wav: &[u8]) {
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
    let media = first.contains(" /media.wav ");
    let body = if media { wav } else { html };
    let mime = if media {
        "audio/wav"
    } else {
        "text/html; charset=utf-8"
    };
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if stream.write_all(headers.as_bytes()).is_ok() && !first.starts_with("HEAD ") {
        let _ = stream.write_all(body);
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
    assert!(
        result.is_err(),
        "real yt-dlp skipped an existing file, but Queue reported Done: {result:?}"
    );
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
    assert!(
        result.is_err(),
        "real yt-dlp left a directory at the output path, but Queue reported Done: {result:?}"
    );
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
