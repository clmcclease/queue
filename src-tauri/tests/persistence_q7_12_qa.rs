//! Independent QA: #12 / Q7, D-025. Only disposable files; no app-data or real downloads.
use queue_lib::{
    download::{self, Format, Progress},
    meta::Meta,
    queue::{AddError, INTERRUPTED, Queue, Status},
    run_fetch_job,
    settings::{Settings, write_atomic},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::SystemTime,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "queue-q7-12-qa-{}-{}",
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
    q.fetched(id, Ok(Meta::default())).unwrap();
    id
}
fn document(rows: Vec<Value>) -> Vec<u8> {
    serde_json::to_vec(&json!({"version": 1, "items": rows})).unwrap()
}
fn row(id: u64, url: &str, state: &str) -> Value {
    json!({"id": id, "url": url, "status": {"state": state}, "meta": {}, "notice": null})
}

#[test]
fn q7_round_trips_messy_unicode_metadata_and_exact_completed_paths() {
    let f = Fixture::new();
    let path = f.0.join("nested 日本語").join("queue.json");
    let mut q = Queue::default();
    for (n, title) in [
        "CON",
        "NUL.txt",
        "LPT9",
        "  日本語 🎵 e\u{301} (OFFICIAL VIDEO)  ",
        "quote \" & <script>literal</script> / : ? *",
        "🎵",
    ]
    .iter()
    .enumerate()
    {
        let id = q
            .add(&format!("https://example.test/video/{n}"))
            .unwrap()
            .id;
        q.fetched(
            id,
            Ok(Meta {
                title: Some((*title).into()),
                channel: Some("Björk & 日本語".into()),
                duration: Some(123.25),
                thumbnail: Some("https://img.test/a?x=1&x=2".into()),
                site: Some("Generic".into()),
            }),
        )
        .unwrap();
        q.start(id).unwrap();
        q.finish(id, Ok(format!("C:\\Downloads 日本語\\_CON ({n}).m4a")))
            .unwrap();
        q.noticed(id, "Original format retained 🎵".into()).unwrap();
    }
    write_atomic(&path, &q.to_json()).unwrap();
    let restored = Queue::load(&path);
    assert_eq!(restored.to_json(), q.to_json());
    for (old, new) in q.items().iter().zip(restored.items()) {
        assert_eq!(new.meta, old.meta);
        assert_eq!(
            restored.saved_file(new.id).unwrap(),
            q.saved_file(old.id).unwrap()
        );
    }
}

#[test]
fn q7_both_inflight_states_wait_and_keep_notice_across_a_second_restart() {
    for processing in [false, true] {
        let mut q = Queue::default();
        let id = ready(&mut q, "https://e.test/interrupted");
        q.start(id).unwrap();
        q.retrying(id, 3).unwrap();
        q.progressed(
            id,
            Progress {
                percent: Some(88.0),
                ..Progress::default()
            },
        )
        .unwrap();
        if processing {
            q.processing(id).unwrap();
        }
        let mut restored = Queue::from_json(&q.to_json()).unwrap();
        assert!(!restored.is_busy());
        assert!(restored.fetching().is_empty());
        let item = &restored.items()[0];
        assert_eq!(item.status, Status::Queued);
        assert_eq!(item.notice.as_deref(), Some(INTERRUPTED));
        assert_eq!((item.progress.clone(), item.attempt), (None, 0));
        restored = Queue::from_json(&restored.to_json()).unwrap();
        assert_eq!(restored.items()[0].notice.as_deref(), Some(INTERRUPTED));
        assert_eq!(restored.start(id).unwrap().notice, None);
    }
}

#[test]
fn q7_saved_json_excludes_progress_attempt_revision_and_cached_info() {
    let f = Fixture::new();
    let cache = f.0.join("private-info.json");
    fs::write(&cache, b"{}").unwrap();
    let mut q = Queue::default();
    let id = ready(&mut q, "https://e.test/info");
    q.set_info(id, cache.clone(), SystemTime::now());
    q.start(id).unwrap();
    q.retrying(id, 2).unwrap();
    q.progressed(
        id,
        Progress {
            percent: Some(50.0),
            ..Progress::default()
        },
    )
    .unwrap();
    let wire: Value = serde_json::from_slice(&q.to_json()).unwrap();
    for key in ["progress", "attempt", "rev", "key", "info"] {
        assert!(wire["items"][0].get(key).is_none(), "{key}");
    }
    let restored = Queue::from_json(&q.to_json()).unwrap();
    assert!(restored.fresh_info(id, SystemTime::now()).is_none());
    assert_eq!(
        fs::read(cache).unwrap(),
        b"{}",
        "restore doesn't alter cached files"
    );
}

#[test]
fn q7_restored_fetch_jobs_can_timeout_then_continue_and_retry() {
    let mut q = Queue::default();
    let a = q.add("https://e.test/timeout").unwrap().id;
    let b = q.add("https://e.test/next").unwrap().id;
    let q = Mutex::new(Queue::from_json(&q.to_json()).unwrap());
    assert_eq!(q.lock().unwrap().fetching().len(), 2);
    let first = run_fetch_job(&q, a, || Err("Getting info timed out".into())).unwrap();
    assert!(matches!(first.status, Status::Failed { ref reason } if reason.contains("timed out")));
    assert_eq!(
        run_fetch_job(&q, b, || Ok((Meta::default(), None)))
            .unwrap()
            .status,
        Status::Queued
    );
    let mut q = Queue::from_json(&q.lock().unwrap().to_json()).unwrap();
    assert_eq!(q.retry(a).unwrap().status, Status::Fetching);
    assert_eq!(q.fetching(), [(a, "https://e.test/timeout".into())]);
}

#[test]
fn q7_failed_download_retry_and_completed_history_dedupe_survive_restart() {
    let mut q = Queue::default();
    let url = "https://www.youtube.com/watch?v=jNQXAC9IVRw";
    let failed = ready(&mut q, url);
    q.start(failed).unwrap();
    q.finish(failed, Err("HTTP Error 403 日本語".into()))
        .unwrap();
    let done = ready(&mut q, url);
    q.start(done).unwrap();
    q.finish(done, Ok("/downloads/Title (2).opus".into()))
        .unwrap();
    let mut q = Queue::from_json(&q.to_json()).unwrap();
    assert_eq!(q.retry(failed).unwrap().status, Status::Queued);
    assert!(
        matches!(q.add("https://youtu.be/jNQXAC9IVRw"), Err(AddError::Duplicate { id, .. }) if id == failed)
    );
    q.remove(failed).unwrap();
    assert_eq!(q.add("https://youtu.be/jNQXAC9IVRw").unwrap().id, done + 1);
    assert_eq!(
        q.saved_file(done).unwrap(),
        PathBuf::from("/downloads/Title (2).opus")
    );
}

#[test]
fn q7_restore_sorts_ids_drops_invalid_links_and_keeps_distinct_query_keys() {
    let bytes = document(vec![
        row(9, "https://e.test/?a=+", "queued"),
        row(2, "https://e.test/?a=%2B", "queued"),
        row(7, "https://e.test/?a=%20", "queued"),
        row(4, "javascript:alert(1)", "queued"),
        row(7, "https://e.test/duplicate-id", "queued"),
    ]);
    let mut q = Queue::from_json(&bytes).unwrap();
    assert_eq!(
        q.items().iter().map(|i| i.id).collect::<Vec<_>>(),
        [2, 7, 9]
    );
    for (url, id) in [
        ("https://e.test/?a=%2b", 2),
        ("https://e.test/?a=+", 9),
        ("https://e.test/?a=%20", 7),
    ] {
        assert!(matches!(q.add(url), Err(AddError::Duplicate { id: actual, .. }) if actual == id));
    }
    assert_eq!(q.add("https://e.test/next").unwrap().id, 10);
}

#[test]
fn q7_missing_metadata_is_refetched_instead_of_becoming_ready() {
    let mut item = row(8, "https://e.test/missing-meta", "queued");
    item["meta"] = Value::Null;
    let q = Queue::from_json(&document(vec![item])).unwrap();
    assert_eq!(q.items()[0].status, Status::Fetching);
    assert_eq!(q.fetching(), [(8, "https://e.test/missing-meta".into())]);
}

#[test]
fn q7_corrupt_files_are_preserved_byte_exactly_and_settings_stay_independent() {
    let f = Fixture::new();
    let path = f.0.join("queue.json");
    let settings_path = f.0.join("settings.json");
    let settings = Settings {
        download_dir: Some(f.0.join("Vidéos 日本語")),
        format: Format::CompatibleAudio,
    };
    settings.save(&settings_path).unwrap();
    let mut damaged_seen = 0;
    for bytes in [
        &b"{\"version\":1,\"items\":["[..],
        b"\xff\xfe",
        b"[]",
        b"null",
        b"{\"version\":1,\"items\":[{\"id\":1}]}",
    ] {
        fs::write(&path, bytes).unwrap();
        let mut q = Queue::load(&path);
        assert!(q.items().is_empty());
        assert!(!path.exists());
        // The newest damaged copy is kept under queue.json.bad*, and earlier ones are never
        // overwritten (a later .bad gets a unique suffix; D-025).
        let kept: Vec<Vec<u8>> = fs::read_dir(&f.0)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("queue.json.bad")
            })
            .map(|p| fs::read(p).unwrap())
            .collect();
        assert!(
            kept.iter().any(|k| k == bytes),
            "the damaged bytes are kept"
        );
        damaged_seen += 1;
        assert_eq!(kept.len(), damaged_seen, "no earlier damaged copy was lost");
        q.add("https://e.test/recovered").unwrap();
        write_atomic(&path, &q.to_json()).unwrap();
        assert_eq!(Queue::load(&path).items().len(), 1);
        assert_eq!(Settings::load(&settings_path), settings);
    }
}

#[test]
fn q7_atomic_write_failure_does_not_remove_previous_snapshot_or_leave_temp_files() {
    let f = Fixture::new();
    let path = f.0.join("queue.json");
    let mut q = Queue::default();
    ready(&mut q, "https://e.test/previous");
    write_atomic(&path, &q.to_json()).unwrap();
    // Force rename failure without permissions or platform-specific assumptions.
    let blocked = f.0.join("blocked.json");
    fs::create_dir(&blocked).unwrap();
    assert!(write_atomic(&blocked, b"replacement").is_err());
    assert_eq!(Queue::load(&path).to_json(), q.to_json());
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 2);
}

#[test]
fn q7_restoring_and_removing_history_leaves_colliding_media_untouched() {
    let f = Fixture::new();
    let output = f.0.join("output");
    fs::create_dir(&output).unwrap();
    let original = output.join("_CON.opus");
    fs::write(&original, b"original").unwrap();
    let mut q = Queue::default();
    let id = ready(&mut q, "https://e.test/music");
    q.start(id).unwrap();
    q.finish(id, Ok(original.to_str().unwrap().into())).unwrap();
    let mut q = Queue::from_json(&q.to_json()).unwrap();
    assert!(q.add("https://e.test/music").is_ok());
    let source = f.0.join("new-media");
    fs::write(&source, b"second format").unwrap();
    let second =
        download::place_unique(&source, &output, std::ffi::OsStr::new("_CON.opus")).unwrap();
    assert_eq!(second, output.join("_CON (2).opus"));
    q.remove(id).unwrap();
    assert_eq!(fs::read(original).unwrap(), b"original");
    assert_eq!(fs::read(second).unwrap(), b"second format");
}

#[test]
fn q7_bad_backup_path_must_not_allow_the_only_damaged_copy_to_be_destroyed() {
    let f = Fixture::new();
    let path = f.0.join("queue.json");
    let damaged = b"{\"version\":1,\"items\":[damaged-but-recoverable";
    fs::write(&path, damaged).unwrap();
    // A pre-existing directory makes rename(queue.json, queue.json.bad) fail on every OS.
    fs::create_dir(path.with_extension("json.bad")).unwrap();
    let mut q = Queue::load(&path);
    q.add("https://e.test/new").unwrap();
    // This is what the background saver does after the first change.
    let _ = write_atomic(&path, &q.to_json());
    let preserved = fs::read_dir(&f.0)
        .unwrap()
        .any(|entry| fs::read(entry.unwrap().path()).is_ok_and(|bytes| bytes == damaged));
    assert!(
        preserved,
        "D-025: preserve the damaged file even when the normal backup path is blocked"
    );
}

#[test]
fn q7_damaged_maximum_id_cannot_panic_the_next_add() {
    let bytes = document(vec![row(u64::MAX, "https://e.test/max-id", "queued")]);
    // Rejecting the corrupt snapshot or dropping the row are both acceptable recovery choices.
    if let Ok(mut q) = Queue::from_json(&bytes) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            q.add("https://e.test/next")
        }));
        assert!(
            result.is_ok(),
            "a saved id must not make queue_add panic and poison its mutex"
        );
    }
}
