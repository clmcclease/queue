//! #74 / Q6: cancelling one download stops its whole tree and removes its staging, on each OS,
//! during the download, while processing, and during a 403 retry pause (D-017). Unlike a quit
//! (#67) a cancel is per download, so these share one test binary.
#[path = "support/native.rs"]
mod native;

use native::{executable, process_has_exited};
use queue_lib::{download, proc, sidecar::Sidecar};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

/// A sidecar folder whose "yt-dlp" is the native fixture, playing `scenario`.
fn fake(name: &str, scenario: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "queue-cancel-74 日本語 {name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("out")).unwrap();
    fs::copy(executable(), Sidecar::YtDlp.path_in(&root)).unwrap();
    fs::write(root.join("scenario.txt"), scenario).unwrap();
    fs::write(root.join("name.txt"), "Title.webm").unwrap();
    root
}

fn pid(root: &Path, file: &str) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(text) = fs::read_to_string(root.join(file))
            && let Ok(pid) = text.parse()
        {
            return pid;
        }
        assert!(Instant::now() < deadline, "{file} never appeared");
        thread::sleep(Duration::from_millis(25));
    }
}

fn assert_exits(pid: u32, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !process_has_exited(pid) {
        assert!(
            Instant::now() < deadline,
            "{what} (PID {pid}) outlived the cancel"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

/// Runs a download of `root`'s fixture on its own thread with a cancel flag, as `lib.rs` does,
/// reporting each Processing event and retry on `events`.
fn start(
    root: &Path,
    events: mpsc::Sender<String>,
) -> (Arc<AtomicBool>, thread::JoinHandle<Result<String, String>>) {
    let flag = Arc::new(AtomicBool::new(false));
    let cancel = flag.clone();
    let root = root.to_owned();
    let run = thread::spawn(move || {
        let retries = events.clone();
        proc::with_cancel(cancel, || {
            download::run_with_retries_events(
                &root,
                &root.join("work"),
                &root.join("out"),
                "https://example.test/v",
                None,
                proc::sleep_unless_quitting,
                move |attempt| {
                    let _ = retries.send(format!("retry {attempt}"));
                },
                |event| {
                    if let download::Event::Processing(name) = event {
                        let _ = events.send(name);
                    }
                },
            )
        })
    });
    (flag, run)
}

/// Cancels, then checks the run ended as cancelled, promptly, with nothing left behind.
fn cancel_and_check(
    root: &Path,
    flag: &AtomicBool,
    run: thread::JoinHandle<Result<String, String>>,
) {
    let cancelled = Instant::now();
    flag.store(true, Ordering::SeqCst);
    assert_eq!(run.join().unwrap().unwrap_err(), download::CANCELLED);
    assert!(
        cancelled.elapsed() < Duration::from_secs(5),
        "took {:?}",
        cancelled.elapsed()
    );
    assert!(!root.join("work").exists(), "the staging folder is removed");
    assert!(
        fs::read_dir(root.join("out")).unwrap().next().is_none(),
        "nothing was placed"
    );
    assert!(!proc::stopping(), "a cancel isn't a quit");
}

#[test]
fn cancelling_a_download_stops_its_whole_tree_and_removes_its_staging() {
    let root = fake("download", "hang");
    let (events, _) = mpsc::channel();
    let (flag, run) = start(&root, events);
    let leader = pid(&root, "leader.pid");
    let descendant = pid(&root, "descendant.pid");
    cancel_and_check(&root, &flag, run);
    assert_exits(leader, "the download");
    assert_exits(descendant, "the download's descendant");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cancelling_while_processing_stops_the_merge_too() {
    let root = fake("processing", "processing-hang");
    let (events, received) = mpsc::channel();
    let (flag, run) = start(&root, events);
    assert_eq!(
        received.recv_timeout(Duration::from_secs(20)).unwrap(),
        "Merger",
        "the row is processing"
    );
    let leader = pid(&root, "leader.pid");
    let descendant = pid(&root, "descendant.pid");
    cancel_and_check(&root, &flag, run);
    assert_exits(leader, "the download");
    assert_exits(descendant, "the merge");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cancelling_during_a_retry_pause_cuts_it_short_and_starts_no_new_attempt() {
    let root = fake("retry", "retry");
    let (events, received) = mpsc::channel();
    let (flag, run) = start(&root, events);
    // The first attempt got a 403; the run now waits 10 s before the second (D-017).
    assert_eq!(
        received.recv_timeout(Duration::from_secs(20)).unwrap(),
        "retry 2"
    );
    cancel_and_check(&root, &flag, run);
    assert_eq!(
        fs::read_to_string(root.join("attempt.txt")).unwrap(),
        "1",
        "no second attempt ran"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_cancel_stops_only_its_own_download() {
    let cancelled = fake("one", "hang");
    let other = fake("other", "retry");
    let (events, _) = mpsc::channel();
    let (flag, run) = start(&cancelled, events);
    pid(&cancelled, "leader.pid");
    flag.store(true, Ordering::SeqCst);
    // Another download, on another thread with its own flag, still runs to the end (its retry
    // waits are skipped here so the test stays quick).
    let other_run = {
        let other = other.clone();
        thread::spawn(move || {
            proc::with_cancel(Arc::new(AtomicBool::new(false)), || {
                download::run_with_retries_events(
                    &other,
                    &other.join("work"),
                    &other.join("out"),
                    "https://example.test/v",
                    None,
                    |_| {},
                    |_| {},
                    |_| {},
                )
            })
        })
    };
    assert_eq!(run.join().unwrap().unwrap_err(), download::CANCELLED);
    let saved = PathBuf::from(other_run.join().unwrap().unwrap());
    assert_eq!(fs::read(saved).unwrap(), b"fixture media");
    let _ = fs::remove_dir_all(cancelled);
    let _ = fs::remove_dir_all(other);
}
