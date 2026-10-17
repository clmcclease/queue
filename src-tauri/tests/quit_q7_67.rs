//! #67 / Q7, D2: quitting stops running downloads and metadata fetches, and every process they
//! started, on each OS. One test: `proc::shut_down` can't be undone in this process, so it gets
//! a test binary of its own.
#[path = "support/native.rs"]
mod native;

use native::{executable, process_has_exited};
use queue_lib::{download, meta, proc, queue::Queue, run_fetch_job, sidecar::Sidecar};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

/// A sidecar folder whose "yt-dlp" is the native fixture, playing `scenario`.
fn fake(name: &str, scenario: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "queue-quit-67 日本語 {name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("out")).unwrap();
    fs::copy(executable(), Sidecar::YtDlp.path_in(&root)).unwrap();
    fs::write(root.join("scenario.txt"), scenario).unwrap();
    fs::write(root.join("name.txt"), "unused.webm").unwrap();
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
        assert!(
            Instant::now() < deadline,
            "{file} never appeared in {}",
            root.display()
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn assert_exits(pid: u32, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !process_has_exited(pid) {
        assert!(
            Instant::now() < deadline,
            "{what} (PID {pid}) outlived the quit"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn quitting_stops_downloads_and_fetches_and_everything_they_started() {
    let dl = fake("download", "hang");
    let md = fake("meta", "timeout");
    let download = {
        let dl = dl.clone();
        thread::spawn(move || {
            download::run(
                &dl,
                &dl.join("work"),
                &dl.join("out"),
                "https://example.test/v",
            )
        })
    };
    let fetch = {
        let md = md.clone();
        thread::spawn(move || meta::fetch(&md, "https://example.test/v", Duration::from_secs(60)))
    };
    // Both are running: the download's leader and its pipe-holding descendant, and the fetch.
    let leader = pid(&dl, "leader.pid");
    let descendant = pid(&dl, "descendant.pid");
    let fetcher = pid(&md, "leader.pid");
    assert!(
        !proc::wait_for_trees(Duration::from_millis(50)),
        "trees are live before the quit"
    );

    // A job that's still recording its result holds up the final save.
    let recording = proc::job().expect("jobs start before the quit");

    let quit = Instant::now();
    proc::shut_down();
    assert!(proc::stopping());
    assert!(
        proc::wait_for_trees(Duration::from_secs(5)),
        "every tree ended within the quit wait"
    );
    assert!(quit.elapsed() < Duration::from_secs(5));
    assert_eq!(download.join().unwrap().unwrap_err(), download::QUITTING);
    assert_eq!(fetch.join().unwrap().unwrap_err(), download::QUITTING);
    assert_exits(leader, "the download");
    assert_exits(descendant, "the download's descendant");
    assert_exits(fetcher, "the metadata fetch");
    assert!(
        !dl.join("out").read_dir().unwrap().any(|_| true),
        "nothing was placed"
    );
    assert!(
        !proc::wait_idle(Duration::from_millis(100)),
        "a job is still recording"
    );
    drop(recording);
    assert!(
        proc::wait_idle(Duration::from_millis(100)),
        "idle once it's recorded"
    );
    assert!(proc::job().is_none(), "no job starts after the quit");

    // After the quit nothing new starts.
    let late = fake("late", "timeout");
    let reason = meta::fetch(&late, "https://example.test/v", Duration::from_secs(60)).unwrap_err();
    assert!(reason.contains("Queue is quitting"), "{reason}");
    assert!(!late.join("attempt.txt").exists(), "the fixture never ran");
    let reason = download::run(
        &late,
        &late.join("work"),
        &late.join("out"),
        "https://example.test/v",
    )
    .unwrap_err();
    assert!(reason.contains("Queue is quitting"), "{reason}");

    // A copy to another volume stops, removing the partial file and keeping the source, so a
    // quit can't leave a cut-off file under the real name.
    let src = late.join("staged.webm");
    fs::write(&src, vec![7u8; 3 << 20]).unwrap();
    let dest = late.join("out").join("Title.webm");
    let err = download::copy_no_clobber(&src, &dest).unwrap_err();
    assert_eq!(err.to_string(), download::QUITTING);
    assert!(!dest.exists() && src.exists());

    // A retry's pause wakes up at once.
    let pause = Instant::now();
    proc::sleep_unless_quitting(Duration::from_secs(30));
    assert!(pause.elapsed() < Duration::from_secs(1));

    // A fetch that ends after the quit leaves its row Fetching, so it fetches again after a
    // restart, instead of recording the quit as a failure.
    let queue = Mutex::new(Queue::default());
    let id = queue
        .lock()
        .unwrap()
        .add("https://example.test/v")
        .unwrap()
        .id;
    let before = queue.lock().unwrap().to_json();
    assert!(run_fetch_job(&queue, id, || Err(download::QUITTING.to_string())).is_none());
    assert_eq!(
        queue.lock().unwrap().to_json(),
        before,
        "the row is unchanged"
    );
    // One that finished is still recorded, so the final save keeps it.
    let done = run_fetch_job(&queue, id, || {
        let meta = meta::Meta {
            title: Some("日本語".into()),
            ..Default::default()
        };
        Ok((meta, None))
    })
    .expect("a finished fetch is recorded during the quit");
    assert_eq!(done.status, queue_lib::queue::Status::Queued);

    for root in [dl, md, late] {
        let _ = fs::remove_dir_all(root);
    }
}
