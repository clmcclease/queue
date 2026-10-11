//! The download queue (Q1 #3, D2 #4). Plain data with no Tauri types, so it's unit-testable and
//! the UI only sees it through commands and events. URL validation and duplicates come in #5.
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Status {
    Queued,
    Downloading,
    Done { path: String },
    Failed { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Item {
    pub id: u64,
    pub url: String,
    pub status: Status,
    /// Bumped on every change, so the UI can drop replies and events that arrive out of order.
    pub rev: u64,
}

#[derive(Debug, Default)]
pub struct Queue {
    next_id: u64,
    items: Vec<Item>,
}

impl Queue {
    /// Appends a row for `url`. Returns `None` for blank input; the text is otherwise kept raw.
    pub fn add(&mut self, url: &str) -> Option<Item> {
        let url = url.trim();
        if url.is_empty() {
            return None;
        }
        self.next_id += 1;
        let item = Item {
            id: self.next_id,
            url: url.to_owned(),
            status: Status::Queued,
            rev: 1,
        };
        self.items.push(item.clone());
        Some(item)
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// Marks a queued item as downloading. One download at a time (Q4 default): concurrent runs
    /// could merge onto the same title-based path; parallel downloads (#28) need reserved names.
    /// Retrying failed items is #9.
    pub fn start(&mut self, id: u64) -> Result<Item, String> {
        if self.items.iter().any(|i| i.status == Status::Downloading) {
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
        item.rev += 1;
        Ok(item.clone())
    }

    /// Records the outcome of a download started with [`Queue::start`].
    pub fn finish(&mut self, id: u64, outcome: Result<String, String>) -> Option<Item> {
        let item = self.items.iter_mut().find(|i| i.id == id)?;
        if item.status != Status::Downloading {
            return None;
        }
        item.status = match outcome {
            Ok(path) => Status::Done { path },
            Err(reason) => Status::Failed { reason },
        };
        item.rev += 1;
        Some(item.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_rows_in_order_with_unique_ids() {
        let mut q = Queue::default();
        let a = q.add("https://example.com/a").unwrap();
        let b = q.add("https://example.com/b").unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(q.items(), &[a, b]);
    }

    #[test]
    fn trims_whitespace_and_ignores_blank_input() {
        let mut q = Queue::default();
        assert_eq!(q.add("   \t\n"), None);
        assert_eq!(q.add(""), None);
        let item = q.add("  https://example.com/v?x=1  ").unwrap();
        assert_eq!(item.url, "https://example.com/v?x=1");
        assert_eq!(item.status, Status::Queued);
        assert_eq!(q.items().len(), 1);
    }

    #[test]
    fn keeps_raw_text_until_validation_lands() {
        let mut q = Queue::default();
        assert_eq!(q.add("not a url").unwrap().url, "not a url");
        assert_eq!(
            q.add("https://例え.jp/🎵").unwrap().url,
            "https://例え.jp/🎵"
        );
    }

    #[test]
    fn download_moves_queued_to_downloading_to_done_bumping_rev() {
        let mut q = Queue::default();
        let id = q.add("https://example.com/v").unwrap().id;
        let started = q.start(id).unwrap();
        assert_eq!(
            (started.status.clone(), started.rev),
            (Status::Downloading, 2)
        );
        let done = q.finish(id, Ok("/d/Title.mp4".into())).unwrap();
        assert_eq!(
            done.status,
            Status::Done {
                path: "/d/Title.mp4".into()
            }
        );
        assert_eq!(done.rev, 3);
        assert_eq!(q.items()[0], done);
    }

    #[test]
    fn only_one_download_runs_at_a_time() {
        let mut q = Queue::default();
        let a = q.add("https://example.com/a").unwrap().id;
        let b = q.add("https://example.com/b").unwrap().id;
        q.start(a).unwrap();
        assert!(q.start(b).unwrap_err().contains("in progress"));
        assert_eq!(
            (q.items()[1].status.clone(), q.items()[1].rev),
            (Status::Queued, 1),
            "b untouched"
        );
        q.finish(a, Err("x".into())).unwrap();
        assert_eq!(q.start(b).unwrap().status, Status::Downloading);
    }

    #[test]
    fn failed_download_keeps_the_reason() {
        let mut q = Queue::default();
        let id = q.add("https://example.com/v").unwrap().id;
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
        let id = q.add("https://example.com/v").unwrap().id;
        assert_eq!(q.finish(id, Ok("x".into())), None, "not started");
        q.start(id).unwrap();
        assert!(q.start(id).is_err(), "already downloading");
        q.finish(id, Ok("x".into())).unwrap();
        assert!(q.start(id).is_err(), "done");
        assert_eq!(q.finish(id, Err("late".into())), None, "already finished");
    }
}
