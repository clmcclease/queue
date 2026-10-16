//! Independent QA for #9 / Q6. No production edits; fixtures stay in temporary folders.
use queue_lib::{
    dispatch_fetch, download, meta,
    queue::{Queue, Status, run_all},
    run_fetch_job,
};
use std::{
    fs,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::SystemTime,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "queue-q6-qa-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn ready(q: &mut Queue, url: &str) -> u64 {
    let id = q.add(url).unwrap().id;
    q.fetched(id, Ok(meta::Meta::default())).unwrap();
    id
}

#[test]
fn q6_retry_rejects_every_non_failed_state_without_mutating_the_queue() {
    let mut q = Queue::default();
    let id = q.add("https://example.test/a").unwrap().id;
    for state in 0..5 {
        let before = q.items().to_vec();
        assert!(q.retry(id).unwrap_err().contains("Only failed"));
        assert_eq!(q.items(), before);
        match state {
            0 => {
                q.fetched(id, Ok(meta::Meta::default())).unwrap();
            }
            1 => {
                q.start(id).unwrap();
            }
            2 => {
                q.processing(id).unwrap();
            }
            3 => {
                q.finish(id, Ok("saved.webm".into())).unwrap();
            }
            _ => {}
        }
    }
}

#[test]
fn q6_retry_duplicate_guard_covers_all_active_states_and_can_be_unblocked_by_remove() {
    for state in 0..5 {
        let mut q = Queue::default();
        let old = ready(&mut q, "https://www.youtube.com/watch?v=jNQXAC9IVRw");
        q.start(old).unwrap();
        q.finish(old, Err("unavailable".into())).unwrap();
        let newer = q.add("https://youtu.be/jNQXAC9IVRw").unwrap().id;
        if state >= 1 {
            q.fetched(newer, Ok(meta::Meta::default())).unwrap();
        }
        if state >= 2 {
            q.start(newer).unwrap();
        }
        if state >= 3 {
            q.processing(newer).unwrap();
        }
        if state >= 4 {
            // #59: a finished copy isn't active, so it no longer blocks the retry.
            q.finish(newer, Ok("saved.webm".into())).unwrap();
            assert_eq!(q.retry(old).unwrap().status, Status::Queued);
            continue;
        }
        let before = q.items().to_vec();
        assert!(q.retry(old).unwrap_err().contains("already in the queue"));
        assert_eq!(q.items(), before, "a refused retry must preserve both rows");
        if matches!(
            q.items()[1].status,
            Status::Downloading | Status::Processing
        ) {
            q.finish(newer, Err("failed".into())).unwrap();
        }
        q.remove(newer).unwrap();
        assert_eq!(q.retry(old).unwrap().status, Status::Queued);
    }
}

#[test]
fn q6_repeated_retry_preserves_messy_metadata_and_monotonically_increases_revision() {
    for title in [
        "日本語 🎵 e\u{301} – العربية",
        "CON",
        "NUL.txt",
        "title<>:\"/\\|?*.",
    ] {
        let mut q = Queue::default();
        let id = q.add("https://example.test/title").unwrap().id;
        let metadata = meta::Meta {
            title: Some(title.into()),
            ..Default::default()
        };
        let mut rev = q.fetched(id, Ok(metadata.clone())).unwrap().rev;
        for _ in 0..3 {
            q.start(id).unwrap();
            q.progressed(
                id,
                download::Progress {
                    percent: Some(75.0),
                    part: 2,
                    ..Default::default()
                },
            )
            .unwrap();
            q.processing(id).unwrap();
            let failed = q
                .finish(id, Err("HTTP Error 403: Forbidden".into()))
                .unwrap();
            assert!(failed.rev > rev);
            let retry = q.retry(id).unwrap();
            assert!(retry.rev > failed.rev);
            assert_eq!(retry.meta, Some(metadata.clone()));
            assert_eq!((retry.attempt, retry.progress), (0, None));
            assert_eq!(retry.status, Status::Queued);
            rev = retry.rev;
        }
    }
}

#[test]
fn q6_failed_remove_preserves_progress_and_saved_info() {
    let f = Fixture::new();
    let saved = f.0.join("saved.json");
    fs::write(&saved, "{}").unwrap();
    let mut q = Queue::default();
    let id = ready(&mut q, "https://example.test/a");
    q.set_info(id, saved.clone(), SystemTime::now());
    q.start(id).unwrap();
    q.progressed(
        id,
        download::Progress {
            percent: Some(50.0),
            ..Default::default()
        },
    )
    .unwrap();
    let before = q.items().to_vec();
    assert!(q.remove(id).is_err());
    assert_eq!(q.items(), before);
    assert_eq!(q.fresh_info(id, SystemTime::now()), Some(saved.clone()));
    assert!(saved.exists());
}

#[test]
fn q6_removing_done_returns_only_saved_info_and_leaves_download_bytes_intact() {
    let f = Fixture::new();
    let saved = f.0.join("saved.json");
    let output = f.0.join("日本語 (2).webm");
    fs::write(&saved, "{}").unwrap();
    fs::write(&output, b"completed download").unwrap();
    let mut q = Queue::default();
    let id = ready(&mut q, "https://example.test/a");
    q.set_info(id, saved.clone(), SystemTime::now());
    q.start(id).unwrap();
    q.finish(id, Ok(output.to_string_lossy().into_owned()))
        .unwrap();
    let mut removed = q.remove(id).unwrap();
    assert_eq!(removed.take_info(), Some(saved.clone()));
    assert_eq!(removed.take_info(), None);
    fs::remove_file(saved).unwrap(); // same cleanup as queue_remove
    assert_eq!(fs::read(output).unwrap(), b"completed download");
    assert!(q.items().is_empty());
}

#[test]
fn q6_removal_during_fetch_cleans_info_and_does_not_modify_a_readded_link() {
    let f = Fixture::new();
    let saved = f.0.join("late.json");
    let q = Mutex::new(Queue::default());
    let old = q.lock().unwrap().add("https://example.test/a").unwrap().id;
    let result = run_fetch_job(&q, old, || {
        q.lock().unwrap().remove(old).unwrap();
        let new = q.lock().unwrap().add("https://example.test/a").unwrap();
        assert!(new.id > old);
        fs::write(&saved, "{}").unwrap();
        Ok((meta::Meta::default(), Some(saved.clone())))
    });
    assert_eq!(result, None);
    assert!(!saved.exists());
    assert_eq!(q.lock().unwrap().items()[0].status, Status::Fetching);
}

#[test]
fn q6_retry_of_metadata_failure_recovers_from_dead_worker_without_staying_fetching() {
    let q = Mutex::new(Queue::default());
    let id = q.lock().unwrap().add("https://example.test/a").unwrap().id;
    q.lock()
        .unwrap()
        .fetched(id, Err("timed out".into()))
        .unwrap();
    let retry = q.lock().unwrap().retry(id).unwrap();
    let (sender, receiver) = mpsc::channel();
    drop(receiver);
    let failed = dispatch_fetch(&q, &Mutex::new(sender), retry);
    assert!(
        matches!(failed.status, Status::Failed { ref reason } if reason.contains("Restart Queue"))
    );
    let retry = q.lock().unwrap().retry(id).unwrap();
    let (sender, receiver) = mpsc::channel();
    dispatch_fetch(&q, &Mutex::new(sender), retry);
    assert_eq!(
        receiver.recv().unwrap(),
        (id, "https://example.test/a".into())
    );
    assert_eq!(
        run_fetch_job(&q, id, || Ok((meta::Meta::default(), None)))
            .unwrap()
            .status,
        Status::Queued
    );
}

#[test]
fn q6_running_queue_picks_up_retry_and_skips_removed_rows_after_a_failure() {
    let q = Mutex::new(Queue::default());
    let failed = ready(&mut q.lock().unwrap(), "https://example.test/fail");
    let active = ready(&mut q.lock().unwrap(), "https://example.test/active");
    let removed = ready(&mut q.lock().unwrap(), "https://example.test/remove");
    let mut order = Vec::new();
    run_all(
        &q,
        |id| {
            let mut q = q.lock().unwrap();
            q.start(id).unwrap();
            order.push(id);
            if id == failed && order.len() == 1 {
                q.finish(id, Err("Unavailable".into())).unwrap();
            } else {
                q.finish(id, Ok(format!("{id}.webm"))).unwrap();
            }
            if id == active {
                q.retry(failed).unwrap();
                q.remove(removed).unwrap();
            }
            true
        },
        || panic!("all metadata is already ready"),
    );
    assert_eq!(order, [failed, active, failed]);
    assert!(
        q.lock()
            .unwrap()
            .items()
            .iter()
            .all(|i| matches!(i.status, Status::Done { .. }))
    );
}

#[test]
fn q6_failure_reason_ignores_malformed_progress_after_the_last_error() {
    let stderr = "ERROR: old\r\nERROR: [generic] 日本語: 動画を取得できません 🎵\r\nQUEUE-PROGRESS {broken\nWARNING: cleanup\n";
    assert_eq!(
        download::failure_reason(stderr, "exit status: 1"),
        "動画を取得できません 🎵"
    );
    assert!(download::is_forbidden(&download::failure_reason(
        "ERROR: [youtube] id: HTTP Error 403: Forbidden",
        "1"
    )));
}

#[test]
fn q6_empty_error_body_still_has_a_readable_failure_reason() {
    let reason = download::failure_reason("WARNING: cleanup\nERROR:   \n", "exit status: 1");
    assert!(
        !reason.trim().is_empty(),
        "a failed row must have a readable reason, got {reason:?}"
    );
}

#[cfg(unix)]
#[test]
fn q6_metadata_timeout_can_be_retried_and_refetched_successfully() {
    use std::{os::unix::fs::PermissionsExt, time::Duration};
    let f = Fixture::new();
    let script = queue_lib::sidecar::Sidecar::YtDlp.path_in(&f.0);
    fs::write(&script, "#!/bin/sh\nsleep 30\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let q = Mutex::new(Queue::default());
    let id = q
        .lock()
        .unwrap()
        .add("https://example.test/timeout")
        .unwrap()
        .id;
    let failed = run_fetch_job(&q, id, || {
        meta::fetch(
            &f.0,
            "https://example.test/timeout",
            Duration::from_millis(100),
        )
        .map(|m| (m, None))
    })
    .unwrap();
    assert!(
        matches!(failed.status, Status::Failed { ref reason } if reason.contains("took longer"))
    );
    q.lock().unwrap().retry(id).unwrap();
    fs::write(
        &script,
        "#!/bin/sh\nprintf '%s\\n' '{\"title\":\"日本語 🎵 CON\"}'\n",
    )
    .unwrap();
    let ready = run_fetch_job(&q, id, || {
        meta::fetch(&f.0, "https://example.test/timeout", Duration::from_secs(5)).map(|m| (m, None))
    })
    .unwrap();
    assert_eq!(ready.status, Status::Queued);
    assert_eq!(ready.meta.unwrap().title.as_deref(), Some("日本語 🎵 CON"));
}
