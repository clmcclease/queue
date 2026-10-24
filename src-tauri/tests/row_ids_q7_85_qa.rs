//! Independent QA for #85 / Q7: restored and allocated ids must survive JavaScript numbers.
use queue_lib::{
    meta::Meta,
    queue::{AddError, MAX_ID, Queue, Saving, Status},
    settings::write_atomic,
};
use serde_json::{Value, json};

fn row(id: u64) -> Value {
    json!({
        "id": id, "url": format!("https://e.test/{id}"),
        "status": {"state": "queued"},
        "meta": {"title": "  日本語 🎵 e\u{301} (OFFICIAL VIDEO) / CON  "}
    })
}

fn restore(rows: Vec<Value>) -> Queue {
    Queue::from_json(&serde_json::to_vec(&json!({"version": 1, "items": rows})).unwrap()).unwrap()
}

fn full(q: &mut Queue) {
    let before = q.to_json();
    let changes = q.changes();
    for _ in 0..3 {
        assert_eq!(
            q.add("https://e.test/new"),
            Err(AddError::Invalid {
                message: "The queue is full. Remove some rows to add more.".into()
            })
        );
        assert_eq!(q.to_json(), before, "rejection must not mutate rows");
        assert_eq!(q.changes(), changes, "rejection must not dirty persistence");
    }
}

#[test]
fn mixed_unsafe_rows_are_dropped_without_losing_valid_rows_or_poisoning_allocation() {
    let mut q = restore(vec![
        row(MAX_ID + 1),
        row(9),
        row(u64::MAX),
        row(0),
        row(MAX_ID + 2),
        row(2),
    ]);
    assert_eq!(q.items().iter().map(|i| i.id).collect::<Vec<_>>(), [2, 9]);
    // Dropping an unsafe row must drop its duplicate identity as well.
    assert_eq!(
        q.add(&format!("https://e.test/{}", MAX_ID + 1)).unwrap().id,
        10
    );
    assert_eq!(q.add("https://e.test/next").unwrap().id, 11);
    let saved = q.to_json();
    assert_eq!(Queue::from_json(&saved).unwrap().to_json(), saved);
}

#[test]
fn unsafe_ids_are_dropped_in_every_persisted_state() {
    for status in [
        json!({"state": "fetching"}),
        json!({"state": "queued"}),
        json!({"state": "downloading"}),
        json!({"state": "processing"}),
        json!({"state": "done", "path": "/downloads/CON (2).opus"}),
        json!({"state": "failed", "reason": "Timed out 日本語"}),
    ] {
        let mut unsafe_row = row(MAX_ID + 1);
        unsafe_row["status"] = status;
        let q = restore(vec![unsafe_row, row(MAX_ID)]);
        assert_eq!(q.items().len(), 1);
        assert_eq!(q.items()[0].id, MAX_ID);
    }
}

#[test]
fn rejection_at_the_limit_is_repeatable_and_leaves_persistence_unchanged() {
    let mut q = restore(vec![row(MAX_ID)]);
    full(&mut q);
    let mut q = Queue::from_json(&q.to_json()).unwrap();
    full(&mut q);
}

#[test]
fn invalid_and_duplicate_adds_do_not_consume_the_last_safe_id() {
    let mut q = restore(vec![row(MAX_ID - 1)]);
    assert!(matches!(
        q.add("javascript:alert(1)"),
        Err(AddError::Invalid { .. })
    ));
    assert!(matches!(q.add(&format!("https://e.test/{}", MAX_ID - 1)),
        Err(AddError::Duplicate { id, .. }) if id == MAX_ID - 1));
    let last = q.add("https://e.test/last").unwrap();
    assert_eq!(last.id, MAX_ID);
    let wire = serde_json::to_value(&last).unwrap();
    assert_eq!(wire["id"].as_f64().unwrap() as u64, MAX_ID);
    assert_eq!(wire["id"].to_string(), "9007199254740991");
    full(&mut q);
}

#[test]
fn adjacent_large_ids_address_only_the_requested_row() {
    let mut q = restore(vec![row(MAX_ID), row(MAX_ID - 1)]);
    q.rename(MAX_ID, "NUL.txt").unwrap();
    assert_ne!(q.items()[0].name, q.items()[1].name);
    assert_eq!(q.items()[1].name.as_deref(), Some("NUL_.txt"));
    q.start(MAX_ID).unwrap();
    q.finish(MAX_ID, Err("Timed out 日本語".into())).unwrap();
    assert_eq!(q.items()[0].status, Status::Queued);
    assert!(matches!(q.items()[1].status, Status::Failed { .. }));
    q.retry(MAX_ID).unwrap();
    q.remove(MAX_ID - 1).unwrap();
    assert_eq!(q.items().len(), 1);
    assert_eq!(q.items()[0].id, MAX_ID);
    assert_eq!(q.items()[0].status, Status::Queued);
}

#[test]
fn safe_limit_survives_fetch_jobs_and_serialized_duplicate_errors() {
    let mut q = restore(vec![row(MAX_ID - 1)]);
    let id = q.add("https://e.test/last").unwrap().id;
    assert_eq!(q.fetching(), [(MAX_ID, "https://e.test/last".into())]);
    q.fetched(id, Ok(Meta::default())).unwrap();
    let error = q.add("https://e.test/last").unwrap_err();
    assert_eq!(serde_json::to_value(error).unwrap()["id"], json!(MAX_ID));
    assert_eq!(q.awaiting_ai(), [MAX_ID - 1, MAX_ID]);
}

#[test]
fn disk_restore_filters_unsafe_ids_and_persists_the_last_safe_allocation() {
    let dir = std::env::temp_dir().join(format!("queue-q7-85-qa-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());
    let path = dir.join("queue.json");
    let saved = serde_json::to_vec(&json!({"version": 1,
        "items": [row(MAX_ID + 1), row(MAX_ID - 1), row(u64::MAX)]}))
    .unwrap();
    std::fs::write(&path, &saved).unwrap();
    let (mut q, saving) = Queue::load_checked(&path);
    assert_eq!(saving, Saving::Safe);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        saved,
        "loading alone doesn't write"
    );
    assert!(!path.with_extension("json.bad").exists());
    assert_eq!(q.add("https://e.test/last").unwrap().id, MAX_ID);
    write_atomic(&path, &q.to_json()).unwrap();
    let (mut restarted, saving) = Queue::load_checked(&path);
    assert_eq!(saving, Saving::Safe);
    assert_eq!(
        restarted.items().iter().map(|i| i.id).collect::<Vec<_>>(),
        [MAX_ID - 1, MAX_ID]
    );
    full(&mut restarted);
}
