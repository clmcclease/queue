//! The download queue (Q1 #3, Q2 #5, D2 #4). Plain data with no Tauri types, so it's
//! unit-testable and the UI only sees it through commands and events.
use serde::Serialize;

use crate::link;
use crate::meta::Meta;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// Saved `-J` info is used for the first download attempt while younger than this; YouTube's
/// media links expire about 6 h after extraction (D-017).
pub const INFO_FRESH_FOR: Duration = Duration::from_secs(5 * 60 * 60);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Status {
    /// Metadata (`yt-dlp -J`) is being fetched (Q3).
    Fetching,
    Queued,
    Downloading,
    /// yt-dlp is merging streams, extracting audio, or fixing up the file (D4).
    Processing,
    Done {
        path: String,
    },
    Failed {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Item {
    pub id: u64,
    pub url: String,
    pub status: Status,
    /// From `yt-dlp -J`, once fetched.
    pub meta: Option<Meta>,
    /// Live progress while downloading (D4, #8); cleared when the state changes.
    pub progress: Option<crate::download::Progress>,
    /// Bumped on every change, so the UI can drop replies and events that arrive out of order.
    pub rev: u64,
    /// Download attempt while downloading (1, then 2–3 after a 403 retry; D-017), else 0.
    pub attempt: u32,
    /// Duplicate identity from [`link::parse`]; internal only.
    #[serde(skip)]
    key: String,
    /// The saved `-J` info file and when it was fetched; internal only.
    #[serde(skip)]
    info: Option<(PathBuf, SystemTime)>,
}

impl Item {
    /// Forgets (and returns) the saved info file, e.g. of a row just removed from the queue.
    pub fn take_info(&mut self) -> Option<PathBuf> {
        self.info.take().map(|(p, _)| p)
    }
}

/// Why a link wasn't added. Serialized for the UI as `{ kind, message, id? }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AddError {
    /// Shown inline under the field; the text stays so it can be fixed.
    Invalid { message: String },
    /// Shown as a toast; `id` is the row that already has this link.
    Duplicate { message: String, id: u64 },
}

#[derive(Debug, Default)]
pub struct Queue {
    next_id: u64,
    items: Vec<Item>,
}

impl Queue {
    /// Appends a row for a valid, new link (Q2). A link matching a row that hasn't failed is a
    /// duplicate; a failed row doesn't block adding its link again (or it can be retried, Q6).
    pub fn add(&mut self, input: &str) -> Result<Item, AddError> {
        if input.trim().is_empty() {
            return Err(AddError::Invalid {
                message: "Enter a link to add.".into(),
            });
        }
        let link = link::parse(input).map_err(|message| AddError::Invalid { message })?;
        if let Some(existing) = self
            .items
            .iter()
            .find(|i| i.key == link.key && !matches!(i.status, Status::Failed { .. }))
        {
            return Err(AddError::Duplicate {
                message: "That link is already in the queue.".into(),
                id: existing.id,
            });
        }
        self.next_id += 1;
        let item = Item {
            id: self.next_id,
            url: link.text,
            status: Status::Fetching,
            meta: None,
            progress: None,
            rev: 1,
            attempt: 0,
            key: link.key,
            info: None,
        };
        self.items.push(item.clone());
        Ok(item)
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// Records the outcome of fetching an item's metadata: `Fetching` becomes `Queued` (with
    /// the metadata) or `Failed` (with yt-dlp's reason). Ignored for any other state.
    pub fn fetched(&mut self, id: u64, outcome: Result<Meta, String>) -> Option<Item> {
        let item = self.items.iter_mut().find(|i| i.id == id)?;
        if item.status != Status::Fetching {
            return None;
        }
        match outcome {
            Ok(meta) => {
                item.meta = Some(meta);
                item.status = Status::Queued;
            }
            Err(reason) => item.status = Status::Failed { reason },
        }
        item.rev += 1;
        Some(item.clone())
    }

    /// Marks a queued item as downloading. One download at a time (Q4 default): concurrent runs
    /// could merge onto the same title-based path; parallel downloads (#28) need reserved names.
    pub fn start(&mut self, id: u64) -> Result<Item, String> {
        if self.is_busy() {
            return Err("Another download is in progress. Try again when it finishes.".into());
        }
        let item = self
            .items
            .iter_mut()
            .find(|i| i.id == id)
            .ok_or("That item is no longer in the queue.")?;
        if item.status != Status::Queued {
            return Err("Only queued items can be downloaded.".into());
        }
        item.status = Status::Downloading;
        item.attempt = 1;
        item.progress = None;
        item.rev += 1;
        Ok(item.clone())
    }

    /// Records live progress for a downloading item (D4).
    pub fn progressed(&mut self, id: u64, progress: crate::download::Progress) -> Option<Item> {
        let item = self.items.iter_mut().find(|i| i.id == id)?;
        if item.status != Status::Downloading {
            return None;
        }
        item.progress = Some(progress);
        item.rev += 1;
        Some(item.clone())
    }

    /// Downloading → Processing (merge, audio extraction, fixups).
    pub fn processing(&mut self, id: u64) -> Option<Item> {
        let item = self.items.iter_mut().find(|i| i.id == id)?;
        if item.status != Status::Downloading {
            return None;
        }
        item.status = Status::Processing;
        item.progress = None;
        item.rev += 1;
        Some(item.clone())
    }

    /// Marks a downloading item's retry `attempt` (D-017), so the row can say so.
    pub fn retrying(&mut self, id: u64, attempt: u32) -> Option<Item> {
        let item = self.items.iter_mut().find(|i| i.id == id)?;
        if !matches!(item.status, Status::Downloading | Status::Processing) {
            return None;
        }
        item.status = Status::Downloading;
        item.attempt = attempt;
        item.progress = None;
        item.rev += 1;
        Some(item.clone())
    }

    /// Remembers the saved `-J` info for a row.
    pub fn set_info(&mut self, id: u64, path: PathBuf, fetched_at: SystemTime) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.info = Some((path, fetched_at));
        }
    }

    /// The saved info file if it's still fresh enough to download from at `now`.
    pub fn fresh_info(&self, id: u64, now: SystemTime) -> Option<PathBuf> {
        let (path, at) = self.items.iter().find(|i| i.id == id)?.info.as_ref()?;
        let age = now.duration_since(*at).ok()?; // a clock that went backwards: don't trust it
        (age < INFO_FRESH_FOR && path.is_file()).then(|| path.clone())
    }

    /// Forgets (and returns) a row's saved info, e.g. to delete the file once it's done.
    pub fn take_info(&mut self, id: u64) -> Option<PathBuf> {
        self.items
            .iter_mut()
            .find(|i| i.id == id)?
            .info
            .take()
            .map(|(p, _)| p)
    }

    /// Fails a Queued row that couldn't start for a lasting reason (e.g. no Downloads folder), so
    /// "Download all" moves on instead of retrying it forever.
    pub fn fail_queued(&mut self, id: u64, reason: String) -> Option<Item> {
        let item = self.items.iter_mut().find(|i| i.id == id)?;
        if item.status != Status::Queued {
            return None;
        }
        item.status = Status::Failed { reason };
        item.rev += 1;
        Some(item.clone())
    }

    /// Removes a row (Q6) and returns it, so the caller can delete its saved info. A download in
    /// progress can't be removed (cancel is #22). Removing a Done row leaves its file alone, and
    /// a row removed while Fetching just ignores the late result ([`Queue::fetched`]).
    pub fn remove(&mut self, id: u64) -> Result<Item, String> {
        let index = self
            .items
            .iter()
            .position(|i| i.id == id)
            .ok_or("That item is no longer in the queue.")?;
        if matches!(
            self.items[index].status,
            Status::Downloading | Status::Processing
        ) {
            return Err("A download in progress can't be removed yet.".into());
        }
        Ok(self.items.remove(index))
    }

    /// Retries a Failed row (Q6): back to Fetching if its info never arrived (the caller fetches
    /// it again), else Queued, to download again; the queue (and "Download all") picks it up.
    /// Refused if the same link was added again meanwhile and is still active.
    pub fn retry(&mut self, id: u64) -> Result<Item, String> {
        let item = self
            .items
            .iter()
            .find(|i| i.id == id)
            .ok_or("That item is no longer in the queue.")?;
        if !matches!(item.status, Status::Failed { .. }) {
            return Err("Only failed items can be retried.".into());
        }
        let key = item.key.clone();
        if self
            .items
            .iter()
            .any(|i| i.id != id && i.key == key && !matches!(i.status, Status::Failed { .. }))
        {
            return Err("That link is already in the queue again.".into());
        }
        let item = self
            .items
            .iter_mut()
            .find(|i| i.id == id)
            .expect("found above");
        item.status = if item.meta.is_some() {
            Status::Queued
        } else {
            Status::Fetching
        };
        item.progress = None;
        item.attempt = 0;
        item.rev += 1;
        Ok(item.clone())
    }

    /// The file a Done row saved (D7, for Show in folder). Looked up by row id, so the UI can
    /// only ever reveal files Queue itself saved, never an arbitrary path.
    pub fn saved_file(&self, id: u64) -> Result<PathBuf, String> {
        match &self
            .items
            .iter()
            .find(|i| i.id == id)
            .ok_or("That item is no longer in the queue.")?
            .status
        {
            Status::Done { path } => Ok(PathBuf::from(path)),
            _ => Err("Only finished items have a file to show.".into()),
        }
    }

    /// Whether a download holds the one slot (Q4: one at a time).
    pub fn is_busy(&self) -> bool {
        self.items
            .iter()
            .any(|i| matches!(i.status, Status::Downloading | Status::Processing))
    }

    /// Records the outcome of a download started with [`Queue::start`].
    pub fn finish(&mut self, id: u64, outcome: Result<String, String>) -> Option<Item> {
        let item = self.items.iter_mut().find(|i| i.id == id)?;
        if !matches!(item.status, Status::Downloading | Status::Processing) {
            return None;
        }
        item.progress = None;
        item.status = match outcome {
            Ok(path) => Status::Done { path },
            Err(reason) => Status::Failed { reason },
        };
        item.attempt = 0;
        item.rev += 1;
        Some(item.clone())
    }
}

/// "Download all" (Q4): downloads the next Queued row (lowest id first) until none are left.
/// Rows still fetching their info are waited for, but don't hold up rows that are ready. A
/// failed item doesn't stop the run (Q6). `download(id)` returns false if it couldn't start
/// (e.g. a manual download holds the one slot); the loop then `wait`s and tries again.
pub fn run_all(
    queue: &std::sync::Mutex<Queue>,
    mut download: impl FnMut(u64) -> bool,
    mut wait: impl FnMut(),
) {
    loop {
        let (next, pending) = match queue.lock() {
            Ok(q) => (
                q.items
                    .iter()
                    .find(|i| i.status == Status::Queued)
                    .map(|i| i.id),
                q.items.iter().any(|i| {
                    matches!(
                        i.status,
                        Status::Fetching | Status::Downloading | Status::Processing
                    )
                }),
            ),
            Err(_) => return,
        };
        match next {
            Some(id) if download(id) => {}
            Some(_) => wait(),
            None if pending => wait(),
            None => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Adds a link and completes its metadata fetch, so it can be downloaded.
    fn ready(q: &mut Queue, url: &str) -> u64 {
        let id = q.add(url).unwrap().id;
        q.fetched(id, Ok(Meta::default())).unwrap();
        id
    }

    #[test]
    fn remove_takes_any_row_except_a_download_in_progress() {
        let mut q = Queue::default();
        let fetching = q.add("https://e.test/fetching").unwrap().id;
        let queued = ready(&mut q, "https://e.test/queued");
        let busy = ready(&mut q, "https://e.test/busy");
        q.start(busy).unwrap();
        assert!(q.remove(busy).unwrap_err().contains("in progress"));
        q.processing(busy).unwrap();
        assert!(q.remove(busy).unwrap_err().contains("in progress"));
        q.finish(busy, Ok("/d/busy.webm".into())).unwrap();
        assert_eq!(q.remove(queued).unwrap().id, queued);
        assert_eq!(q.remove(fetching).unwrap().id, fetching);
        assert_eq!(
            q.fetched(fetching, Ok(Meta::default())),
            None,
            "late fetch ignored"
        );
        assert_eq!(q.remove(busy).unwrap().id, busy, "done rows can go");
        assert!(q.items().is_empty());
        assert!(q.remove(busy).unwrap_err().contains("no longer"));
        assert_eq!(
            q.add("https://e.test/queued").unwrap().id,
            4,
            "ids are never reused"
        );
    }

    #[test]
    fn retry_requeues_a_failed_download_and_refetches_a_failed_fetch() {
        let mut q = Queue::default();
        let fetch = q.add("https://e.test/fetch").unwrap().id;
        q.fetched(fetch, Err("timed out".into())).unwrap();
        let dl = ready(&mut q, "https://e.test/dl");
        q.start(dl).unwrap();
        q.retrying(dl, 2).unwrap();
        let failed = q.finish(dl, Err("HTTP Error 403".into())).unwrap();
        let r = q.retry(dl).unwrap();
        assert_eq!(r.status, Status::Queued);
        assert_eq!((r.attempt, r.progress.clone()), (0, None));
        assert!(r.rev > failed.rev);
        assert_eq!(q.retry(fetch).unwrap().status, Status::Fetching);
        assert!(
            q.fetched(fetch, Ok(Meta::default())).is_some(),
            "the new fetch lands"
        );
        assert!(q.retry(dl).unwrap_err().contains("Only failed"));
        assert!(q.retry(99).unwrap_err().contains("no longer"));
        q.start(dl).unwrap();
        q.finish(dl, Ok("/d/dl.webm".into())).unwrap();
        assert!(matches!(q.items()[1].status, Status::Done { .. }));
    }

    #[test]
    fn retry_is_refused_while_the_same_link_was_added_again() {
        let mut q = Queue::default();
        let old = ready(&mut q, "https://www.youtube.com/watch?v=jNQXAC9IVRw");
        q.start(old).unwrap();
        q.finish(old, Err("unavailable".into())).unwrap();
        let again = q.add("https://youtu.be/jNQXAC9IVRw").unwrap().id;
        assert!(q.retry(old).unwrap_err().contains("already in the queue"));
        q.fetched(again, Err("still unavailable".into())).unwrap();
        assert_eq!(
            q.retry(old).unwrap().status,
            Status::Queued,
            "both failed: ok"
        );
        assert!(matches!(
            q.add("https://youtu.be/jNQXAC9IVRw"),
            Err(AddError::Duplicate { id, .. }) if id == old
        ));
    }

    #[test]
    fn saved_file_is_only_for_done_rows() {
        let mut q = Queue::default();
        let id = ready(&mut q, "https://e.test/a");
        assert!(q.saved_file(id).unwrap_err().contains("Only finished"));
        q.start(id).unwrap();
        assert!(q.saved_file(id).unwrap_err().contains("Only finished"));
        q.finish(id, Ok("/d/日本語 (2).webm".into())).unwrap();
        assert_eq!(
            q.saved_file(id).unwrap(),
            PathBuf::from("/d/日本語 (2).webm")
        );
        let failed = ready(&mut q, "https://e.test/b");
        q.start(failed).unwrap();
        q.finish(failed, Err("no".into())).unwrap();
        assert!(q.saved_file(failed).is_err());
        assert!(q.saved_file(99).unwrap_err().contains("no longer"));
    }

    #[test]
    fn adds_rows_in_order_with_unique_ids() {
        let mut q = Queue::default();
        let a = q.add("https://example.com/a").unwrap();
        let b = q.add("https://example.com/b").unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(q.items(), &[a, b]);
    }

    #[test]
    fn trims_whitespace_and_rejects_blank_input() {
        let mut q = Queue::default();
        for blank in ["   \t\n", ""] {
            assert!(matches!(q.add(blank), Err(AddError::Invalid { .. })));
        }
        let item = q.add("  https://example.com/v?x=1  ").unwrap();
        assert_eq!(item.url, "https://example.com/v?x=1");
        assert_eq!(item.status, Status::Fetching);
        assert_eq!(q.items().len(), 1);
    }

    #[test]
    fn rejects_invalid_links_without_adding_a_row() {
        let mut q = Queue::default();
        assert!(matches!(q.add("not a url"), Err(AddError::Invalid { .. })));
        assert!(matches!(
            q.add("javascript:alert(1)"),
            Err(AddError::Invalid { .. })
        ));
        assert!(q.items().is_empty());
        assert_eq!(
            q.add("https://例え.jp/🎵").unwrap().url,
            "https://例え.jp/🎵"
        );
    }

    #[test]
    fn duplicates_point_at_the_existing_row_and_add_nothing() {
        let mut q = Queue::default();
        let first = q
            .add("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
            .unwrap();
        let dup = q.add("https://youtu.be/dQw4w9WgXcQ?si=x").unwrap_err();
        assert_eq!(
            dup,
            AddError::Duplicate {
                message: "That link is already in the queue.".into(),
                id: first.id
            }
        );
        assert_eq!(q.items().len(), 1);
        q.add("https://youtu.be/aaaaaaaaaaa").unwrap();
        assert_eq!(q.items().len(), 2);
    }

    #[test]
    fn a_failed_row_does_not_block_adding_its_link_again() {
        let mut q = Queue::default();
        let id = ready(&mut q, "https://example.com/v");
        q.start(id).unwrap();
        assert!(
            matches!(
                q.add("https://example.com/v"),
                Err(AddError::Duplicate { .. })
            ),
            "downloading"
        );
        q.finish(id, Err("network".into())).unwrap();
        assert_ne!(q.add("https://example.com/v").unwrap().id, id);
    }

    #[test]
    fn add_errors_serialize_with_a_kind_for_the_ui() {
        let json = serde_json::to_string(&AddError::Duplicate {
            message: "m".into(),
            id: 3,
        })
        .unwrap();
        assert_eq!(json, r#"{"kind":"duplicate","message":"m","id":3}"#);
        let item = Queue::default().add("https://example.com").unwrap();
        assert!(
            !serde_json::to_string(&item).unwrap().contains("key"),
            "key stays internal"
        );
    }

    #[test]
    fn download_moves_queued_to_downloading_to_done_bumping_rev() {
        let mut q = Queue::default();
        let id = ready(&mut q, "https://example.com/v");
        let started = q.start(id).unwrap();
        assert_eq!(
            (started.status.clone(), started.rev),
            (Status::Downloading, 3)
        );
        let done = q.finish(id, Ok("/d/Title.mp4".into())).unwrap();
        assert_eq!(
            done.status,
            Status::Done {
                path: "/d/Title.mp4".into()
            }
        );
        assert_eq!(done.rev, 4);
        assert_eq!(q.items()[0], done);
    }

    #[test]
    fn only_one_download_runs_at_a_time() {
        let mut q = Queue::default();
        let a = ready(&mut q, "https://example.com/a");
        let b = ready(&mut q, "https://example.com/b");
        q.start(a).unwrap();
        assert!(q.start(b).unwrap_err().contains("in progress"));
        assert_eq!(
            (q.items()[1].status.clone(), q.items()[1].rev),
            (Status::Queued, 2),
            "b untouched"
        );
        q.finish(a, Err("x".into())).unwrap();
        assert_eq!(q.start(b).unwrap().status, Status::Downloading);
    }

    #[test]
    fn failed_download_keeps_the_reason() {
        let mut q = Queue::default();
        let id = ready(&mut q, "https://example.com/v");
        q.start(id).unwrap();
        let failed = q.finish(id, Err("Video unavailable".into())).unwrap();
        assert_eq!(
            failed.status,
            Status::Failed {
                reason: "Video unavailable".into()
            }
        );
    }

    #[test]
    fn start_rejects_unknown_and_non_queued_items_and_finish_ignores_them() {
        let mut q = Queue::default();
        assert!(q.start(42).is_err());
        let id = ready(&mut q, "https://example.com/v");
        assert_eq!(q.finish(id, Ok("x".into())), None, "not started");
        q.start(id).unwrap();
        assert!(q.start(id).is_err(), "already downloading");
        q.finish(id, Ok("x".into())).unwrap();
        assert!(q.start(id).is_err(), "done");
        assert_eq!(q.finish(id, Err("late".into())), None, "already finished");
    }

    #[test]
    fn new_rows_fetch_info_then_become_queued_or_failed() {
        let mut q = Queue::default();
        let a = q.add("https://example.com/a").unwrap();
        assert_eq!(
            (a.status.clone(), a.meta.clone(), a.rev),
            (Status::Fetching, None, 1)
        );
        assert!(q.start(a.id).is_err(), "no download before the info is in");
        let meta = Meta {
            title: Some("A".into()),
            ..Meta::default()
        };
        let a = q.fetched(a.id, Ok(meta.clone())).unwrap();
        assert_eq!((a.status, a.meta, a.rev), (Status::Queued, Some(meta), 2));
        assert_eq!(q.fetched(a.id, Ok(Meta::default())), None, "only once");
        let b = q.add("https://example.com/b").unwrap().id;
        let b = q.fetched(b, Err("Unsupported URL".into())).unwrap();
        assert_eq!(
            b.status,
            Status::Failed {
                reason: "Unsupported URL".into()
            }
        );
        assert_eq!(q.fetched(99, Ok(Meta::default())), None);
    }

    #[test]
    fn saved_info_is_used_only_while_fresh_and_present() {
        let dir = std::env::temp_dir().join(format!("queue-info-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("item-1.json");
        std::fs::write(&file, "{}").unwrap();
        let mut q = Queue::default();
        let id = ready(&mut q, "https://example.com/v");
        let t0 = SystemTime::now();
        assert_eq!(q.fresh_info(id, t0), None, "nothing saved yet");
        q.set_info(id, file.clone(), t0);
        assert_eq!(
            q.fresh_info(id, t0 + Duration::from_secs(60)),
            Some(file.clone())
        );
        assert_eq!(q.fresh_info(id, t0 + INFO_FRESH_FOR), None, "expired");
        assert_eq!(
            q.fresh_info(id, t0 - Duration::from_secs(60)),
            None,
            "clock went back"
        );
        std::fs::remove_file(&file).unwrap();
        assert_eq!(q.fresh_info(id, t0), None, "file gone");
        assert_eq!(q.take_info(id), Some(file));
        assert_eq!(q.take_info(id), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn retry_attempts_show_while_downloading_and_reset_after() {
        let mut q = Queue::default();
        let id = ready(&mut q, "https://example.com/v");
        assert_eq!(q.retrying(id, 2), None, "not downloading yet");
        assert_eq!(q.start(id).unwrap().attempt, 1);
        let r = q.retrying(id, 2).unwrap();
        assert_eq!((r.attempt, r.status.clone()), (2, Status::Downloading));
        let done = q.finish(id, Ok("/d/x.webm".into())).unwrap();
        assert_eq!(done.attempt, 0);
        assert!(
            serde_json::to_string(&done)
                .unwrap()
                .contains("\"attempt\":0")
        );
        assert!(
            !serde_json::to_string(&done).unwrap().contains("info"),
            "info stays internal"
        );
    }

    mod runner {
        use super::*;
        use std::sync::Mutex;

        fn queued(urls: &[&str]) -> Mutex<Queue> {
            let mut q = Queue::default();
            for url in urls {
                ready(&mut q, url);
            }
            Mutex::new(q)
        }

        /// A download stand-in: start, then finish ok (or failed for urls containing "bad").
        fn fake_download(queue: &Mutex<Queue>, order: &mut Vec<u64>) -> impl FnMut(u64) -> bool {
            move |id| {
                let mut q = queue.lock().unwrap();
                if q.start(id).is_err() {
                    return false;
                }
                order.push(id);
                let bad = q
                    .items()
                    .iter()
                    .any(|i| i.id == id && i.url.contains("bad"));
                q.finish(
                    id,
                    if bad {
                        Err("x".into())
                    } else {
                        Ok(format!("/d/{id}"))
                    },
                );
                true
            }
        }

        #[test]
        fn downloads_every_queued_row_in_order_and_continues_past_failures() {
            let q = queued(&["https://e.test/1", "https://e.test/bad", "https://e.test/3"]);
            let mut order = Vec::new();
            run_all(&q, fake_download(&q, &mut order), || {
                panic!("no waiting needed")
            });
            assert_eq!(order, [1, 2, 3]);
            let states: Vec<_> = q
                .lock()
                .unwrap()
                .items()
                .iter()
                .map(|i| i.status.clone())
                .collect();
            assert!(matches!(states[1], Status::Failed { .. }));
            assert!(
                matches!(states[2], Status::Done { .. }),
                "a failure doesn't stop the run"
            );
        }

        #[test]
        fn waits_for_rows_still_fetching_and_then_downloads_them() {
            let q = Mutex::new(Queue::default());
            let a = q.lock().unwrap().add("https://e.test/a").unwrap().id;
            let mut order = Vec::new();
            let mut waits = 0;
            run_all(&q, fake_download(&q, &mut order), || {
                waits += 1;
                q.lock().unwrap().fetched(a, Ok(Meta::default())); // the info arrives
            });
            assert_eq!((order, waits), (vec![a], 1));
        }

        #[test]
        fn a_ready_row_is_not_held_up_by_an_earlier_row_still_fetching() {
            let q = Mutex::new(Queue::default());
            let slow = q.lock().unwrap().add("https://e.test/slow").unwrap().id;
            let fast = ready(&mut q.lock().unwrap(), "https://e.test/fast");
            let mut order = Vec::new();
            run_all(&q, fake_download(&q, &mut order), || {
                q.lock().unwrap().fetched(slow, Err("Private video".into()));
            });
            assert_eq!(order, [fast]);
        }

        #[test]
        fn a_row_that_cant_start_is_failed_and_the_run_moves_on() {
            let q = queued(&["https://e.test/1", "https://e.test/2"]);
            let mut order = Vec::new();
            let mut inner = fake_download(&q, &mut order);
            run_all(
                &q,
                |id| {
                    if id == 1 {
                        // like begin_download failing for a lasting reason with the slot free
                        assert!(!q.lock().unwrap().is_busy());
                        q.lock()
                            .unwrap()
                            .fail_queued(id, "No Downloads folder".into());
                        return true;
                    }
                    inner(id)
                },
                || panic!("a lasting failure must not be retried"),
            );
            drop(inner);
            assert_eq!(order, [2]);
            let mut q = q.lock().unwrap();
            assert!(
                matches!(&q.items()[0].status, Status::Failed { reason } if reason.contains("Downloads"))
            );
            assert_eq!(q.fail_queued(2, "x".into()), None, "only Queued rows");
        }

        #[test]
        fn waits_while_a_manual_download_holds_the_slot() {
            let q = queued(&["https://e.test/1", "https://e.test/2"]);
            q.lock().unwrap().start(1).unwrap(); // manual Download in progress
            let mut order = Vec::new();
            let mut waits = 0;
            run_all(&q, fake_download(&q, &mut order), || {
                waits += 1;
                q.lock().unwrap().finish(1, Ok("/d/1".into()));
            });
            assert_eq!((order, waits), (vec![2], 1));
        }

        #[test]
        fn rows_added_during_the_run_are_picked_up_and_an_empty_queue_returns() {
            let q = queued(&["https://e.test/1"]);
            let qr = &q;
            let mut order = Vec::new();
            let mut added = false;
            let mut inner = fake_download(qr, &mut order);
            run_all(
                qr,
                |id| {
                    let ok = inner(id);
                    if !added {
                        added = true;
                        ready(&mut qr.lock().unwrap(), "https://e.test/2");
                    }
                    ok
                },
                || panic!("no waiting needed"),
            );
            drop(inner);
            assert_eq!(order, [1, 2]);
            let empty = Mutex::new(Queue::default());
            let mut none = Vec::new();
            run_all(&empty, fake_download(&empty, &mut none), || panic!());
            assert!(none.is_empty());
        }
    }

    #[test]
    fn progress_and_processing_flow_then_finish() {
        use crate::download::Progress;
        let mut q = Queue::default();
        let id = ready(&mut q, "https://example.com/v");
        assert_eq!(
            q.progressed(id, Progress::default()),
            None,
            "not downloading yet"
        );
        q.start(id).unwrap();
        let p = Progress {
            percent: Some(42.0),
            part: 1,
            ..Progress::default()
        };
        assert_eq!(q.progressed(id, p.clone()).unwrap().progress, Some(p));
        let proc = q.processing(id).unwrap();
        assert_eq!(
            (proc.status.clone(), proc.progress),
            (Status::Processing, None)
        );
        assert!(q.is_busy(), "processing holds the one slot");
        assert_eq!(
            q.progressed(id, Progress::default()),
            None,
            "no progress while processing"
        );
        let done = q.finish(id, Ok("/d/v.webm".into())).unwrap();
        assert!(matches!(done.status, Status::Done { .. }));
        assert!(!q.is_busy());
    }
}
