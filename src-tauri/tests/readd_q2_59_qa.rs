//! Independent QA for #59 / Q2, with Q6 retry and N2 keep-both regressions.
//! No network, user folders, or production edits.
use queue_lib::{
    download,
    meta::Meta,
    queue::{AddError, Queue, Status, run_all},
    run_fetch_job,
};
use std::{
    ffi::OsStr,
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
        let root = std::env::temp_dir().join(format!(
            "queue-readd-59-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("out 日本語")).unwrap();
        Self(root)
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

fn done(q: &mut Queue, url: &str) -> u64 {
    let id = ready(q, url);
    q.start(id).unwrap();
    q.processing(id).unwrap();
    q.finish(id, Ok(format!("history-{id}.opus"))).unwrap();
    id
}

#[test]
fn active_copy_after_multiple_finished_rows_blocks_add_and_retry_without_mutation() {
    for (url, alias) in [
        (
            "https://www.youtube.com/watch?v=jNQXAC9IVRw",
            "https://music.youtube.com/watch?v=jNQXAC9IVRw&utm_source=qa",
        ),
        (
            "https://例え.jp/動画?q=%7e",
            "http://www.例え.jp/動画/?q=~#片段",
        ),
    ] {
        for state in 0..4 {
            let mut q = Queue::default();
            done(&mut q, url);
            done(&mut q, alias);
            let failed = q.add(url).unwrap().id;
            q.fetched(failed, Err("metadata timed out".into())).unwrap();
            let active = q.add(alias).unwrap().id;
            if state >= 1 {
                q.fetched(active, Ok(Meta::default())).unwrap();
            }
            if state >= 2 {
                q.start(active).unwrap();
            }
            if state >= 3 {
                q.processing(active).unwrap();
            }
            let before = q.items().to_vec();
            assert!(
                q.items()
                    .iter()
                    .find(|i| i.id == active)
                    .unwrap()
                    .is_active()
            );
            assert!(matches!(q.add(url), Err(AddError::Duplicate { id, .. }) if id == active));
            assert!(
                q.retry(failed)
                    .unwrap_err()
                    .contains("already in the queue")
            );
            assert_eq!(
                q.items(),
                before,
                "rejections preserve every row and revision"
            );
        }
    }
}

#[test]
fn readd_gets_fresh_metadata_and_info_without_inheriting_finished_row_state() {
    let f = Fixture::new();
    let mut q = Queue::default();
    let url = "https://example.test/music";
    let first = q.add(url).unwrap().id;
    let old_info = f.0.join("old-info.json");
    fs::write(&old_info, "{}").unwrap();
    let now = SystemTime::now();
    q.set_info(first, old_info.clone(), now);
    q.fetched(
        first,
        Ok(Meta {
            title: Some("CON 日本語 🎵 (OFFICIAL VIDEO)".into()),
            ..Meta::default()
        }),
    )
    .unwrap();
    q.start(first).unwrap();
    q.retrying(first, 3).unwrap();
    q.processing(first).unwrap();
    q.finish(first, Ok("old.opus".into())).unwrap();
    let history = q
        .noticed(first, "Original compatibility notice".into())
        .unwrap();

    let second = q.add(url).unwrap();
    assert!(second.id > first);
    assert_eq!(second.status, Status::Fetching);
    assert_eq!(second.meta, None);
    assert_eq!((second.rev, second.attempt), (1, 0));
    assert_eq!((second.progress, second.notice), (None, None));
    assert_eq!(q.fresh_info(second.id, now), None);
    assert_eq!(q.fresh_info(first, now), Some(old_info));

    let new_info = f.0.join("new-info.json");
    fs::write(&new_info, "{}").unwrap();
    let q = Mutex::new(q);
    let metadata = Meta {
        title: Some("NUL.txt – 新しいタイトル <script>".into()),
        ..Meta::default()
    };
    let fresh = run_fetch_job(&q, second.id, || {
        Ok((metadata.clone(), Some(new_info.clone())))
    })
    .unwrap();
    assert_eq!(fresh.status, Status::Queued);
    assert_eq!(fresh.meta, Some(metadata));
    let q = q.lock().unwrap();
    assert_eq!(q.fresh_info(second.id, SystemTime::now()), Some(new_info));
    assert_eq!(
        q.items()[0],
        history,
        "metadata and notices belong to their own row"
    );
}

#[test]
fn metadata_timeout_can_be_retried_beside_done_and_then_blocks_another_add() {
    let url = "https://youtu.be/jNQXAC9IVRw";
    let alias = "https://www.youtube.com/shorts/jNQXAC9IVRw";
    let mut q = Queue::default();
    let failed = q.add(url).unwrap().id;
    q.fetched(failed, Err("Getting info timed out.".into()))
        .unwrap();
    let finished = done(&mut q, alias);
    let history = q.items()[1].clone();
    let retried = q.retry(failed).unwrap();
    assert_eq!(retried.status, Status::Fetching);
    assert!(retried.is_active());
    assert!(matches!(q.add(alias), Err(AddError::Duplicate { id, .. }) if id == failed));
    assert_eq!(q.items()[1], history);
    assert!(!q.items()[1].is_active());
    assert_eq!(
        q.saved_file(finished).unwrap(),
        PathBuf::from(history_path(finished))
    );
    q.fetched(failed, Err("Getting info timed out again.".into()))
        .unwrap();
    assert!(!q.items()[0].is_active());
    assert!(
        q.add(alias).is_ok(),
        "failed and finished history do not block"
    );
}

fn history_path(id: u64) -> String {
    format!("history-{id}.opus")
}

#[test]
fn late_callbacks_for_a_finished_id_cannot_change_the_new_copy() {
    let mut q = Queue::default();
    let url = "https://example.test/video";
    let first = done(&mut q, url);
    let second = q.add(url).unwrap().id;
    let before = q.items().to_vec();
    assert_eq!(q.fetched(first, Err("late metadata error".into())), None);
    assert_eq!(q.progressed(first, download::Progress::default()), None);
    assert_eq!(q.retrying(first, 2), None);
    assert_eq!(q.processing(first), None);
    assert_eq!(q.finish(first, Err("late download error".into())), None);
    assert_eq!(q.items(), before);
    assert_eq!(q.items()[1].id, second);
}

#[test]
fn download_all_processes_readded_links_and_keeps_unicode_and_safe_reserved_filenames() {
    for name in [
        "日本語 🎵 é & 'quote' (OFFICIAL VIDEO).opus",
        "_CON.m4a",
        "_NUL.txt.webm",
    ] {
        let f = Fixture::new();
        let out = f.0.join("out 日本語");
        let q = Mutex::new(Queue::default());
        let url = "https://example.test/repeated";
        let mut ids = Vec::new();
        for n in 1..=3 {
            let id = ready(&mut q.lock().unwrap(), url);
            ids.push(id);
            let mut calls = 0;
            run_all(
                &q,
                |selected| {
                    calls += 1;
                    assert_eq!(selected, id, "finished history must never run again");
                    q.lock().unwrap().start(selected).unwrap();
                    let src = f.0.join("staged-media");
                    fs::write(&src, format!("download {n}")).unwrap();
                    let saved = download::place_unique(&src, &out, OsStr::new(name)).unwrap();
                    q.lock()
                        .unwrap()
                        .finish(selected, Ok(saved.to_str().unwrap().into()))
                        .unwrap();
                    true
                },
                || panic!("all metadata is ready and the download slot is free"),
            );
            assert_eq!(calls, 1);
            let expected = if n == 1 {
                name.into()
            } else {
                let (stem, ext) = name.rsplit_once('.').unwrap();
                format!("{stem} ({n}).{ext}")
            };
            assert_eq!(
                q.lock().unwrap().saved_file(id).unwrap(),
                out.join(expected)
            );
            for (index, previous) in ids.iter().enumerate() {
                let saved = q.lock().unwrap().saved_file(*previous).unwrap();
                assert_eq!(
                    fs::read(saved).unwrap(),
                    format!("download {}", index + 1).as_bytes()
                );
            }
        }
        assert_eq!(fs::read_dir(&out).unwrap().count(), 3);
        q.lock().unwrap().remove(ids[0]).unwrap();
        assert_eq!(
            fs::read_dir(&out).unwrap().count(),
            3,
            "removing history keeps its file"
        );
    }
}
