//! #6 / Q3: `yt-dlp -J` metadata fetching through the real subprocess path, using a fake yt-dlp
//! (Unix shell script). The opt-in real-sidecar check is in download_real_d2.rs's style below.
#![cfg(unix)]
use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use queue_lib::{meta, sidecar::Sidecar};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fake {
    root: PathBuf,
}

impl Fake {
    fn new(body: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "queue-q3-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let path = Sidecar::YtDlp.path_in(&root);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, Permissions::from_mode(0o700)).unwrap();
        Self { root }
    }

    fn fetch(&self, limit: Duration) -> Result<meta::Meta, String> {
        meta::fetch(&self.root, "https://example.test/v", limit)
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn success_parses_stdout_json() {
    let f = Fake::new(r#"printf '%s' '{"title":"Hello","uploader":"Me","duration":61.5}'"#);
    let m = f.fetch(Duration::from_secs(10)).unwrap();
    assert_eq!(m.title.as_deref(), Some("Hello"));
    assert_eq!(m.channel.as_deref(), Some("Me"));
    assert_eq!(m.duration, Some(61.5));
}

#[test]
fn failure_returns_the_last_error_line() {
    let f = Fake::new("echo 'WARNING: x' >&2\necho 'ERROR: [generic] Unsupported URL' >&2\nexit 1");
    assert_eq!(
        f.fetch(Duration::from_secs(10)).unwrap_err(),
        "Unsupported URL"
    );
}

#[test]
fn a_hung_yt_dlp_is_killed_at_the_limit() {
    let f = Fake::new("sleep 30");
    let started = Instant::now();
    let err = f.fetch(Duration::from_millis(300)).unwrap_err();
    assert!(err.contains("took longer"), "{err}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "killed promptly"
    );
}

#[test]
fn timeout_kills_the_whole_process_group_including_descendants() {
    // Like the onefile launcher: yt-dlp starts a child that would outlive a plain kill.
    let pidfile = std::env::temp_dir().join(format!("queue-q3-pid-{}", std::process::id()));
    let f = Fake::new(&format!(
        "sleep 30 &\necho $! > '{}'\nwait",
        pidfile.display()
    ));
    assert!(
        f.fetch(Duration::from_millis(300))
            .unwrap_err()
            .contains("took longer")
    );
    let pid = fs::read_to_string(&pidfile).unwrap().trim().to_owned();
    let _ = fs::remove_file(&pidfile);
    std::thread::sleep(Duration::from_millis(200));
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid])
        .status()
        .unwrap()
        .success();
    assert!(!alive, "descendant {pid} must be killed with its group");
}

#[test]
fn large_output_on_both_pipes_does_not_deadlock() {
    // ~1.5 MB of stdout JSON (like YouTube's formats list) plus noisy stderr.
    let f = Fake::new(
        r#"i=0; while [ $i -lt 2000 ]; do echo "warning line $i" >&2; i=$((i+1)); done
printf '{"title":"Big","formats":['
i=0; while [ $i -lt 20000 ]; do printf '{"format_id":"%d","url":"https://example.test/media/%d"},' $i $i; i=$((i+1)); done
printf '{"format_id":"last"}]}'"#,
    );
    assert_eq!(
        f.fetch(Duration::from_secs(30)).unwrap().title.as_deref(),
        Some("Big")
    );
}

#[test]
fn runaway_stdout_is_stopped_with_a_readable_error_well_before_the_deadline() {
    // 40 MB of output, over the 32 MB cap; must not be buffered to the end or wait 60 s.
    let f = Fake::new("head -c 41943040 /dev/zero | tr '\\0' 'x'\nsleep 60");
    let started = Instant::now();
    let err = f.fetch(Duration::from_secs(60)).unwrap_err();
    assert!(err.contains("too much output"), "{err}");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn overflow_from_a_descendant_after_the_parent_exits_is_stopped_promptly() {
    // The parent exits at once; a background descendant floods stdout past the cap, then hangs
    // holding stderr. Expect the overflow reason (not a timeout), quickly, and no survivor.
    let pidfile = std::env::temp_dir().join(format!("queue-q3-ovf-{}", std::process::id()));
    let f = Fake::new(&format!(
        "( echo $$ > /dev/null; sh -c 'echo $$ > \"{pid}\"; head -c 41943040 /dev/zero | tr \"\\\\0\" x; sleep 60' ) &\nexit 0",
        pid = pidfile.display()
    ));
    let started = Instant::now();
    let err = f.fetch(Duration::from_secs(60)).unwrap_err();
    assert!(err.contains("too much output"), "{err}");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    std::thread::sleep(Duration::from_millis(300));
    let pid = fs::read_to_string(&pidfile)
        .unwrap_or_default()
        .trim()
        .to_owned();
    let _ = fs::remove_file(&pidfile);
    assert!(!pid.is_empty(), "descendant wrote its pid");
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid])
        .status()
        .unwrap()
        .success();
    assert!(!alive, "descendant {pid} survived");
}

#[test]
fn huge_stderr_keeps_the_final_error_line() {
    let f = Fake::new(
        "i=0; while [ $i -lt 20000 ]; do echo \"WARNING: noise line $i padding padding padding\" >&2; i=$((i+1)); done\necho 'ERROR: the real reason' >&2\nexit 1",
    );
    assert_eq!(
        f.fetch(Duration::from_secs(30)).unwrap_err(),
        "the real reason"
    );
}

#[test]
fn a_bad_fetch_does_not_affect_the_next_one() {
    let bad = Fake::new("sleep 30");
    assert!(bad.fetch(Duration::from_millis(200)).is_err());
    let good = Fake::new(r#"printf '%s' '{"title":"Next"}'"#);
    assert_eq!(
        good.fetch(Duration::from_secs(10))
            .unwrap()
            .title
            .as_deref(),
        Some("Next")
    );
}

#[test]
fn playlist_json_is_refused() {
    let f = Fake::new(r#"printf '%s' '{"_type":"playlist","title":"Mix","entries":[]}'"#);
    assert!(
        f.fetch(Duration::from_secs(10))
            .unwrap_err()
            .contains("Playlists")
    );
}

/// Opt-in: real sidecars + network. `QUEUE_QA_REAL_SIDECARS=src-tauri/binaries
/// QUEUE_QA_NETWORK=1 cargo test --test meta_q3 -- --ignored`
#[test]
#[ignore = "requires pinned sidecars and network"]
fn real_youtube_metadata() {
    let (Some(src), Some(_)) = (
        std::env::var_os("QUEUE_QA_REAL_SIDECARS"),
        std::env::var_os("QUEUE_QA_NETWORK"),
    ) else {
        return;
    };
    let src = PathBuf::from(src);
    let triple = if cfg!(target_arch = "aarch64") {
        "aarch64-apple-darwin"
    } else if cfg!(target_os = "macos") {
        "x86_64-apple-darwin"
    } else {
        "x86_64-unknown-linux-gnu"
    };
    let bin = std::env::temp_dir().join(format!("queue-q3-real-{}", std::process::id()));
    fs::create_dir_all(&bin).unwrap();
    for s in [Sidecar::YtDlp, Sidecar::Ffmpeg, Sidecar::Deno] {
        fs::copy(
            src.join(format!("{}-{triple}", s.file_name())),
            bin.join(s.file_name()),
        )
        .unwrap();
    }
    let m = meta::fetch(
        &bin,
        "https://www.youtube.com/watch?v=jNQXAC9IVRw",
        Duration::from_secs(90),
    );
    let _ = fs::remove_dir_all(&bin);
    let m = m.unwrap();
    assert_eq!(m.title.as_deref(), Some("Me at the zoo"));
    assert_eq!(m.channel.as_deref(), Some("jawed"));
    assert_eq!(m.duration.map(f64::round), Some(19.0));
    assert!(m.thumbnail.unwrap().starts_with("https://"));
    assert_eq!(m.site.as_deref(), Some("Youtube"));
}

/// Opt-in: a real onefile yt-dlp stopped mid-extraction must leave no processes and no
/// `_MEI*` unpack dir (SIGTERM first lets the launcher clean up; see proc.rs).
#[test]
#[ignore = "requires pinned sidecars and network"]
fn real_timeout_leaves_no_processes_or_unpack_dirs() {
    let (Some(src), Some(_)) = (
        std::env::var_os("QUEUE_QA_REAL_SIDECARS"),
        std::env::var_os("QUEUE_QA_NETWORK"),
    ) else {
        return;
    };
    let mei = || -> usize {
        fs::read_dir(std::env::temp_dir())
            .map(|d| {
                d.flatten()
                    .filter(|e| e.file_name().to_string_lossy().starts_with("_MEI"))
                    .count()
            })
            .unwrap_or(0)
    };
    let before = mei();
    let src = PathBuf::from(src);
    let triple = if cfg!(target_arch = "aarch64") {
        "aarch64-apple-darwin"
    } else if cfg!(target_os = "macos") {
        "x86_64-apple-darwin"
    } else {
        "x86_64-unknown-linux-gnu"
    };
    let bin = std::env::temp_dir().join(format!("queue-q3-kill-{}", std::process::id()));
    fs::create_dir_all(&bin).unwrap();
    for s in [Sidecar::YtDlp, Sidecar::Ffmpeg, Sidecar::Deno] {
        fs::copy(
            src.join(format!("{}-{triple}", s.file_name())),
            bin.join(s.file_name()),
        )
        .unwrap();
    }
    let err = meta::fetch(
        &bin,
        "https://www.youtube.com/watch?v=jNQXAC9IVRw",
        Duration::from_millis(1500),
    )
    .unwrap_err();
    assert!(err.contains("took longer"), "{err}");
    std::thread::sleep(Duration::from_millis(500));
    let running = std::process::Command::new("pgrep")
        .args(["-f", &bin.to_string_lossy()])
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&bin);
    assert!(
        running.stdout.is_empty(),
        "leftover processes: {:?}",
        String::from_utf8_lossy(&running.stdout)
    );
    assert_eq!(mei(), before, "no leaked _MEI unpack dir");
}
