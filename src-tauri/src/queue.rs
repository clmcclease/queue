//! The download queue (Q1 #3, Q2 #5, D2 #4). Plain data with no Tauri types, so it's
//! unit-testable and the UI only sees it through commands and events.
use serde::Serialize;

use crate::link;
use crate::meta::Meta;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Status {
    /// Metadata (`yt-dlp -J`) is being fetched (Q3).
    Fetching,
    Queued,
    Downloading,
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
    /// Bumped on every change, so the UI can drop replies and events that arrive out of order.
    pub rev: u64,
    /// Duplicate identity from [`link::parse`]; internal only.
    #[serde(skip)]
    key: String,
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
    /// duplicate; a failed row doesn't block adding its link again (retry proper is #9).
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
            rev: 1,
            key: link.key,
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

    /// Adds a link and completes its metadata fetch, so it can be downloaded.
    fn ready(q: &mut Queue, url: &str) -> u64 {
        let id = q.add(url).unwrap().id;
        q.fetched(id, Ok(Meta::default())).unwrap();
        id
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
}
