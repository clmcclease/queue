//! Independent QA: issue #3 / Q1. Validation, filenames and metadata are later items.
use queue_lib::queue::{Item, Queue};

#[test]
fn a_new_queue_is_empty() {
    assert!(Queue::default().items().is_empty());
}

#[test]
fn raw_unicode_and_untrusted_text_is_preserved() {
    let mut queue = Queue::default();
    for text in [
        "https://例え.jp/🎵?title=混合 e\u{301} &  spaced#片段",
        "https://example.test/CON/NUL/COM1?title=OFFICIAL%20VIDEO",
        "https://example.test/?x=<script>alert('x')</script>&a=\"b\"",
        "https://example.test/a%2Fb?x=1&x=2#fragment",
        "not a URL; $(echo nope) `nope` | &",
    ] {
        let item = queue.add(text).unwrap();
        assert_eq!(item.url, text);
        assert_eq!(queue.items().last(), Some(&item));
    }
}

#[test]
fn unicode_blank_input_does_not_consume_ids_or_change_rows() {
    let mut queue = Queue::default();
    let first = queue.add("https://example.test/a").unwrap();
    for blank in ["", " \t\r\n", "\u{a0}", "\u{2003}\u{3000}"] {
        assert_eq!(queue.add(blank), None);
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
            .add("\u{2003} https://example.test/?title=a  b\u{a0}c \u{3000}")
            .unwrap()
            .url,
        "https://example.test/?title=a  b\u{a0}c"
    );
}

#[test]
fn long_inputs_and_repeated_urls_keep_order_and_unique_ids() {
    let mut queue = Queue::default();
    let long_url = format!("https://example.test/?title={}", "日本語🎵".repeat(2048));
    let expected: Vec<Item> = (0..128).map(|_| queue.add(&long_url).unwrap()).collect();
    assert_eq!(queue.items(), expected);
    for pair in expected.windows(2) {
        assert_eq!(pair[1].id, pair[0].id + 1);
    }
    assert!(expected.iter().all(|item| item.url == long_url));
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
