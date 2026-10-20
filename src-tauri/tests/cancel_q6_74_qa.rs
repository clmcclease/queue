//! Independent QA for #74 / Q6, D-027: cancellation at pipe boundaries, restart, and state.
#[path = "support/native.rs"]
mod native;

use queue_lib::{
    download,
    meta::Meta,
    proc,
    queue::{self, Queue, Status},
    sidecar::Sidecar,
};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

fn pipe_fixture() -> &'static PathBuf {
    static EXE: OnceLock<PathBuf> = OnceLock::new();
    EXE.get_or_init(|| {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let exe = manifest.join(format!(
            "target/cancel-pipes-74-{}{}",
            std::process::id(),
            std::env::consts::EXE_SUFFIX
        ));
        let output = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args(["--edition=2024", "--crate-name", "cancel_pipes_74"])
            .arg(manifest.join("tests/support/quit_fixture_67_qa.rs"))
            .arg("-o")
            .arg(&exe)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        exe
    })
}

struct Fixture(PathBuf);
impl Fixture {
    fn new(scenario: &str, pipes: bool) -> Self {
        let root = std::env::temp_dir().join(format!(
            "queue-74-qa 日本語 {scenario}-{}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("out")).unwrap();
        fs::copy(
            if pipes {
                pipe_fixture()
            } else {
                native::executable()
            },
            Sidecar::YtDlp.path_in(&root),
        )
        .unwrap();
        fs::write(
            root.join(if pipes { "scenario" } else { "scenario.txt" }),
            scenario,
        )
        .unwrap();
        Self(root)
    }
    fn wait_file(&self, name: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(text) = fs::read_to_string(self.0.join(name))
                && !text.is_empty()
            {
                return text;
            }
            assert!(Instant::now() < deadline, "{name} did not appear");
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn assert_exited(&self, name: &str) {
        let pid = self.wait_file(name).parse().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !native::process_has_exited(pid) {
            assert!(Instant::now() < deadline, "{name} survived cancellation");
            thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn cancel_at_pipe_boundary(scenario: &str) {
    let fixture = Fixture::new(scenario, true);
    let flag = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let root = fixture.0.clone();
    let cancel = flag.clone();
    let worker = thread::spawn(move || {
        let outcome = proc::with_cancel(cancel, || {
            download::run(
                &root,
                &root.join("work"),
                &root.join("out"),
                "https://example.test/v",
            )
        });
        tx.send(outcome).unwrap();
    });
    if scenario == "closed-stdout" || scenario == "ignore-term" {
        fixture.wait_file("ready");
    } else {
        fixture.wait_file("descendant.pid");
        fixture.assert_exited("leader.pid");
    }
    // Let the reader observe EOF, so the cancel hits the wait/drain rather than startup.
    thread::sleep(Duration::from_millis(100));
    flag.store(true, Ordering::SeqCst);
    let result = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("cancel must finish within five seconds");
    worker.join().unwrap();
    assert_eq!(result.unwrap_err(), download::CANCELLED);
    fixture.assert_exited("leader.pid");
    if scenario != "closed-stdout" {
        fixture.assert_exited("descendant.pid");
    }
    assert!(!fixture.0.join("work").exists());
    assert_eq!(fs::read_dir(fixture.0.join("out")).unwrap().count(), 0);
    assert!(!proc::stopping());
}

#[test]
fn cancel_after_stdout_eof_stops_a_still_running_leader() {
    cancel_at_pipe_boundary("closed-stdout");
}

#[test]
fn cancel_draining_an_exited_leaders_stdout_stops_its_descendant() {
    cancel_at_pipe_boundary("orphan-stdout");
}

#[test]
fn cancel_draining_an_exited_leaders_stderr_stops_its_descendant() {
    cancel_at_pipe_boundary("orphan-stderr");
}

#[test]
fn cancel_escalates_when_a_download_ignores_sigterm() {
    cancel_at_pipe_boundary("ignore-term");
}

#[test]
fn cancel_cleanup_preserves_existing_files_and_a_later_unicode_download_uses_a_free_name() {
    let fixture = Fixture::new("hang", false);
    let name = "CON 日本語 e\u{301} 🎵 [OFFICIAL VIDEO].webm";
    let original = fixture.0.join("out").join(name);
    fs::write(&original, b"existing media").unwrap();
    fs::write(fixture.0.join("name.txt"), name).unwrap();
    let flag = Arc::new(AtomicBool::new(false));
    let cancel = flag.clone();
    let root = fixture.0.clone();
    let worker = thread::spawn(move || {
        proc::with_cancel(cancel, || {
            download::run(
                &root,
                &root.join("work"),
                &root.join("out"),
                "https://example.test/v",
            )
        })
    });
    fixture.wait_file("descendant.pid");
    flag.store(true, Ordering::SeqCst);
    assert_eq!(worker.join().unwrap().unwrap_err(), download::CANCELLED);
    fixture.assert_exited("leader.pid");
    fixture.assert_exited("descendant.pid");
    assert!(!fixture.0.join("work").exists());
    assert_eq!(fs::read(&original).unwrap(), b"existing media");
    assert_eq!(fs::read_dir(fixture.0.join("out")).unwrap().count(), 1);
    fs::write(fixture.0.join("scenario.txt"), "success").unwrap();
    let saved = proc::with_cancel(Arc::new(AtomicBool::new(false)), || {
        download::run(
            &fixture.0,
            &fixture.0.join("work"),
            &fixture.0.join("out"),
            "https://example.test/v",
        )
    })
    .unwrap();
    assert_eq!(
        PathBuf::from(&saved).file_name().unwrap(),
        "CON 日本語 e\u{301} 🎵 [OFFICIAL VIDEO] (2).webm"
    );
    assert_eq!(fs::read(saved).unwrap(), b"fixture media");
    assert_eq!(fs::read(original).unwrap(), b"existing media");
}

fn ready(q: &mut Queue, suffix: &str) -> u64 {
    let id = q.add(&format!("https://example.test/{suffix}")).unwrap().id;
    q.fetched(
        id,
        Ok(Meta {
            title: Some("  CON 日本語 🎵 <script>  ".into()),
            ..Meta::default()
        }),
    )
    .unwrap();
    id
}

#[test]
fn cancelled_rows_preserve_metadata_and_notice_across_restart_and_can_be_removed() {
    let mut q = Queue::default();
    let id = ready(&mut q, "persist");
    let meta = q.items()[0].meta.clone();
    q.start(id).unwrap();
    q.retrying(id, 3).unwrap();
    let before = q.changes();
    let cancelled = q.cancelled(id).unwrap();
    assert_eq!(cancelled.status, Status::Queued);
    assert_eq!(cancelled.meta, meta);
    assert_eq!(cancelled.notice.as_deref(), Some(queue::CANCELLED));
    assert_eq!((cancelled.attempt, cancelled.progress), (0, None));
    assert_eq!(q.changes(), before + 1);
    assert!(
        q.add("https://example.test/persist").is_err(),
        "cancelled rows remain active duplicates"
    );
    let mut restored = Queue::from_json(&q.to_json()).unwrap();
    assert_eq!(
        restored.items()[0].notice.as_deref(),
        Some(queue::CANCELLED)
    );
    assert_eq!(restored.items()[0].meta, meta);
    restored.remove(id).unwrap();
    assert!(restored.items().is_empty());
}

#[test]
fn a_late_cancel_cannot_change_a_finished_failed_or_removed_row() {
    let mut q = Queue::default();
    let done = ready(&mut q, "done");
    q.start(done).unwrap();
    q.finish(done, Ok("日本語.webm".into())).unwrap();
    let failed = ready(&mut q, "failed");
    q.start(failed).unwrap();
    q.finish(failed, Err("unavailable".into())).unwrap();
    let removed = ready(&mut q, "removed");
    q.remove(removed).unwrap();
    let snapshot = q.to_json();
    let changes = q.changes();
    for id in [done, failed, removed, u64::MAX] {
        assert!(q.cancelled(id).is_none());
    }
    assert_eq!(q.to_json(), snapshot);
    assert_eq!(q.changes(), changes);
}

#[test]
fn download_all_reincludes_previously_cancelled_rows_on_a_later_run() {
    let mut q = Queue::default();
    let first = ready(&mut q, "first");
    let second = ready(&mut q, "second");
    let q = Mutex::new(q);
    let mut visited = Vec::new();
    queue::run_all(
        &q,
        |id| {
            assert!(visited.len() < 3, "cancelled row restarted in the same run");
            visited.push(id);
            let mut q = q.lock().unwrap();
            q.start(id).unwrap();
            q.cancelled(id).unwrap();
            true
        },
        || panic!("nothing needs waiting"),
    );
    assert_eq!(visited, [first, second]);
    visited.clear();
    queue::run_all(
        &q,
        |id| {
            visited.push(id);
            let mut q = q.lock().unwrap();
            assert_eq!(q.start(id).unwrap().notice, None);
            q.finish(id, Ok(format!("{id}.webm"))).unwrap();
            true
        },
        || panic!("nothing needs waiting"),
    );
    assert_eq!(visited, [first, second]);
}

#[test]
fn cancel_interrupts_an_empty_pipe_wait_and_is_reset_for_the_next_wait() {
    let (_tx, rx) = mpsc::channel::<()>();
    let flag = Arc::new(AtomicBool::new(false));
    let cancel = flag.clone();
    let setter = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        cancel.store(true, Ordering::SeqCst);
    });
    let began = Instant::now();
    assert!(matches!(
        proc::with_cancel(flag, || proc::recv_until(
            &rx,
            began + Duration::from_secs(10)
        )),
        proc::Waited::Quitting
    ));
    setter.join().unwrap();
    assert!(began.elapsed() < Duration::from_secs(1));
    assert!(!proc::cancelled());
    let (tx, rx) = mpsc::channel();
    tx.send(42).unwrap();
    assert!(matches!(
        proc::recv_until(&rx, Instant::now() + Duration::from_secs(1)),
        proc::Waited::Got(42)
    ));
}
