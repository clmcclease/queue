//! #48 / D-017: 403 retries and reuse of the saved `-J` info, through the real subprocess path
//! with a fake yt-dlp (Unix shell script). Waits are recorded, not slept.
#![cfg(unix)]
use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use queue_lib::{download, sidecar::Sidecar};

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Prepended to every script: `$P` is the `-P` work dir; `$N` counts attempts (1, 2, 3).
const PRELUDE: &str = r#"P=; prev=; for a in "$@"; do [ "$prev" = -P ] && P=$a; prev=$a; done
N=$(( $(cat "$COUNT" 2>/dev/null || echo 0) + 1 )); echo $N > "$COUNT"
printf '%s\n' "$@" > "$LOGDIR/args-$N""#;

struct Fake {
    root: PathBuf,
    out: PathBuf,
}

impl Fake {
    fn new(body: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "queue-retry-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let out = root.join("out");
        fs::create_dir_all(&out).unwrap();
        let script = format!(
            "#!/bin/sh\nCOUNT='{c}'\nLOGDIR='{l}'\n{PRELUDE}\n{body}\n",
            c = root.join("count").display(),
            l = root.display()
        );
        let path = Sidecar::YtDlp.path_in(&root);
        fs::write(&path, script).unwrap();
        fs::set_permissions(path, Permissions::from_mode(0o700)).unwrap();
        Self { root, out }
    }

    /// Runs with retries; returns (result, waits, retry attempts reported).
    fn run(&self, info: Option<PathBuf>) -> (Result<String, String>, Vec<Duration>, Vec<u32>) {
        let (mut waits, mut retries) = (Vec::new(), Vec::new());
        let work = self.root.join("work");
        let result = download::run_with_retries(
            &self.root,
            &work,
            &self.out,
            "https://example.test/v",
            info.as_deref(),
            |d| waits.push(d),
            |n| retries.push(n),
        );
        assert!(!work.exists(), "work root removed after all attempts");
        (result, waits, retries)
    }

    fn args(&self, attempt: u32) -> Vec<String> {
        fs::read_to_string(self.root.join(format!("args-{attempt}")))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

const FORBIDDEN: &str =
    "echo 'ERROR: unable to download video data: HTTP Error 403: Forbidden' >&2; exit 1";

#[test]
fn a_403_is_retried_with_backoff_and_then_succeeds() {
    let f = Fake::new(&format!(
        "if [ $N -eq 1 ]; then {FORBIDDEN}; fi\nprintf media > \"$P/Title.webm\""
    ));
    let (result, waits, retries) = f.run(None);
    assert!(result.unwrap().ends_with("Title.webm"));
    assert_eq!(waits, [Duration::from_secs(10)]);
    assert_eq!(retries, [2]);
}

#[test]
fn persistent_403s_give_up_after_three_attempts_with_the_reason() {
    let f = Fake::new(FORBIDDEN);
    let (result, waits, retries) = f.run(None);
    assert!(result.unwrap_err().contains("HTTP Error 403"));
    assert_eq!(waits, [Duration::from_secs(10), Duration::from_secs(30)]);
    assert_eq!(retries, [2, 3]);
    assert!(!f.root.join("args-4").exists(), "exactly three attempts");
}

#[test]
fn other_errors_are_not_retried() {
    let f = Fake::new("echo 'ERROR: [youtube] x: Video unavailable' >&2; exit 1");
    let (result, waits, retries) = f.run(None);
    assert_eq!(result.unwrap_err(), "[youtube] x: Video unavailable");
    assert!(waits.is_empty() && retries.is_empty());
}

#[test]
fn saved_info_is_used_first_and_a_retry_re_extracts_from_the_url() {
    let f = Fake::new(&format!(
        "if [ $N -eq 1 ]; then {FORBIDDEN}; fi\nprintf media > \"$P/Title.webm\""
    ));
    let info = f.root.join("item-1.json");
    fs::write(&info, "{}").unwrap();
    let (result, _, _) = f.run(Some(info.clone()));
    assert!(result.is_ok());
    let first = f.args(1);
    let at = first.iter().position(|a| a == "--load-info-json").unwrap();
    assert_eq!(first[at + 1], info.to_str().unwrap());
    assert!(
        !first
            .iter()
            .any(|a| a == "--" || a.contains("example.test"))
    );
    let second = f.args(2);
    assert_eq!(second[second.len() - 2..], ["--", "https://example.test/v"]);
    assert!(!second.iter().any(|a| a == "--load-info-json"));
}

#[test]
fn an_existing_target_is_not_retried_and_not_overwritten() {
    let f = Fake::new("printf new > \"$P/Title.webm\"");
    fs::write(f.out.join("Title.webm"), b"old").unwrap();
    let (result, waits, _) = f.run(None);
    assert!(result.unwrap_err().contains("already exists"));
    assert!(waits.is_empty());
    assert_eq!(fs::read(f.out.join("Title.webm")).unwrap(), b"old");
}

/// Opt-in: real sidecars + network. Fetch `-J`, save it, then download from the saved info
/// (`--load-info-json`, no second extraction). `QUEUE_QA_REAL_SIDECARS=src-tauri/binaries
/// QUEUE_QA_NETWORK=1 cargo test --test retry_d2 -- --ignored`
#[test]
#[ignore = "requires pinned sidecars and network"]
fn real_download_from_saved_info() {
    let (Some(src), Some(_)) = (
        std::env::var_os("QUEUE_QA_REAL_SIDECARS"),
        std::env::var_os("QUEUE_QA_NETWORK"),
    ) else {
        return;
    };
    let triple = if cfg!(target_arch = "aarch64") {
        "aarch64-apple-darwin"
    } else if cfg!(target_os = "macos") {
        "x86_64-apple-darwin"
    } else {
        "x86_64-unknown-linux-gnu"
    };
    let root = std::env::temp_dir().join(format!("queue-retry-real-{}", std::process::id()));
    let (bin, out) = (root.join("bin"), root.join("out"));
    fs::create_dir_all(&bin).unwrap();
    // queue-ffprobe has no Sidecar variant: yt-dlp finds it next to queue-ffmpeg by name.
    for name in [
        "queue-yt-dlp",
        "queue-ffmpeg",
        "queue-ffprobe",
        "queue-deno",
    ] {
        fs::copy(
            PathBuf::from(&src).join(format!("{name}-{triple}")),
            bin.join(name),
        )
        .unwrap();
    }
    let url = "https://www.youtube.com/watch?v=jNQXAC9IVRw";
    let (_, raw) = queue_lib::meta::fetch_raw(&bin, url, Duration::from_secs(90)).unwrap();
    let info = root.join("item-1.json");
    fs::write(&info, raw).unwrap();
    let result = download::run_with_retries(
        &bin,
        &root.join("work"),
        &out,
        url,
        Some(&info),
        std::thread::sleep,
        |_| {},
    );
    let _ = fs::remove_dir_all(&root);
    assert!(result.unwrap().contains("Me at the zoo"));
}
