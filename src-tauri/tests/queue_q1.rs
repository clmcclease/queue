//! Independent QA: issue #3 / Q1, updated for #5 / Q2 (validation and duplicates).
use queue_lib::queue::{AddError, Item, Queue};

#[test]
fn a_new_queue_is_empty() {
    assert!(Queue::default().items().is_empty());
}

#[test]
fn unicode_and_untrusted_text_in_valid_links_is_preserved_exactly() {
    let mut queue = Queue::default();
    for text in [
        "https://例え.jp/🎵?title=混合e\u{301}&spaced#片段",
        "https://example.test/CON/NUL/COM1?title=OFFICIAL%20VIDEO",
        "https://example.test/?x=<script>alert('x')</script>&a=\"b\"",
        "https://example.test/a%2Fb?x=1&x=2#fragment",
    ] {
        let item = queue.add(text).unwrap();
        assert_eq!(item.url, text);
        assert_eq!(queue.items().last(), Some(&item));
    }
}

#[test]
fn shell_like_text_and_links_with_inner_spaces_are_rejected() {
    let mut queue = Queue::default();
    for text in [
        "not a URL; $(echo nope) `nope` | &",
        "https://a.test/x https://b.test/y",
        "https://example.test/?title=a  b",
    ] {
        assert!(
            matches!(queue.add(text), Err(AddError::Invalid { .. })),
            "{text:?}"
        );
    }
    assert!(queue.items().is_empty());
}

#[test]
fn unicode_blank_input_does_not_consume_ids_or_change_rows() {
    let mut queue = Queue::default();
    let first = queue.add("https://example.test/a").unwrap();
    for blank in ["", " \t\r\n", "\u{a0}", "\u{2003}\u{3000}"] {
        assert!(matches!(queue.add(blank), Err(AddError::Invalid { .. })));
        assert_eq!(queue.items(), std::slice::from_ref(&first));
    }
    let second = queue.add("https://example.test/b").unwrap();
    assert_eq!(second.id, first.id + 1);
}

#[test]
fn trims_only_the_edges_of_nonblank_text() {
    let mut queue = Queue::default();
    assert_eq!(
        queue
            .add("\u{2003} https://example.test/?title=a%20b \u{3000}")
            .unwrap()
            .url,
        "https://example.test/?title=a%20b"
    );
}

#[test]
fn a_long_link_is_added_once_and_repeats_are_duplicates() {
    let mut queue = Queue::default();
    let long_url = format!("https://example.test/?title={}", "日本語🎵".repeat(2048));
    let first = queue.add(&long_url).unwrap();
    for _ in 0..5 {
        assert_eq!(
            queue.add(&long_url),
            Err(AddError::Duplicate {
                message: "That link is already in the queue.".into(),
                id: first.id
            })
        );
    }
    let items: Vec<Item> = queue.items().to_vec();
    assert_eq!(items, vec![first]);
    let next = queue.add("https://example.test/other").unwrap();
    assert_eq!(next.id, items[0].id + 1, "rejected adds don't consume ids");
}

#[test]
fn mutating_a_returned_item_cannot_change_the_stored_row() {
    let mut queue = Queue::default();
    let mut item = queue.add("https://example.test/original").unwrap();
    let original = item.clone();
    item.url.clear();
    item.id = 999;
    assert_eq!(queue.items(), &[original]);
}
