//! #8 / D4: fake Unix sidecar tests exercise the real pipes; no shell is used by Queue.
#![cfg(unix)]
use queue_lib::{download, sidecar::Sidecar};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fake(PathBuf);
impl Fake {
    fn new(body: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "queue-d4-qa-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let path = Sidecar::YtDlp.path_in(&root);
        let script = format!(
            "#!/bin/sh\nROOT='{}'\nP=; prev=; for a in \"$@\"; do [ \"$prev\" = -P ] && P=$a; prev=$a; done\n{body}\n",
            root.display()
        );
        fs::write(&path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(root)
    }
    fn run(&self, callback: impl FnMut(download::Event)) -> Result<String, String> {
        let result = download::run_with_retries_events(
            &self.0,
            &self.0.join("work"),
            &self.0.join("out"),
            "https://example.test/v",
            None,
            |_| {},
            |_| {},
            callback,
        );
        assert!(
            !self.0.join("work").exists(),
            "staging cleaned after success or failure"
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
fn a_progress_callback_runs_before_the_sidecar_can_finish() {
    let f = Fake::new(
        r#"
echo 'QUEUE-PROGRESS {"status":"downloading","downloaded":1,"total":4,"speed":3,"eta":1}'
i=0
while [ ! -f "$ROOT/ack" ] && [ "$i" -lt 200 ]; do sleep 0.01; i=$((i + 1)); done
[ -f "$ROOT/ack" ] || { echo 'ERROR: progress was buffered until exit' >&2; exit 1; }
printf media > "$P/日本語 🎵.webm"
"#,
    );
    let mut events = Vec::new();
    let result = f.run(|event| {
        fs::write(f.0.join("ack"), b"received while child alive").unwrap();
        events.push(event);
    });
    assert!(result.unwrap().ends_with("日本語 🎵.webm"));
    assert_eq!(events.len(), 1);
}

#[test]
fn oversized_stdout_and_flooded_stderr_do_not_block_later_crlf_progress() {
    let f = Fake::new(
        r#"
awk 'BEGIN { for (i=0;i<70000;i++) printf "x"; printf "\n"; for (i=0;i<70000;i++) printf "y" > "/dev/stderr"; printf "\n" > "/dev/stderr" }'
printf 'QUEUE-PROGRESS not-json\nQUEUE-PROGRESS {"status":"downloading","downloaded":3,"total":4,"speed":5,"eta":1}\r\n'
echo 'QUEUE-POST {"status":"started","postprocessor":"ExtractAudio"}'
echo 'ERROR: extraction failed after progress' >&2
exit 1
"#,
    );
    let mut events = Vec::new();
    assert_eq!(
        f.run(|e| events.push(e)).unwrap_err(),
        "extraction failed after progress"
    );
    assert_eq!(events.len(), 2);
    assert!(matches!(&events[0], download::Event::Progress(p) if p.percent == Some(75.0)));
    assert_eq!(
        events[1],
        download::Event::Processing("ExtractAudio".into())
    );
}

#[test]
fn stream_numbers_restart_on_retry_and_collision_preserves_the_original() {
    let f = Fake::new(
        r#"
if [ ! -f "$ROOT/retried" ]; then
  echo 'QUEUE-PROGRESS {"status":"finished","downloaded":4,"total":4}'
  touch "$ROOT/retried"
  echo 'ERROR: HTTP Error 403: Forbidden' >&2
  exit 1
fi
echo 'QUEUE-PROGRESS {"status":"downloading","downloaded":1,"total":4}'
echo 'QUEUE-PROGRESS {"status":"finished","downloaded":4,"total":4}'
printf new > "$P/日本語.webm"
"#,
    );
    fs::create_dir_all(f.0.join("out")).unwrap();
    fs::write(f.0.join("out/日本語.webm"), b"original").unwrap();
    let mut parts = Vec::new();
    let path = f
        .run(|event| {
            if let download::Event::Progress(p) = event {
                parts.push(p.part);
            }
        })
        .unwrap();
    // The retry's 25% may be replaced by its 100% before it's taken (same part).
    assert!(
        parts.len() >= 2 && parts.iter().all(|&p| p == 1),
        "{parts:?}"
    );
    assert!(path.ends_with("日本語 (2).webm"));
    assert_eq!(fs::read(f.0.join("out/日本語.webm")).unwrap(), b"original");
    assert_eq!(fs::read(path).unwrap(), b"new");
}

/// Review finding (#8): yt-dlp exiting while a descendant keeps a pipe open must not hold the
/// download slot. The leftover process is stopped within the drain grace and the row fails.
fn assert_bounded_failure_with_orphan(hold: &str) {
    let f = Fake::new(&format!(
        "sleep 30 {hold} &\necho $! > \"$ROOT/pid\"\necho 'ERROR: boom' >&2\nexit 1"
    ));
    let started = std::time::Instant::now();
    let result = f.run(|_| {});
    assert!(
        started.elapsed() < std::time::Duration::from_secs(15),
        "bounded"
    );
    assert_eq!(result.unwrap_err(), "boom");
    let pid = fs::read_to_string(f.0.join("pid")).unwrap();
    let alive = std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(!alive, "the descendant holding the pipe was stopped");
}

#[test]
fn a_descendant_holding_only_stdout_cannot_stall_a_failed_download() {
    assert_bounded_failure_with_orphan("2>/dev/null");
}

#[test]
fn a_descendant_holding_only_stderr_cannot_stall_a_failed_download() {
    assert_bounded_failure_with_orphan(">/dev/null");
}

#[test]
fn a_success_with_a_lingering_descendant_still_saves_the_file() {
    let f = Fake::new("printf media > \"$P/done.webm\"\nsleep 30 &\nexit 0");
    let started = std::time::Instant::now();
    let saved = PathBuf::from(f.run(|_| {}).unwrap());
    assert!(started.elapsed() < std::time::Duration::from_secs(15));
    assert_eq!(fs::read(saved).unwrap(), b"media");
}

/// Review finding (#8): a progress flood with a slow consumer stays bounded, and Processing,
/// which comes after it, is never dropped.
#[test]
fn a_progress_flood_with_a_slow_consumer_is_bounded_and_keeps_processing() {
    let f = Fake::new(
        r#"
awk 'BEGIN { for (i=1;i<=20000;i++) printf "QUEUE-PROGRESS {\"status\":\"downloading\",\"downloaded\":%d,\"total\":20000}\n", i }'
echo 'QUEUE-POST {"status":"started","postprocessor":"Merger"}'
printf media > "$P/flood.webm"
"#,
    );
    let mut progress = 0;
    let mut last = None;
    let mut processing = Vec::new();
    let result = f.run(|event| match event {
        download::Event::Progress(p) => {
            assert!(processing.is_empty(), "progress stays before Processing");
            progress += 1;
            last = p.downloaded;
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        download::Event::Processing(name) => processing.push(name),
    });
    assert!(result.unwrap().ends_with("flood.webm"));
    assert!(
        progress < 20000,
        "a slow consumer sheds progress ({progress} kept)"
    );
    assert_eq!(processing, ["Merger"]);
    assert_eq!(
        last,
        Some(20000),
        "the newest progress survives backpressure"
    );
}

#[test]
fn the_final_progress_survives_backpressure_when_nothing_follows_it() {
    let f = Fake::new(
        r#"
awk 'BEGIN { for (i=1;i<=5000;i++) printf "QUEUE-PROGRESS {\"status\":\"downloading\",\"downloaded\":%d,\"total\":5000}\n", i }'
printf media > "$P/last.webm"
"#,
    );
    let mut last = None;
    let result = f.run(|event| {
        std::thread::sleep(std::time::Duration::from_millis(1));
        if let download::Event::Progress(p) = event {
            last = p.percent;
        }
    });
    assert!(result.unwrap().ends_with("last.webm"));
    assert_eq!(last, Some(100.0));
}

/// Review finding (#8, round 4): after a burst, the newest progress reaches the caller while
/// yt-dlp is quiet, not only when more output (or EOF) arrives.
#[test]
fn the_newest_progress_arrives_during_a_pause_after_a_burst() {
    let f = Fake::new(
        r#"
awk 'BEGIN { for (i=1;i<=5000;i++) printf "QUEUE-PROGRESS {\"status\":\"downloading\",\"downloaded\":%d,\"total\":5000}\n", i }'
i=0
while [ ! -f "$ROOT/seen" ] && [ "$i" -lt 500 ]; do sleep 0.01; i=$((i + 1)); done
[ -f "$ROOT/seen" ] || { echo 'ERROR: newest progress waited for more output' >&2; exit 1; }
printf media > "$P/pause.webm"
"#,
    );
    let result = f.run(|event| {
        if let download::Event::Progress(p) = event
            && p.percent == Some(100.0)
        {
            fs::write(f.0.join("seen"), b"").unwrap();
        }
    });
    assert!(result.unwrap().ends_with("pause.webm"));
}

/// Review finding (#8, round 5): an update held back by the 250 ms throttle still reaches the
/// caller when yt-dlp goes quiet right after it. 60% is only printed once 10% was delivered,
/// so it can't merge with 10% and must take the held path.
#[test]
fn a_throttled_update_is_delivered_during_a_pause() {
    let f = Fake::new(
        r#"
echo 'QUEUE-PROGRESS {"status":"downloading","downloaded":10,"total":100}'
i=0
while [ ! -f "$ROOT/first" ] && [ "$i" -lt 300 ]; do sleep 0.01; i=$((i + 1)); done
echo 'QUEUE-PROGRESS {"status":"downloading","downloaded":60,"total":100}'
i=0
while [ ! -f "$ROOT/seen" ] && [ "$i" -lt 300 ]; do sleep 0.01; i=$((i + 1)); done
[ -f "$ROOT/seen" ] || { echo 'ERROR: 60% waited for more output' >&2; exit 1; }
printf media > "$P/held.webm"
"#,
    );
    let mut seen = Vec::new();
    let result = f.run(|event| {
        if let download::Event::Progress(p) = event {
            seen.push((p.percent, std::time::Instant::now()));
            let ack = if p.percent == Some(10.0) {
                "first"
            } else {
                "seen"
            };
            fs::write(f.0.join(ack), b"").unwrap();
        }
    });
    assert!(result.unwrap().ends_with("held.webm"));
    let percents: Vec<_> = seen.iter().map(|(p, _)| *p).collect();
    assert_eq!(percents, [Some(10.0), Some(60.0)]);
    let gap = seen[1].1 - seen[0].1;
    assert!(
        gap >= download::PROGRESS_EVERY - std::time::Duration::from_millis(5),
        "60% was held for the throttle interval ({gap:?})"
    );
}
