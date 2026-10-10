//! The download queue (Q1, #3). Plain data with no Tauri types, so it's unit-testable and the UI
//! only sees it through commands. URL validation and duplicate rejection come in #5.
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Item {
    pub id: u64,
    pub url: String,
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
        };
        self.items.push(item.clone());
        Some(item)
    }

    pub fn items(&self) -> &[Item] {
        &self.items
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
        assert_eq!(
            q.add("  https://example.com/v?x=1  ").unwrap().url,
            "https://example.com/v?x=1"
        );
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
}
