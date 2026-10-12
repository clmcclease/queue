//! Independent QA for #7 / Q4,Q5. Tests only; no native UI or external network.
use queue_lib::{
    meta::Meta,
    queue::{Queue, Status, run_all},
    run_fetch_job,
};
use std::sync::{Mutex, mpsc};
use std::time::Duration;

fn ready(queue: &mut Queue, url: &str) -> u64 {
    let id = queue.add(url).unwrap().id;
    queue.fetched(id, Ok(Meta::default())).unwrap();
    id
}

#[test]
fn terminal_rows_are_never_downloaded_again() {
    let mut q = Queue::default();
    let done = ready(&mut q, "https://e.test/done");
    q.start(done).unwrap();
    q.finish(done, Ok("/d/日本語.webm".into())).unwrap();
    let failed = ready(&mut q, "https://e.test/failed");
    q.start(failed).unwrap();
    q.finish(failed, Err("unavailable".into())).unwrap();
    let before = q.items().to_vec();
    let q = Mutex::new(q);
    run_all(
        &q,
        |_| panic!("terminal rows must be skipped"),
        || panic!("no pending work"),
    );
    assert_eq!(q.lock().unwrap().items(), before);
}

#[test]
fn metadata_timeout_during_a_run_fails_only_that_row() {
    let q = Mutex::new(Queue::default());
    let slow = q.lock().unwrap().add("https://e.test/timeout").unwrap().id;
    let next = q.lock().unwrap().add("https://e.test/next").unwrap().id;
    let mut order = Vec::new();
    let mut waits = 0;
    run_all(
        &q,
        |id| {
            let mut q = q.lock().unwrap();
            q.start(id).unwrap();
            order.push(id);
            q.finish(id, Ok("/d/next.webm".into())).unwrap();
            true
        },
        || {
            waits += 1;
            assert_eq!(
                waits, 1,
                "runner must exit after timeout and next completion"
            );
            run_fetch_job(&q, slow, || {
                Err("Getting info timed out after 90 seconds.".into())
            })
            .unwrap();
            run_fetch_job(&q, next, || Ok((Meta::default(), None))).unwrap();
        },
    );
    assert_eq!(order, [next]);
    let q = q.lock().unwrap();
    assert!(
        matches!(&q.items()[0].status, Status::Failed { reason } if reason.contains("timed out"))
    );
    assert!(matches!(q.items()[1].status, Status::Done { .. }));
}

#[test]
fn a_failed_start_is_retried_without_losing_the_row_or_order() {
    let mut q = Queue::default();
    let a = ready(&mut q, "https://e.test/a");
    let b = ready(&mut q, "https://e.test/b");
    let q = Mutex::new(q);
    let mut calls = Vec::new();
    let mut waits = 0;
    run_all(
        &q,
        |id| {
            calls.push(id);
            if calls.len() == 1 {
                return false;
            }
            let mut q = q.lock().unwrap();
            q.start(id).unwrap();
            q.finish(id, Ok(format!("/d/{id}.webm"))).unwrap();
            true
        },
        || waits += 1,
    );
    assert_eq!(calls, [a, a, b]);
    assert_eq!(waits, 1);
}

#[test]
fn an_item_added_fetching_during_download_is_waited_for_and_downloaded() {
    let mut q = Queue::default();
    let first = ready(&mut q, "https://e.test/first");
    let q = Mutex::new(q);
    let mut order = Vec::new();
    let mut waits = 0;
    run_all(
        &q,
        |id| {
            let mut q = q.lock().unwrap();
            q.start(id).unwrap();
            if id == first {
                q.add("https://e.test/new").unwrap();
            }
            q.finish(id, Ok(format!("/d/{id}.webm"))).unwrap();
            order.push(id);
            true
        },
        || {
            waits += 1;
            assert_eq!(waits, 1);
            let mut q = q.lock().unwrap();
            let pending = q
                .items()
                .iter()
                .find(|i| i.status == Status::Fetching)
                .unwrap()
                .id;
            q.fetched(pending, Ok(Meta::default())).unwrap();
        },
    );
    assert_eq!(order, [first, first + 1]);
}

#[test]
fn manual_start_cannot_take_the_slot_while_a_runner_download_is_blocked() {
    let mut q = Queue::default();
    let a = ready(&mut q, "https://e.test/a");
    let b = ready(&mut q, "https://e.test/b");
    let q = Mutex::new(q);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    std::thread::scope(|scope| {
        let worker_queue = &q;
        let worker = scope.spawn(move || {
            let mut order = Vec::new();
            run_all(
                worker_queue,
                |id| {
                    worker_queue.lock().unwrap().start(id).unwrap();
                    if id == a {
                        started_tx.send(id).unwrap();
                        release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
                    }
                    order.push(id);
                    worker_queue
                        .lock()
                        .unwrap()
                        .finish(id, Ok(format!("/d/{id}")))
                        .unwrap();
                    true
                },
                || panic!("runner owns the slot"),
            );
            order
        });
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(3)).unwrap(), a);
        let attempt = q.lock().unwrap().start(b);
        let active = q
            .lock()
            .unwrap()
            .items()
            .iter()
            .filter(|i| i.status == Status::Downloading)
            .count();
        release_tx.send(()).unwrap();
        assert!(attempt.unwrap_err().contains("Another download"));
        assert_eq!(active, 1);
        assert_eq!(worker.join().unwrap(), [a, b]);
    });
}

#[test]
fn emitted_item_states_keep_unicode_metadata_and_monotonic_revisions() {
    for title in [
        "日本語 🎵 e\u{301} – العربية",
        "CON",
        "NUL.txt",
        "title<>:\"/\\|?*.",
    ] {
        let mut q = Queue::default();
        let added = q.add("https://e.test/video").unwrap();
        let fetched = q
            .fetched(
                added.id,
                Ok(Meta {
                    title: Some(title.into()),
                    ..Meta::default()
                }),
            )
            .unwrap();
        let started = q.start(added.id).unwrap();
        let retried = q.retrying(added.id, 2).unwrap();
        let done = q.finish(added.id, Ok(format!("/d/{title}.webm"))).unwrap();
        let states = [&added, &fetched, &started, &retried, &done];
        let encoded: Vec<_> = states
            .iter()
            .map(|i| serde_json::to_value(i).unwrap())
            .collect();
        assert_eq!(
            encoded
                .iter()
                .map(|i| i["status"]["state"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["fetching", "queued", "downloading", "downloading", "done"]
        );
        assert!(states.windows(2).all(|w| w[0].rev < w[1].rev));
        assert_eq!(done.meta.unwrap().title.as_deref(), Some(title));
        assert_eq!(done.attempt, 0);
    }
}

#[cfg(unix)]
mod subprocess {
    use super::*;
    use queue_lib::{download, sidecar::Sidecar};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn collision_and_bad_progress_do_not_stop_later_subprocess_downloads() {
        let root = std::env::temp_dir().join(format!("queue-q4-qa-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        let bin = root.join("bin");
        let out = root.join("out");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&out).unwrap();
        let executable = Sidecar::YtDlp.path_in(&bin);
        fs::write(
            &executable,
            r#"#!/bin/sh
P=; prev=; url=
for arg in "$@"; do
  [ "$prev" = -P ] && P=$arg
  prev=$arg
  url=$arg
done
printf '%s\n' '[download] NaN% ETA ???' '{broken json' 'progress:null'
case "$url" in
  */collision) printf new > "$P/日本語 é.webm" ;;
  */bad) echo 'ERROR: <script>bad()</script> 日本語 unavailable' >&2; exit 1 ;;
  *) printf media > "$P/次の動画.webm" ;;
esac
"#,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let existing = out.join("日本語 e\u{301}.webm");
        fs::write(&existing, b"original").unwrap();
        let mut q = Queue::default();
        for suffix in ["collision", "bad", "success"] {
            ready(&mut q, &format!("https://e.test/{suffix}"));
        }
        let q = Mutex::new(q);
        let mut order = Vec::new();
        run_all(
            &q,
            |id| {
                let item = q.lock().unwrap().start(id).unwrap();
                let result = download::run_with_retries(
                    &bin,
                    &root.join(format!("work-{id}")),
                    &out,
                    &item.url,
                    None,
                    |_| panic!("these errors are not retryable"),
                    |_| panic!("no retries"),
                );
                q.lock().unwrap().finish(id, result).unwrap();
                order.push(id);
                true
            },
            || panic!("nothing pending"),
        );
        assert_eq!(order, [1, 2, 3]);
        let q = q.lock().unwrap();
        // N2 (pulled forward with #7): a name collision keeps both instead of failing. On macOS
        // the NFC/NFD names collide and the new file gets " (2)"; elsewhere they're distinct.
        assert!(matches!(&q.items()[0].status, Status::Done { .. }));
        assert!(
            matches!(&q.items()[1].status, Status::Failed { reason } if reason == "<script>bad()</script> 日本語 unavailable")
        );
        assert!(matches!(q.items()[2].status, Status::Done { .. }));
        assert_eq!(fs::read(existing).unwrap(), b"original");
        assert_eq!(fs::read(out.join("次の動画.webm")).unwrap(), b"media");
        for id in 1..=3 {
            assert!(!root.join(format!("work-{id}")).exists());
        }
    }
}
