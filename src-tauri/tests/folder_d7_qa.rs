//! Independent QA: #39 / D7. Tests saved-file lookup; never launches a file manager.
use queue_lib::{download, meta::Meta, queue::Queue, settings::Settings};
use std::{
    ffi::OsStr,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "queue-d7-qa-{}-{}",
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

#[test]
fn d7_lookup_rejects_every_non_done_state_without_mutating_rows() {
    for state in [
        "fetching",
        "queued",
        "downloading",
        "processing",
        "fetch_failed",
        "download_failed",
    ] {
        let mut q = Queue::default();
        let id = q.add("https://example.test/video").unwrap().id;
        if state == "fetch_failed" {
            q.fetched(id, Err("Getting info timed out".into())).unwrap();
        } else if state != "fetching" {
            q.fetched(id, Ok(Meta::default())).unwrap();
            if state != "queued" {
                q.start(id).unwrap();
                if state == "processing" {
                    q.processing(id).unwrap();
                } else if state == "download_failed" {
                    q.finish(id, Err("Not saved".into())).unwrap();
                }
            }
        }
        let before = q.items().to_vec();
        assert_eq!(
            q.saved_file(id).unwrap_err(),
            "Only finished items have a file to show.",
            "{state}"
        );
        assert_eq!(
            q.saved_file(u64::MAX).unwrap_err(),
            "That item is no longer in the queue."
        );
        assert_eq!(q.items(), before, "{state}");
    }
}

#[test]
fn d7_lookup_uses_id_and_preserves_exact_saved_paths_instead_of_metadata_titles() {
    let mut q = Queue::default();
    let mut saved = Vec::new();
    for (i, path) in [
        "/downloads/日本語 🎵 e\u{301} & 'quote' (2).webm",
        "C:\\Users\\Queue\\Downloads\\_CON (2).mp4",
        "\\\\server\\share\\Vidéos\\_NUL.m4a",
        "/downloads/$(literal) % <b>name</b>.opus",
    ]
    .into_iter()
    .enumerate()
    {
        let id = q.add(&format!("https://example.test/{i}")).unwrap().id;
        q.fetched(
            id,
            Ok(Meta {
                title: Some("CON 🔥 <script>wrong title</script>".into()),
                ..Default::default()
            }),
        )
        .unwrap();
        q.start(id).unwrap();
        q.finish(id, Ok(path.into())).unwrap();
        saved.push((id, PathBuf::from(path)));
    }
    q.remove(saved[1].0).unwrap(); // Row position is no longer its id.
    for (i, (id, path)) in saved.into_iter().enumerate() {
        if i == 1 {
            assert!(q.saved_file(id).is_err());
        } else {
            let mut returned = q.saved_file(id).unwrap();
            assert_eq!(returned, path);
            returned.push("caller-mutation");
            assert_eq!(
                q.saved_file(id).unwrap(),
                path,
                "lookup returns an owned copy"
            );
        }
    }
}

#[test]
fn d7_removed_done_id_cannot_reveal_a_readded_copy_of_the_same_link() {
    let mut q = Queue::default();
    let old = ready(&mut q, "https://example.test/video");
    q.start(old).unwrap();
    q.finish(old, Ok("/downloads/old.webm".into())).unwrap();
    q.remove(old).unwrap();
    let new = ready(&mut q, "https://example.test/video");
    q.start(new).unwrap();
    q.finish(new, Ok("/downloads/new (2).webm".into())).unwrap();
    assert_ne!(old, new);
    assert!(q.saved_file(old).is_err());
    assert_eq!(
        q.saved_file(new).unwrap(),
        PathBuf::from("/downloads/new (2).webm")
    );
}

#[test]
fn d7_real_collision_result_stays_attached_to_its_row_after_folder_changes() {
    let f = Fixture::new();
    let out = f.0.join("original 日本語");
    let next = f.0.join("new folder");
    fs::create_dir(&out).unwrap();
    fs::create_dir(&next).unwrap();
    let mut q = Queue::default();
    for (i, name) in ["日本語 e\u{301} & 'quote'.webm", "_CON.mp4", "_NUL.opus"]
        .into_iter()
        .enumerate()
    {
        fs::write(out.join(name), b"original bytes").unwrap();
        let source = f.0.join("source");
        fs::write(&source, b"new bytes").unwrap();
        let saved = download::place_unique(&source, &out, OsStr::new(name)).unwrap();
        assert!(
            saved
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .ends_with(" (2)")
        );
        let id = ready(&mut q, &format!("https://example.test/{i}"));
        q.start(id).unwrap();
        q.finish(id, Ok(saved.to_str().unwrap().into())).unwrap();
        let settings = Settings {
            download_dir: Some(next.clone()),
            ..Default::default()
        };
        assert_eq!(
            settings
                .download_dir(|| panic!("chosen folder must be used"))
                .unwrap(),
            next
        );
        assert_eq!(
            q.saved_file(id).unwrap(),
            saved,
            "reveal keeps the original saved location"
        );
        assert_eq!(fs::read(q.saved_file(id).unwrap()).unwrap(), b"new bytes");
        assert_eq!(fs::read(out.join(name)).unwrap(), b"original bytes");
    }
}

#[test]
fn d7_metadata_timeout_and_retry_have_no_saved_file_until_success() {
    let mut q = Queue::default();
    let id = q.add("https://example.test/video").unwrap().id;
    q.fetched(
        id,
        Err("Getting the video's info took longer than 90 seconds.".into()),
    )
    .unwrap();
    assert!(q.saved_file(id).is_err());
    q.retry(id).unwrap();
    assert!(q.saved_file(id).is_err());
    q.fetched(id, Ok(Meta::default())).unwrap();
    q.start(id).unwrap();
    assert!(q.saved_file(id).is_err());
    q.finish(id, Ok("/downloads/recovered.webm".into()))
        .unwrap();
    assert_eq!(
        q.saved_file(id).unwrap(),
        PathBuf::from("/downloads/recovered.webm")
    );
}

#[test]
fn d7_malformed_and_late_progress_never_grant_or_change_a_saved_file() {
    let mut q = Queue::default();
    let id = ready(&mut q, "https://example.test/video");
    q.start(id).unwrap();
    let mut finished = 0;
    for line in [
        "QUEUE-PROGRESS {broken",
        "QUEUE-PROGRESS null",
        r#"QUEUE-PROGRESS {"status":"finished","downloaded":"bad"}"#,
    ] {
        assert!(download::parse_line(line, &mut finished).is_none());
        assert!(q.saved_file(id).is_err());
    }
    let Some(download::Event::Progress(progress)) = download::parse_line(
        r#"QUEUE-PROGRESS {"status":"finished","downloaded":10,"total":10}"#,
        &mut finished,
    ) else {
        panic!("valid progress expected")
    };
    q.progressed(id, progress.clone()).unwrap();
    assert!(q.saved_file(id).is_err(), "100% still isn't Done");
    q.processing(id).unwrap();
    assert!(q.saved_file(id).is_err());
    q.finish(id, Ok("/downloads/日本語 (2).webm".into()))
        .unwrap();
    let before = q.items().to_vec();
    assert!(q.progressed(id, progress).is_none());
    assert_eq!(q.items(), before);
    assert_eq!(
        q.saved_file(id).unwrap(),
        PathBuf::from("/downloads/日本語 (2).webm")
    );
}
