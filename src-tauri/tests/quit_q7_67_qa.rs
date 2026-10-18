//! Independent QA for #67 / Q7, D2: pipe edge cases, shutdown accounting, restart state.
//! Each case re-execs itself because shutdown is intentionally process-global and irreversible.
#[path = "support/native.rs"]
mod native;

use queue_lib::{
    download, meta, proc,
    queue::{INTERRUPTED, Queue, Status},
    run_fetch_job,
    sidecar::Sidecar,
};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};

const QUIT: Duration = Duration::from_secs(5);

/// Whether `pid` is gone within a moment of its tree ending: a SIGKILLed process can still be
/// visible for a few milliseconds on macOS while it exits (and before it's a zombie).
fn exits_soon(pid: u32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(1);
    while !native::process_has_exited(pid) {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
    true
}

fn isolated(name: &str, test: impl FnOnce()) {
    if std::env::var("QUEUE_67_QA_CASE").as_deref() == Ok(name) {
        test();
        return;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env("QUEUE_67_QA_CASE", name)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn executable() -> &'static PathBuf {
    static EXE: OnceLock<PathBuf> = OnceLock::new();
    EXE.get_or_init(|| {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let path = manifest.join(format!(
            "target/qa-native-67-fixture-{}{}",
            std::process::id(),
            std::env::consts::EXE_SUFFIX
        ));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let output = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args(["--edition=2024", "--crate-name", "quit_fixture_67_qa"])
            .arg(manifest.join("tests/support/quit_fixture_67_qa.rs"))
            .arg("-o")
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        path
    })
}

struct Fixture(PathBuf);
impl Fixture {
    fn new(scenario: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "queue-67-qa 日本語 {scenario}-{}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("out")).unwrap();
        fs::copy(executable(), Sidecar::YtDlp.path_in(&root)).unwrap();
        fs::write(root.join("scenario"), scenario).unwrap();
        Self(root)
    }
    fn pid(&self, name: &str) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(text) = fs::read_to_string(self.0.join(name))
                && let Ok(pid) = text.parse()
            {
                return pid;
            }
            assert!(Instant::now() < deadline, "{name} wasn't written");
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.0.join("ready").exists() {
            assert!(Instant::now() < deadline, "fixture never became ready");
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn stop_survivors(&self) {
        // Always clean up our own fixtures, including after a failed shutdown assertion.
        for file in ["descendant.pid", "leader.pid"] {
            if let Ok(text) = fs::read_to_string(self.0.join(file))
                && let Ok(pid) = text.parse::<u32>()
                && !native::process_has_exited(pid)
            {
                #[cfg(unix)]
                unsafe {
                    libc::kill(i32::try_from(pid).unwrap(), libc::SIGKILL);
                }
                #[cfg(windows)]
                unsafe {
                    use windows_sys::Win32::{
                        Foundation::CloseHandle,
                        System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess},
                    };
                    let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
                    if !handle.is_null() {
                        TerminateProcess(handle, 1);
                        CloseHandle(handle);
                    }
                }
            }
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop_survivors();
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn quit_interrupts_download_after_stdout_has_closed() {
    isolated("quit_interrupts_download_after_stdout_has_closed", || {
        let fixture = Fixture::new("closed-stdout");
        let root = fixture.0.clone();
        let worker = thread::spawn(move || {
            download::run(
                &root,
                &root.join("work"),
                &root.join("out"),
                "https://example.test/video",
            )
        });
        fixture.ready();
        let leader = fixture.pid("leader.pid");
        // Allow the stdout reader to observe EOF and enter the post-stdout child wait.
        thread::sleep(Duration::from_millis(250));
        proc::shut_down();
        let ended = proc::wait_for_trees(QUIT);
        let alive = !exits_soon(leader);
        fixture.stop_survivors();
        let outcome = worker.join().unwrap();
        assert!(
            ended && !alive,
            "download with closed stdout survived the five-second quit wait (ended={ended}, alive={alive}, outcome={outcome:?})"
        );
        assert!(
            !fixture.0.join("work").exists(),
            "staging is safe for restart cleanup"
        );
    });
}

fn metadata_orphan(scenario: &str) {
    let fixture = Fixture::new(scenario);
    let root = fixture.0.clone();
    let worker = thread::spawn(move || {
        meta::fetch(&root, "https://example.test/video", Duration::from_secs(30))
    });
    let leader = fixture.pid("leader.pid");
    let descendant = fixture.pid("descendant.pid");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !native::process_has_exited(leader) {
        assert!(Instant::now() < deadline, "leader didn't exit");
        thread::sleep(Duration::from_millis(10));
    }
    thread::sleep(Duration::from_millis(100));
    proc::shut_down();
    let ended = proc::wait_for_trees(QUIT);
    let alive = !exits_soon(descendant);
    fixture.stop_survivors();
    let outcome = worker.join().unwrap();
    assert!(
        ended && !alive,
        "metadata descendant holding {scenario} survived the five-second quit wait (ended={ended}, alive={alive}, outcome={outcome:?})"
    );
}

#[test]
fn quit_interrupts_metadata_after_leader_exits_with_stdout_held() {
    isolated(
        "quit_interrupts_metadata_after_leader_exits_with_stdout_held",
        || metadata_orphan("orphan-stdout"),
    );
}

#[test]
fn quit_interrupts_metadata_after_leader_exits_with_stderr_held() {
    isolated(
        "quit_interrupts_metadata_after_leader_exits_with_stderr_held",
        || metadata_orphan("orphan-stderr"),
    );
}

#[cfg(unix)]
#[test]
fn quit_escalates_to_sigkill_for_a_tree_ignoring_sigterm() {
    isolated(
        "quit_escalates_to_sigkill_for_a_tree_ignoring_sigterm",
        || {
            let fixture = Fixture::new("ignore-term");
            let root = fixture.0.clone();
            let worker = thread::spawn(move || {
                meta::fetch(&root, "https://example.test/video", Duration::from_secs(30))
            });
            fixture.ready();
            let leader = fixture.pid("leader.pid");
            let descendant = fixture.pid("descendant.pid");
            let start = Instant::now();
            proc::shut_down();
            let ended = proc::wait_for_trees(QUIT);
            let elapsed = start.elapsed();
            let both_exited = exits_soon(leader) && exits_soon(descendant);
            fixture.stop_survivors();
            let result = worker.join().unwrap();
            assert!(
                ended && both_exited,
                "SIGTERM-resistant tree survived shutdown"
            );
            assert!(
                elapsed >= Duration::from_secs(3) && elapsed < QUIT,
                "SIGTERM grace and five-second bound: {elapsed:?}"
            );
            assert_eq!(result.unwrap_err(), download::QUITTING);
        },
    );
}

#[test]
fn quit_refuses_new_spawns_and_saved_inflight_rows_restore_as_interrupted() {
    isolated(
        "quit_refuses_new_spawns_and_saved_inflight_rows_restore_as_interrupted",
        || {
            // One active download per queue: exercise each pre-quit state independently.
            let mut snapshots = Vec::new();
            for (title, processing) in [("  日本語 🎵 e\u{301}  ", false), ("CON", true)] {
                let mut q = Queue::default();
                let id = q.add("https://example.test/download").unwrap().id;
                q.fetched(
                    id,
                    Ok(meta::Meta {
                        title: Some(title.into()),
                        ..Default::default()
                    }),
                )
                .unwrap();
                q.start(id).unwrap();
                if processing {
                    q.processing(id).unwrap();
                }
                snapshots.push((q, title));
            }
            let mut queue = Queue::default();
            for index in 0..2 {
                queue.add(&format!("https://example.test/{index}")).unwrap();
            }
            let queue = Mutex::new(queue);
            proc::shut_down();
            proc::shut_down(); // idempotent, including an empty registry
            assert!(proc::wait_for_trees(Duration::ZERO));
            for id in [1, 2] {
                assert!(run_fetch_job(&queue, id, || Err(download::QUITTING.into())).is_none());
            }
            let restored = Queue::from_json(&queue.lock().unwrap().to_json()).unwrap();
            for (q, title) in snapshots {
                let restored = Queue::from_json(&q.to_json()).unwrap();
                let item = &restored.items()[0];
                assert_eq!(item.status, Status::Queued);
                assert_eq!(item.notice.as_deref(), Some(INTERRUPTED));
                assert_eq!(item.meta.as_ref().unwrap().title.as_deref(), Some(title));
                assert!(!restored.is_busy());
            }
            assert_eq!(restored.fetching().len(), 2);
            assert!(!restored.is_busy());
            let fixture = Fixture::new("closed-stdout");
            let reason =
                match proc::spawn_tree(&mut Command::new(Sidecar::YtDlp.path_in(&fixture.0))) {
                    Ok(mut tree) => {
                        tree.terminate();
                        panic!("spawn accepted after quit");
                    }
                    Err(reason) => reason,
                };
            assert!(reason.to_string().contains("Queue is quitting"));
            assert!(!fixture.0.join("leader.pid").exists());
            assert!(proc::wait_for_trees(Duration::ZERO));
        },
    );
}

#[test]
fn failed_spawns_do_not_leak_shutdown_accounting_and_wait_times_out() {
    isolated(
        "failed_spawns_do_not_leak_shutdown_accounting_and_wait_times_out",
        || {
            let fixture = Fixture::new("closed-stdout");
            assert!(
                proc::spawn_tree(&mut Command::new(fixture.0.join("missing-executable"))).is_err()
            );
            assert!(proc::wait_for_trees(Duration::ZERO));
            let mut tree = proc::spawn_tree(
                Command::new(Sidecar::YtDlp.path_in(&fixture.0))
                    .stdout(Stdio::null())
                    .stderr(Stdio::null()),
            )
            .unwrap();
            let before = Instant::now();
            assert!(!proc::wait_for_trees(Duration::from_millis(60)));
            assert!(before.elapsed() < Duration::from_secs(1));
            tree.terminate();
            drop(tree);
            proc::shut_down();
            assert!(proc::wait_for_trees(Duration::ZERO));
        },
    );
}
