//! Independent QA for #5 / Q2: validation and duplicate identity, without network or Tauri IPC.
use queue_lib::{
    link,
    meta::Meta,
    queue::{AddError, Queue},
};

fn assert_duplicate(queue: &mut Queue, url: &str, id: u64) {
    let before = queue.items().to_vec();
    assert!(matches!(queue.add(url), Err(AddError::Duplicate { id: found, .. }) if found == id));
    assert_eq!(
        queue.items(),
        before,
        "a rejected duplicate must not change rows"
    );
}

#[test]
fn invalid_schemes_ports_and_multiline_input_leave_queue_unchanged() {
    let mut queue = Queue::default();
    let first = queue.add("https://example.test/first").unwrap();
    for invalid in [
        "data:text/plain,hello",
        "file://example.test/video",
        "ws://example.test/video",
        "https://example.test:65536/video",
        "https://[::1/video",
        "https://example.test/a\nhttps://example.test/b",
        "https://example.test/a\u{2003}b",
    ] {
        assert!(
            matches!(queue.add(invalid), Err(AddError::Invalid { .. })),
            "{invalid:?}"
        );
        assert_eq!(queue.items(), std::slice::from_ref(&first));
    }
    assert_eq!(
        queue.add("https://example.test/next").unwrap().id,
        first.id + 1
    );
}

#[test]
fn rejects_mailto_instead_of_reinterpreting_it_as_https_credentials() {
    let mut queue = Queue::default();
    let result = queue.add("mailto:user@example.test");
    assert!(
        matches!(result, Err(AddError::Invalid { .. })),
        "a mailto scheme must be rejected, not changed to https://mailto:user@example.test: {result:?}"
    );
    assert!(queue.items().is_empty());
}

#[test]
fn rejects_literal_nul_that_cannot_be_passed_to_a_subprocess() {
    let mut queue = Queue::default();
    let url = "https://example.test/video\0suffix";
    assert!(
        matches!(queue.add(url), Err(AddError::Invalid { .. })),
        "literal NUL must be an inline validation error, not a queued download that cannot launch"
    );
    assert!(queue.items().is_empty());
}

#[test]
fn unicode_host_and_path_spellings_share_identity_without_rewriting_display() {
    let mut queue = Queue::default();
    let typed = "https://例え.jp/日本語/🎵?title=混合e%CC%81";
    let first = queue.add(typed).unwrap();
    assert_eq!(first.url, typed);
    let ascii = url::Url::parse(typed).unwrap().to_string();
    assert_ne!(ascii, typed);
    assert_duplicate(&mut queue, &ascii, first.id);
}

#[test]
fn cosmetic_variants_keep_one_row_and_do_not_consume_ids() {
    let mut queue = Queue::default();
    let first = queue.add("https://example.test/video?a=1&b=2").unwrap();
    for duplicate in [
        "http://www.example.test:80/video/?a=1&b=2#comments",
        "https://EXAMPLE.TEST:443/video?utm_source=email&a=1&utm_medium=x&b=2",
        "example.test/video?a=1&b=2",
        "https://example.test/x/../video?a=1&b=2",
    ] {
        assert_duplicate(&mut queue, duplicate, first.id);
    }
    // #5 review round 2: on an unknown site, parameter order and non-utm parameters can select
    // different content, so these are new rows rather than duplicates.
    for distinct in [
        "https://example.test/video?b=2&a=1",
        "https://example.test/video?a=1&b=2&feature=share",
    ] {
        assert!(queue.add(distinct).is_ok(), "{distinct}");
    }
    assert_eq!(
        queue.add("https://example.test/next").unwrap().id,
        first.id + 3
    );
}

#[test]
fn all_supported_youtube_forms_point_to_the_same_row() {
    let mut queue = Queue::default();
    let first = queue
        .add("https://youtube.com/watch?v=AbCdEf_-123")
        .unwrap();
    for duplicate in [
        "https://youtu.be/AbCdEf_-123?t=42",
        "https://youtube.com/live/AbCdEf_-123",
        "https://youtube.com/embed/AbCdEf_-123",
        "https://youtube.com/v/AbCdEf_-123",
        "https://www.youtube-nocookie.com/embed/AbCdEf_-123",
        "https://music.youtube.com/watch?v=AbCdEf_-123",
        "https://m.youtube.com/shorts/AbCdEf_-123",
        "https://youtube.com/watch?v=AbCdEf_-123&list=PL123&index=2",
    ] {
        assert_duplicate(&mut queue, duplicate, first.id);
    }
}

#[test]
fn different_videos_playlists_channels_and_lookalike_hosts_are_not_duplicates() {
    let mut queue = Queue::default();
    for distinct in [
        "https://youtube.com/watch?v=AbCdEf_-123",
        "https://youtube.com/watch?v=abcdEf_-123",
        "https://youtube.com/playlist?list=PL123",
        "https://youtube.com/playlist?list=PL456",
        "https://youtube.com/@channel",
        "https://youtube.com.evil.test/watch?v=AbCdEf_-123",
        "https://notyoutube.com/watch?v=AbCdEf_-123",
        "https://youtu.be.evil.test/AbCdEf_-123",
    ] {
        queue.add(distinct).unwrap();
    }
    assert_eq!(queue.items().len(), 8);
}

#[test]
fn meaningful_path_port_and_query_changes_can_be_queued_together() {
    let mut queue = Queue::default();
    for distinct in [
        "https://example.test/CON?title=日本語%20(OFFICIAL%20VIDEO)",
        "https://example.test/con?title=日本語%20(OFFICIAL%20VIDEO)",
        "https://example.test/NUL?title=日本語%20(OFFICIAL%20VIDEO)",
        "https://example.test:8080/CON?title=日本語%20(OFFICIAL%20VIDEO)",
        "https://example.test/CON?title=日本語%20(OFFICIAL%20AUDIO)",
        "https://example.test/CON?title=日本語%20(OFFICIAL%20VIDEO)&page=2",
    ] {
        queue.add(distinct).unwrap();
    }
    assert_eq!(queue.items().len(), 6);
}

#[test]
fn encoded_ampersand_in_one_value_does_not_collide_with_another_parameter() {
    let mut queue = Queue::default();
    queue.add("https://example.test/video?a=1%26b%3D2").unwrap();
    let result = queue.add("https://example.test/video?a=1&b=2");
    assert!(
        result.is_ok(),
        "one opaque value and two separate parameters identify distinct targets: {result:?}"
    );
    assert_eq!(queue.items().len(), 2);
}

#[test]
fn encoded_equals_in_a_parameter_name_does_not_collide_with_a_value() {
    let mut queue = Queue::default();
    queue.add("https://example.test/video?a%3Db=c").unwrap();
    let result = queue.add("https://example.test/video?a=b%3Dc");
    assert!(
        result.is_ok(),
        "parameter (a=b, c) differs from (a, b=c): {result:?}"
    );
    assert_eq!(queue.items().len(), 2);
}

#[test]
fn query_escape_spellings_for_the_same_value_are_duplicates() {
    let mut queue = Queue::default();
    let first = queue
        .add("https://example.test/video?q=hello%20world&x=%26%3D")
        .unwrap();
    assert_duplicate(
        &mut queue,
        "https://example.test/video?q=hello%20world&x=%26%3d",
        first.id,
    );
}

/// #59 (owner, 2026-10-15): only an active row makes a link a duplicate. A finished row no
/// longer blocks it (e.g. to download it again in another format); before #59 it did.
#[test]
fn only_active_rows_block_duplicates_and_a_finished_link_can_be_added_again() {
    let mut queue = Queue::default();
    let url = "https://example.test/video";
    let first = queue.add(url).unwrap();
    assert_duplicate(&mut queue, url, first.id); // fetching
    queue.fetched(first.id, Ok(Meta::default())).unwrap(); // #6
    assert_duplicate(&mut queue, url, first.id); // queued
    queue.start(first.id).unwrap();
    assert_duplicate(&mut queue, url, first.id); // downloading
    queue.processing(first.id).unwrap();
    assert_duplicate(&mut queue, url, first.id); // processing
    queue.finish(first.id, Err("unavailable".into())).unwrap();
    let second = queue.add(url).unwrap();
    assert_duplicate(&mut queue, url, second.id);
    queue.fetched(second.id, Ok(Meta::default())).unwrap();
    queue.start(second.id).unwrap();
    queue
        .finish(second.id, Ok("C:\\Downloads\\CON.webm".into()))
        .unwrap();
    let third = queue.add(url).unwrap();
    assert!(third.id > second.id, "a finished link starts a new row");
    assert_duplicate(&mut queue, url, third.id);
    assert_eq!(queue.items().len(), 3);
}

#[test]
fn invalid_error_has_the_frontend_discriminator_and_no_row_id() {
    let error = Queue::default().add("not a link").unwrap_err();
    let encoded = serde_json::to_value(error).unwrap();
    assert_eq!(encoded["kind"], "invalid");
    assert!(!encoded["message"].as_str().unwrap().is_empty());
    assert!(encoded.get("id").is_none());
    assert!(link::parse("http://127.0.0.1:8080/video").is_ok());
    assert!(link::parse("https://[::1]/video").is_ok());
}
