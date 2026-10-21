//! #74 / D-027: quit wins over cancel. Separate binary: shutdown is irreversible.
#[path = "support/native.rs"]
mod native;

use queue_lib::{download, proc, sidecar::Sidecar};
use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

#[test]
fn quitting_and_cancelling_together_reports_quitting_and_cleans_the_tree() {
    let root = std::env::temp_dir().join(format!("queue-74-quit-qa-{}", std::process::id()));
    fs::create_dir_all(root.join("out")).unwrap();
    fs::copy(native::executable(), Sidecar::YtDlp.path_in(&root)).unwrap();
    fs::write(root.join("scenario.txt"), "hang").unwrap();
    let flag = Arc::new(AtomicBool::new(false));
    let cancel = flag.clone();
    let dir = root.clone();
    let (tx, rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        tx.send(proc::with_cancel(cancel, || {
            download::run(
                &dir,
                &dir.join("work"),
                &dir.join("out"),
                "https://example.test/v",
            )
        }))
        .unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    let descendant = loop {
        if let Ok(text) = fs::read_to_string(root.join("descendant.pid"))
            && let Ok(pid) = text.parse()
        {
            break pid;
        }
        assert!(Instant::now() < deadline, "fixture did not start");
        thread::sleep(Duration::from_millis(10));
    };
    proc::shut_down();
    flag.store(true, Ordering::SeqCst);
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap_err(),
        download::QUITTING
    );
    worker.join().unwrap();
    assert!(proc::wait_for_trees(Duration::from_secs(1)));
    let deadline = Instant::now() + Duration::from_secs(2);
    while !native::process_has_exited(descendant) {
        assert!(
            Instant::now() < deadline,
            "descendant survived quit and cancel"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(!root.join("work").exists());
    assert_eq!(fs::read_dir(root.join("out")).unwrap().count(), 0);
    fs::remove_dir_all(root).unwrap();
}
