//! #41 / PRD §7 cross-platform NFR: real download/metadata subprocess paths on every OS.
//! A tiny Rust executable replaces the Unix shell fixtures, including on Windows.
use queue_lib::{download, meta, sidecar::Sidecar};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
static EXECUTABLE: OnceLock<PathBuf> = OnceLock::new();

fn executable() -> &'static PathBuf {
    EXECUTABLE.get_or_init(|| {
        let target = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/qa-native-41");
        fs::create_dir_all(&target).unwrap();
        let output = target.join(format!(
            "fixture-{}{}",
            std::process::id(),
            std::env::consts::EXE_SUFFIX
        ));
        let result = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args(["--edition=2024", "--crate-name", "queue_native_fixture"])
            .arg(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/support/native_sidecar_41.rs"),
            )
            .arg("-o")
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "fixture compilation: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        output
    })
}

struct Fake(PathBuf);
impl Fake {
    fn new(scenario: &str, name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "queue-native-41 日本語 {}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("out 日本語")).unwrap();
        fs::copy(executable(), Sidecar::YtDlp.path_in(&root)).unwrap();
        fs::write(root.join("scenario.txt"), scenario).unwrap();
        fs::write(root.join("name.txt"), name).unwrap();
        Self(root)
    }
    fn out(&self) -> PathBuf {
        self.0.join("out 日本語")
    }
    fn args(&self, n: u32) -> Vec<String> {
        fs::read_to_string(self.0.join(format!("args-{n}.txt")))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }
    fn run(
        &self,
        url: &str,
        info: Option<&std::path::Path>,
        mut event: impl FnMut(download::Event),
    ) -> Result<String, String> {
        let result = download::run_with_retries_events(
            &self.0,
            &self.0.join("work 日本語"),
            &self.out(),
            url,
            info,
            |_| {},
            |_| {},
            &mut event,
        );
        assert!(
            !self.0.join("work 日本語").exists(),
            "staging cleaned on success and failure"
        );
        result
    }
}
impl Drop for Fake {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn native_unicode_argv_and_collision_gaps_preserve_every_existing_file() {
    let name = "日本語 🎵 é & $() ;.webm";
    let f = Fake::new("success", name);
    fs::write(f.out().join(name), b"original").unwrap();
    fs::write(f.out().join("日本語 🎵 é & $() ; (2).webm"), b"second").unwrap();
    fs::write(f.out().join("日本語 🎵 é & $() ; (4).webm"), b"fourth").unwrap();
    let url = "https://example.test/日本語?x=$(echo%20oops)&y=;|\"&z=%20";
    let path = PathBuf::from(f.run(url, None, |_| {}).unwrap());
    assert_eq!(path, f.out().join("日本語 🎵 é & $() ; (3).webm"));
    assert_eq!(fs::read(path).unwrap(), b"fixture media");
    assert_eq!(fs::read(f.out().join(name)).unwrap(), b"original");
    assert_eq!(
        fs::read(f.out().join("日本語 🎵 é & $() ; (2).webm")).unwrap(),
        b"second"
    );
    assert_eq!(
        fs::read(f.out().join("日本語 🎵 é & $() ; (4).webm")).unwrap(),
        b"fourth"
    );
    let args = f.args(1);
    assert_eq!(&args[args.len() - 2..], ["--", url]);
    assert_eq!(
        args[args.iter().position(|a| a == "-P").unwrap() + 1],
        f.0.join("work 日本語").to_str().unwrap()
    );
    assert_eq!(
        args[args.iter().position(|a| a == "-o").unwrap() + 1],
        "%(title)s.%(ext)s"
    );
    assert!(
        !args
            .iter()
            .any(|a| ["-f", "--recode-video", "--merge-output-format"].contains(&a.as_str()))
    );
}

#[test]
fn native_malformed_and_oversized_pipes_keep_valid_crlf_progress_and_error() {
    let f = Fake::new("malformed", "unused.webm");
    let mut events = Vec::new();
    assert_eq!(
        f.run("https://example.test/v", None, |e| events.push(e))
            .unwrap_err(),
        "extraction failed 日本語"
    );
    assert_eq!(events.len(), 2);
    assert!(
        matches!(&events[0], download::Event::Progress(p) if p.percent == Some(75.0) && p.part == 1 && p.speed == Some(5.0) && p.eta == Some(1))
    );
    assert_eq!(
        events[1],
        download::Event::Processing("ExtractAudio".into())
    );
    assert_eq!(fs::read_dir(f.out()).unwrap().count(), 0);
}

#[test]
fn native_403_reextracts_after_unicode_saved_info_and_reuses_staging() {
    let f = Fake::new("retry", "日本語.webm");
    let info = f.0.join("saved 日本語 & ;.json");
    fs::write(&info, b"{}").unwrap();
    let url = "https://example.test/日本語?x=&y=;";
    let mut waits = Vec::new();
    let mut attempts = Vec::new();
    let path = download::run_with_retries_events(
        &f.0,
        &f.0.join("work"),
        &f.out(),
        url,
        Some(&info),
        |d| waits.push(d),
        |n| attempts.push(n),
        |_| {},
    )
    .unwrap();
    assert_eq!(fs::read(path).unwrap(), b"fixture media");
    assert_eq!(waits, [Duration::from_secs(10)]);
    assert_eq!(attempts, [2]);
    let first = f.args(1);
    assert_eq!(
        first[first.iter().position(|a| a == "--load-info-json").unwrap() + 1],
        info.to_str().unwrap()
    );
    assert!(!first.iter().any(|a| a == url));
    let second = f.args(2);
    assert!(!second.iter().any(|a| a == "--load-info-json"));
    assert_eq!(&second[second.len() - 2..], ["--", url]);
    assert!(!f.0.join("work").exists());
}

#[test]
fn native_metadata_timeout_stops_a_running_executable_promptly() {
    let f = Fake::new("timeout", "unused");
    let started = Instant::now();
    let reason =
        meta::fetch(&f.0, "https://example.test/v", Duration::from_millis(300)).unwrap_err();
    assert!(reason.contains("took longer"), "{reason}");
    assert!(
        f.0.join("started").exists(),
        "fixture actually ran before timing out"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn native_metadata_timeout_covers_descendants_holding_the_output_pipe() {
    let f = Fake::new("orphan", "unused");
    let started = Instant::now();
    let reason =
        meta::fetch(&f.0, "https://example.test/v", Duration::from_millis(500)).unwrap_err();
    assert!(reason.contains("took longer"), "{reason}");
    assert_eq!(f.args(1).last().unwrap(), "https://example.test/v");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "descendant held output beyond the deadline"
    );
}

// Linux refuses to exec a binary with an open writer; macOS doesn't. These deterministically
// cover #41's ETXTBSY retry rather than relying on a probabilistic high-core-count flake.
#[cfg(target_os = "linux")]
#[test]
fn linux_transient_text_file_busy_is_retried_until_the_writer_closes() {
    let f = Fake::new("success", "日本語.webm");
    let binary = Sidecar::YtDlp.path_in(&f.0);
    let writer = fs::OpenOptions::new().write(true).open(&binary).unwrap();
    let error = Command::new(&binary).spawn().unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::ETXTBSY));
    let closing = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(80));
        drop(writer);
    });
    let result = f.run("https://example.test/v", None, |_| {});
    closing.join().unwrap();
    assert_eq!(fs::read(result.unwrap()).unwrap(), b"fixture media");
}

#[cfg(target_os = "linux")]
#[test]
fn linux_persistent_text_file_busy_returns_a_bounded_readable_error() {
    let f = Fake::new("success", "unused.webm");
    let _writer = fs::OpenOptions::new()
        .write(true)
        .open(Sidecar::YtDlp.path_in(&f.0))
        .unwrap();
    let started = Instant::now();
    let error = f.run("https://example.test/v", None, |_| {}).unwrap_err();
    assert!(error.starts_with("Couldn't start yt-dlp:"), "{error}");
    assert!(error.contains("Text file busy"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(3));
}
