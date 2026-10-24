//! Independent QA for Q7 / #83 / #85, using disposable queue files only.
use queue_lib::{
    naming,
    queue::{AddError, MAX_ID, NameSource, Queue, Saving},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "queue-83-85-qa-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn row(id: u64, url: &str) -> Value {
    json!({"id": id, "url": url, "status": {"state": "queued"}, "meta": {"title": "日本語 e\u{301} 🎵"}})
}
fn load(rows: Vec<Value>) -> Queue {
    Queue::from_json(&serde_json::to_vec(&json!({"version": 1, "items": rows})).unwrap()).unwrap()
}
fn protected_version(version: u64, future_shape: bool) {
    let f = Fixture::new();
    let path = f.0.join("queue.json");
    let value = if future_shape {
        json!({"version": version, "items": {"future": ["日本語", null]}, "extra": true})
    } else {
        json!({"version": version, "items": [row(1, "https://e.test/saved")]})
    };
    let bytes = serde_json::to_vec_pretty(&value).unwrap();
    fs::write(&path, &bytes).unwrap();
    fs::write(f.0.join("queue.json.bad"), b"previous damaged queue").unwrap();
    // Repeated launches must neither quarantine this file nor authorize a later save.
    for _ in 0..2 {
        let (mut q, saving) = Queue::load_checked(&path);
        assert!(q.items().is_empty());
        assert_eq!(
            fs::read(&path).ok().as_deref(),
            Some(bytes.as_slice()),
            "version {version} must remain at the original path, byte for byte"
        );
        assert_eq!(
            saving,
            Saving::Newer,
            "version {version} must disable saving"
        );
        q.add("https://e.test/session-only").unwrap();
        assert_eq!(q.items()[0].id, 1);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    assert!(Queue::from_json(&bytes).is_err());
    assert_eq!(
        fs::read(f.0.join("queue.json.bad")).unwrap(),
        b"previous damaged queue"
    );
    assert_eq!(
        fs::read_dir(&f.0).unwrap().count(),
        2,
        "no extra quarantine or temp file"
    );
}

#[test]
fn other_u32_versions_are_preserved_even_with_incompatible_rows() {
    for version in [0, 2, u32::MAX as u64] {
        for future_shape in [false, true] {
            protected_version(version, future_shape);
        }
    }
}

#[test]
fn version_above_u32_is_preserved_instead_of_quarantined() {
    protected_version(u32::MAX as u64 + 1, false);
}

#[test]
fn largest_u64_version_with_future_shape_is_preserved() {
    protected_version(u64::MAX, true);
}

#[test]
fn version_one_keeps_additive_fields_compatible_and_sanitizes_saved_names() {
    let f = Fixture::new();
    let path = f.0.join("queue.json");
    for name in ["CON", "NUL.txt", "LPT9", "  日本語 e\u{301} 🎵 / : ? *  "] {
        let mut item = row(7, "https://e.test/name");
        item["name"] = json!(name);
        item["name_source"] = json!("a_future_source");
        item["future_row_field"] = json!({"nested": true});
        let bytes =
            serde_json::to_vec(&json!({"version": 1, "items": [item], "future_top_level": [1, 2]}))
                .unwrap();
        fs::write(&path, &bytes).unwrap();
        let (q, saving) = Queue::load_checked(&path);
        assert_eq!(saving, Saving::Safe);
        assert_eq!(q.items().len(), 1);
        assert_eq!(
            q.items()[0].name.as_deref(),
            Some(naming::sanitize(name).as_str())
        );
        assert_eq!(q.items()[0].name_source, Some(NameSource::Rules));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let saved: Value = serde_json::from_slice(&q.to_json()).unwrap();
        assert_eq!(saved["version"], 1);
        assert_eq!(Queue::from_json(&q.to_json()).unwrap().items(), q.items());
    }
}

#[test]
fn unsafe_rows_do_not_poison_valid_rows_duplicate_checks_or_next_id() {
    let mut q = load(vec![
        row(MAX_ID + 1, "https://e.test/reuse"),
        row(u64::MAX, "https://e.test/max"),
        row(0, "https://e.test/zero"),
        row(13, "https://e.test/valid"),
    ]);
    assert_eq!(q.items().iter().map(|i| i.id).collect::<Vec<_>>(), [13]);
    assert_eq!(q.add("https://e.test/reuse").unwrap().id, 14);
    assert_eq!(q.add("https://e.test/max").unwrap().id, 15);
    assert_eq!(q.add("https://e.test/zero").unwrap().id, 16);
}

#[test]
fn a_queue_with_only_unsafe_ids_starts_allocating_at_one() {
    let mut q = load(vec![row(MAX_ID + 2, "https://e.test/drop")]);
    assert!(q.items().is_empty());
    assert_eq!(q.add("https://e.test/drop").unwrap().id, 1);
}

#[test]
fn last_safe_id_survives_restart_and_exhaustion_is_nonmutating() {
    assert_eq!(MAX_ID, 9_007_199_254_740_991);
    let mut q = load(vec![row(MAX_ID - 1, "https://e.test/penultimate")]);
    let last = q.add("https://e.test/last").unwrap();
    assert_eq!(last.id, MAX_ID);
    let wire: Value = serde_json::from_slice(&serde_json::to_vec(&last).unwrap()).unwrap();
    assert_eq!(wire["id"].as_u64(), Some(MAX_ID));
    let mut q = Queue::from_json(&q.to_json()).unwrap();
    let before = q.to_json();
    let changes = q.changes();
    for _ in 0..3 {
        assert_eq!(
            q.add("https://e.test/overflow"),
            Err(AddError::Invalid {
                message: "The queue is full. Remove some rows to add more.".into()
            })
        );
        assert_eq!(q.to_json(), before);
        assert_eq!(q.changes(), changes);
    }
    q.remove(MAX_ID).unwrap();
    assert!(
        q.add("https://e.test/overflow").is_err(),
        "ids must never be reused in a session"
    );
}

#[test]
fn adjacent_safe_boundary_ids_remain_independently_addressable() {
    let mut q = load(vec![
        row(MAX_ID, "https://e.test/last"),
        row(MAX_ID - 1, "https://e.test/previous"),
    ]);
    let previous = q.items()[0].clone();
    q.rename(MAX_ID, "CON").unwrap();
    assert_eq!(q.items()[0], previous);
    assert_eq!(
        q.items()[1].name.as_deref(),
        Some(naming::sanitize("CON").as_str())
    );
    q.remove(MAX_ID - 1).unwrap();
    assert_eq!(q.items().len(), 1);
    assert_eq!(q.items()[0].id, MAX_ID);
}
