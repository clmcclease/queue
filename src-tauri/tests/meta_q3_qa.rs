//! Independent QA for #6 / Q3. No network or production-code changes.
use queue_lib::{
    meta,
    queue::{Queue, Status},
};
use serde_json::json;

#[test]
fn messy_unicode_titles_remain_display_metadata_not_filenames() {
    for title in [
        "日本語 🎵 e\u{301} – العربية",
        "CON",
        "NUL.txt",
        "COM1",
        "title<>:\"/\\|?*.",
        "<script>bad()</script>",
    ] {
        let bytes = serde_json::to_vec(&json!({"title": title})).unwrap();
        assert_eq!(meta::parse(&bytes).unwrap().title.as_deref(), Some(title));
    }
    let bytes = serde_json::to_vec(
        &json!({"title":" \t日本語\u{0}\n 🎵\r ", "channel":"\u{7f}\t", "uploader":" 作者 "}),
    )
    .unwrap();
    let parsed = meta::parse(&bytes).unwrap();
    assert_eq!(parsed.title.as_deref(), Some("日本語 🎵"));
    assert_eq!(parsed.channel.as_deref(), Some("作者"));
}

#[test]
fn unicode_limits_do_not_split_utf8_and_all_display_fields_are_bounded() {
    let long = "🎵".repeat(600);
    let bytes =
        serde_json::to_vec(&json!({"title":long,"channel":long,"extractor_key":long})).unwrap();
    let parsed = meta::parse(&bytes).unwrap();
    for field in [parsed.title, parsed.channel, parsed.site] {
        assert_eq!(field.unwrap(), "🎵".repeat(500));
    }
}

#[test]
fn null_and_blank_fields_are_optional_and_channel_wins_over_uploader() {
    assert_eq!(
        meta::parse(br#"{"title":null,"channel":null,"duration":null,"thumbnail":null}"#).unwrap(),
        meta::Meta::default()
    );
    let parsed =
        meta::parse(br#"{"title":"  ","channel":" C ","uploader":"U","extractor_key":"\t"}"#)
            .unwrap();
    assert_eq!(parsed.title, None);
    assert_eq!(parsed.channel.as_deref(), Some("C"));
    assert_eq!(parsed.site, None);
}

#[test]
fn malformed_json_and_wrong_field_types_fail_readably() {
    for bytes in [
        b"".as_slice(),
        b"null",
        b"[]",
        b"{",
        b"{}{}",
        b"{\"title\":42}",
        b"{\"duration\":\"19\"}",
        b"{\"thumbnail\":[]}",
        b"{\"title\":\"\xff\"}",
        b"[download] 50%\n{}",
    ] {
        assert!(
            meta::parse(bytes).unwrap_err().contains("unreadable"),
            "{bytes:?}"
        );
    }
}

#[test]
fn multi_video_is_refused_like_playlist() {
    assert!(
        meta::parse(br#"{"_type":"multi_video","entries":[]}"#)
            .unwrap_err()
            .contains("Playlists")
    );
}

#[test]
fn fractional_zero_and_negative_durations_are_handled() {
    for (input, expected) in [
        (0.0, Some(0.0)),
        (0.5, Some(0.5)),
        (3661.25, Some(3661.25)),
        (-0.5, None),
    ] {
        let bytes = serde_json::to_vec(&json!({"duration":input})).unwrap();
        assert_eq!(meta::parse(&bytes).unwrap().duration, expected);
    }
}

#[test]
fn thumbnail_schemes_are_limited_to_http_and_https() {
    for thumbnail in [
        "javascript:alert(1)",
        "data:image/svg+xml,<svg/>",
        "file:///tmp/private",
        "//example.test/a.jpg",
        "ftp://example.test/a.jpg",
    ] {
        let bytes = serde_json::to_vec(&json!({"thumbnail":thumbnail})).unwrap();
        assert_eq!(meta::parse(&bytes).unwrap().thumbnail, None);
    }
    for thumbnail in [
        "https://example.test/a.jpg?token=a%2Bb&x=2",
        "HTTP://example.test/a.jpg",
    ] {
        let bytes = serde_json::to_vec(&json!({"thumbnail":thumbnail})).unwrap();
        assert_eq!(
            meta::parse(&bytes).unwrap().thumbnail.as_deref(),
            Some(thumbnail)
        );
    }
}

#[test]
fn long_signed_thumbnail_urls_are_preserved_byte_for_byte() {
    // Signed CDN URLs are identifiers: truncation invalidates their signature/path.
    let thumbnail = format!(
        "https://cdn.example.test/image.jpg?token={}&signature=valid",
        "a".repeat(700)
    );
    let bytes = serde_json::to_vec(&json!({"thumbnail":thumbnail})).unwrap();
    assert_eq!(
        meta::parse(&bytes).unwrap().thumbnail.as_deref(),
        Some(thumbnail.as_str())
    );
}

#[test]
fn metadata_serialization_keeps_optional_fields_and_unicode() {
    let bytes = serde_json::to_vec(&json!({"title":"日本語 🎵", "duration":0})).unwrap();
    let meta = meta::parse(&bytes).unwrap();
    let mut queue = Queue::default();
    let id = queue.add("https://example.test/a").unwrap().id;
    let item = queue.fetched(id, Ok(meta)).unwrap();
    let wire = serde_json::to_value(item).unwrap();
    assert_eq!(wire["status"]["state"], "queued");
    assert_eq!(wire["meta"]["title"], "日本語 🎵");
    assert_eq!(wire["meta"]["duration"], 0.0);
    assert!(wire["meta"]["thumbnail"].is_null());
    assert!(wire.get("key").is_none());
}

#[test]
fn same_metadata_titles_do_not_merge_distinct_links() {
    let mut queue = Queue::default();
    let first = queue.add("https://example.test/a").unwrap().id;
    let second = queue.add("https://example.test/b").unwrap().id;
    let meta = meta::parse(br#"{"title":"CON"}"#).unwrap();
    queue.fetched(first, Ok(meta.clone())).unwrap();
    queue.fetched(second, Ok(meta)).unwrap();
    assert_eq!(queue.items().len(), 2);
    assert!(
        queue
            .items()
            .iter()
            .all(|item| item.status == Status::Queued)
    );
}

#[test]
fn failed_metadata_can_be_readded_and_does_not_block_another_item() {
    let mut queue = Queue::default();
    let first = queue.add("https://example.test/a").unwrap().id;
    let second = queue.add("https://example.test/b").unwrap().id;
    let failed = queue
        .fetched(first, Err("Video unavailable".into()))
        .unwrap();
    assert_eq!(failed.rev, 2);
    assert_eq!(failed.meta, None);
    assert_ne!(queue.add("https://example.test/a").unwrap().id, first);
    queue.fetched(second, Ok(meta::Meta::default())).unwrap();
    assert_eq!(queue.start(second).unwrap().status, Status::Downloading);
}

#[test]
fn late_metadata_cannot_change_failed_downloading_or_done_rows() {
    let mut queue = Queue::default();
    let id = queue.add("https://example.test/a").unwrap().id;
    assert!(queue.finish(id, Ok("unexpected".into())).is_none());
    queue.fetched(id, Ok(meta::Meta::default())).unwrap();
    queue.start(id).unwrap();
    let before = queue.items().to_vec();
    assert!(queue.fetched(id, Err("late".into())).is_none());
    assert_eq!(queue.items(), before);
    queue.finish(id, Ok("out/a.webm".into())).unwrap();
    let before = queue.items().to_vec();
    assert!(queue.fetched(id, Ok(meta::Meta::default())).is_none());
    assert_eq!(queue.items(), before);
    let id = queue.add("https://example.test/b").unwrap().id;
    queue.fetched(id, Err("unavailable".into())).unwrap();
    let before = queue.items().to_vec();
    assert!(queue.fetched(id, Ok(meta::Meta::default())).is_none());
    assert_eq!(queue.items(), before);
}

#[cfg(unix)]
mod subprocess {
    use super::*;
    use queue_lib::sidecar::Sidecar;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
        time::{Duration, Instant},
    };

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fake(PathBuf);
    impl Fake {
        fn new(body: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "queue-q3-qa-{}-{} 日本語 spaces",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            let path = Sidecar::YtDlp.path_in(&root);
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(root)
        }
        fn fetch(&self) -> Result<meta::Meta, String> {
            meta::fetch(&self.0, "https://example.test/v", Duration::from_secs(5))
        }
    }
    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn argv_keeps_untrusted_url_literal_and_metadata_only() {
        let fake =
            Fake::new("printf '%s\\n' \"$@\" > \"$0.args\"\nprintf '%s' '{\"title\":\"OK\"}'");
        let url = "https://example.test/日本語?x=$(touch SHOULD_NOT_EXIST)&title=CON";
        meta::fetch(&fake.0, url, Duration::from_secs(5)).unwrap();
        let captured = fs::read_to_string(fake.0.join("queue-yt-dlp.args")).unwrap();
        let args: Vec<_> = captured.lines().collect();
        assert_eq!(
            &args[args.len() - 5..],
            &["--no-playlist", "--flat-playlist", "-J", "--", url]
        );
        assert!(!args.contains(&"-o") && !args.contains(&"-f"));
        assert!(args.contains(&"--ignore-config") && args.contains(&"--no-update"));
    }

    #[test]
    fn malformed_and_progress_polluted_stdout_fail_readably() {
        for body in [
            "printf ''",
            "printf '[download] 50%%\\n{}'",
            "printf '{\"title\":'",
            "printf '{}\\n{}'",
        ] {
            assert!(Fake::new(body).fetch().unwrap_err().contains("unreadable"));
        }
    }

    #[test]
    fn nonzero_exit_wins_even_when_stdout_has_valid_metadata() {
        let fake = Fake::new(
            "printf '%s' '{\"title\":\"Must not queue\"}'\necho 'ERROR: Private video' >&2\nexit 1",
        );
        assert_eq!(fake.fetch().unwrap_err(), "Private video");
    }

    #[test]
    fn missing_sidecar_has_a_readable_start_error() {
        let fake = Fake::new("exit 0");
        fs::remove_file(Sidecar::YtDlp.path_in(&fake.0)).unwrap();
        assert!(
            fake.fetch()
                .unwrap_err()
                .starts_with("Couldn't start yt-dlp:")
        );
    }

    #[test]
    fn killing_a_direct_process_on_timeout_allows_a_subsequent_fetch() {
        let fake = Fake::new("exec sleep 5");
        let started = Instant::now();
        assert!(
            meta::fetch(
                &fake.0,
                "https://example.test/v",
                Duration::from_millis(200)
            )
            .unwrap_err()
            .contains("took longer")
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(
            Fake::new("printf '%s' '{\"title\":\"Next\"}'")
                .fetch()
                .unwrap()
                .title
                .as_deref(),
            Some("Next")
        );
    }

    #[test]
    fn time_limit_also_bounds_pipe_drain_after_parent_exits() {
        // Finite descendant keeps inherited stdout/stderr open; it exits on its own.
        let fake = Fake::new("sleep 2 &\nprintf '%s' '{\"title\":\"Parent finished\"}'\nexit 0");
        let started = Instant::now();
        let result = meta::fetch(
            &fake.0,
            "https://example.test/v",
            Duration::from_millis(200),
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(1),
            "200 ms fetch limit exceeded while draining pipes: {elapsed:?}, result: {result:?}"
        );
    }
}
