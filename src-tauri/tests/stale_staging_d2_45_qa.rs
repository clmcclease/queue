//! Independent QA for #45 / D2 / D-014 (plus D-017 saved-info cleanup).
//! Every path is disposable; never launch the app or use the owner's cache.
use queue_lib::{download::stale_work_dirs, meta};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "queue-stale-45-qa-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn cache(&self) -> PathBuf {
        self.0.join("cache 日本語 e\u{301}")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// The deletion loop in lib.rs setup (`download::remove_stale` per folder), applied only to our
// disposable fixtures.
fn delete_snapshot(snapshot: Vec<PathBuf>) {
    for dir in snapshot {
        let _ = queue_lib::download::remove_stale(&dir);
    }
}

fn put(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

#[test]
fn d45_cleanup_removes_nested_partials_without_touching_completed_downloads() {
    let f = Fixture::new();
    let cache = f.cache();
    let work = cache.join("downloads/item-42-1760000000000000000");
    for name in [
        "  日本語 🎵 (OFFICIAL VIDEO).f137.mp4.part",
        "e\u{301}.webm.part-Frag2",
        "_CON.mp4.ytdl",
        "nested/_NUL.txt.part",
        "queue-result.txt",
    ] {
        put(&work.join(name), b"interrupted");
    }
    let saved = meta::save_info(
        &meta::session_dir(&cache),
        42,
        "{\"title\":\"日本語\"}".as_bytes(),
    )
    .unwrap();
    let completed = f.0.join("completed 日本語/Title (2).webm");
    put(&completed, b"finished: keep exactly");
    let snapshot = stale_work_dirs(&cache);
    assert_eq!(snapshot.len(), 2);
    // Discovery must not delete or alter anything itself.
    assert!(work.join("nested/_NUL.txt.part").is_file());
    assert!(saved.is_file());
    delete_snapshot(snapshot);
    assert!(!work.exists());
    assert!(!saved.exists());
    assert_eq!(fs::read(completed).unwrap(), b"finished: keep exactly");
    assert!(stale_work_dirs(&cache).is_empty(), "cleanup is idempotent");
}

#[test]
fn d45_only_expected_direct_child_folders_are_candidates() {
    let f = Fixture::new();
    let cache = f.cache();
    let work = cache.join("downloads/item-1-123");
    let session = cache.join("info/session-1-123-0");
    fs::create_dir_all(&work).unwrap();
    fs::create_dir_all(&session).unwrap();
    let keep = [
        "downloads/session-1-123-0/keep",
        "info/item-1-123/keep",
        "downloads/Item-2/keep",
        "info/Session-2/keep",
        "downloads/unrelated/item-3/keep",
        "info/unrelated/session-3/keep",
        "item-4/keep",
    ];
    for path in keep {
        put(&cache.join(path), b"keep");
    }
    put(&cache.join("downloads/item-file"), b"keep file");
    put(&cache.join("info/session-file"), b"keep file");
    assert_eq!(stale_work_dirs(&cache), vec![work, session]);
    delete_snapshot(stale_work_dirs(&cache));
    for path in keep {
        assert_eq!(fs::read(cache.join(path)).unwrap(), b"keep");
    }
    assert_eq!(
        fs::read(cache.join("downloads/item-file")).unwrap(),
        b"keep file"
    );
    assert_eq!(
        fs::read(cache.join("info/session-file")).unwrap(),
        b"keep file"
    );
}

#[test]
fn d45_missing_cache_and_non_directory_containers_are_harmless() {
    let f = Fixture::new();
    let cache = f.cache();
    assert!(stale_work_dirs(&cache).is_empty());
    put(&cache.join("downloads"), b"not a directory");
    put(&cache.join("info/session-old/item-1.json"), b"{}");
    assert_eq!(
        stale_work_dirs(&cache),
        vec![cache.join("info/session-old")]
    );
    delete_snapshot(stale_work_dirs(&cache));
    fs::remove_dir(cache.join("info")).unwrap();
    put(&cache.join("info"), b"also not a directory");
    assert!(stale_work_dirs(&cache).is_empty());
    assert_eq!(
        fs::read(cache.join("downloads")).unwrap(),
        b"not a directory"
    );
    assert_eq!(
        fs::read(cache.join("info")).unwrap(),
        b"also not a directory"
    );
}

#[test]
fn d45_background_snapshot_preserves_work_created_after_discovery() {
    let f = Fixture::new();
    let cache = f.cache();
    put(&cache.join("downloads/item-1-100/old.part"), b"old");
    let old_session = meta::session_dir(&cache);
    meta::save_info(&old_session, 1, b"{}").unwrap();
    let snapshot = stale_work_dirs(&cache);
    // Same row id after restart, but fresh download/session names.
    let new_work = cache.join("downloads/item-1-200/current.part");
    put(&new_work, b"running download");
    let new_session = meta::session_dir(&cache);
    let new_info = meta::save_info(&new_session, 1, b"{\"new\":true}").unwrap();
    std::thread::spawn(move || delete_snapshot(snapshot))
        .join()
        .unwrap();
    assert!(!cache.join("downloads/item-1-100").exists());
    assert!(!old_session.exists());
    assert_eq!(fs::read(new_work).unwrap(), b"running download");
    assert_eq!(fs::read(new_info).unwrap(), b"{\"new\":true}");
}

#[test]
fn d45_a_disappearing_candidate_does_not_stop_the_remaining_cleanup() {
    let f = Fixture::new();
    let cache = f.cache();
    let a = cache.join("downloads/item-1-100");
    let b = cache.join("downloads/item-2-100");
    put(&a.join("a.part"), b"a");
    put(&b.join("b.part"), b"b");
    let snapshot = stale_work_dirs(&cache);
    fs::remove_dir_all(&a).unwrap();
    delete_snapshot(snapshot);
    assert!(!b.exists());
    assert!(stale_work_dirs(&cache).is_empty());
}

#[cfg(unix)]
#[test]
fn d45_leaf_and_nested_symlinks_never_delete_their_targets() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let cache = f.cache();
    let target = f.0.join("outside/precious.txt");
    put(&target, b"keep outside");
    let work = cache.join("downloads/item-1-100");
    let session = cache.join("info/session-1-100-0");
    fs::create_dir_all(&work).unwrap();
    fs::create_dir_all(&session).unwrap();
    for (container, prefix) in [("downloads", "item-"), ("info", "session-")] {
        let root = cache.join(container);
        symlink(target.parent().unwrap(), root.join(format!("{prefix}link"))).unwrap();
        symlink(&target, root.join(format!("{prefix}file-link"))).unwrap();
        symlink(f.0.join("missing"), root.join(format!("{prefix}dangling"))).unwrap();
    }
    symlink(target.parent().unwrap(), work.join("nested-dir-link")).unwrap();
    symlink(&target, session.join("nested-file-link")).unwrap();
    assert_eq!(stale_work_dirs(&cache), vec![work, session]);
    delete_snapshot(stale_work_dirs(&cache));
    assert_eq!(fs::read(target).unwrap(), b"keep outside");
    assert!(
        fs::symlink_metadata(cache.join("downloads/item-link"))
            .unwrap()
            .is_symlink()
    );
    assert!(
        fs::symlink_metadata(cache.join("info/session-dangling"))
            .unwrap()
            .is_symlink()
    );
}

#[cfg(unix)]
fn symlinked_container_must_preserve_outside(container: &str, prefix: &str) {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let cache = f.cache();
    fs::create_dir_all(&cache).unwrap();
    let outside = f.0.join("unrelated");
    let precious = outside.join(format!("{prefix}unrelated/precious.txt"));
    put(&precious, b"not Queue staging");
    symlink(&outside, cache.join(container)).unwrap();
    delete_snapshot(stale_work_dirs(&cache));
    assert_eq!(
        fs::read(&precious).ok().as_deref(),
        Some(b"not Queue staging".as_slice()),
        "cleanup followed the {container} container symlink and deleted unrelated data"
    );
}

#[cfg(unix)]
#[test]
fn d45_symlinked_downloads_container_must_not_delete_outside_data() {
    symlinked_container_must_preserve_outside("downloads", "item-");
}

#[cfg(unix)]
#[test]
fn d45_symlinked_info_container_must_not_delete_outside_data() {
    symlinked_container_must_preserve_outside("info", "session-");
}
