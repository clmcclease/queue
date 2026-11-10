//! Independent #104 / N9 QA against D-041. No model, credentials, or external network.
use queue_lib::adtags::{AdTags, DEFAULTS, MAX_CHARS, MAX_TAGS};
use queue_lib::ai::{self, Namer};
use queue_lib::meta::Meta;
use queue_lib::naming::{self, Template};
use queue_lib::queue::{NameSource, Queue};
use queue_lib::{download, run_naming_job};
use std::ffi::OsStr;
use std::sync::Mutex;

fn ready(q: &mut Queue, title: &str) -> u64 {
    let id = q
        .add(&format!("https://example.test/{}", q.items().len()))
        .unwrap()
        .id;
    q.fetched(
        id,
        Ok(Meta {
            title: Some(title.into()),
            channel: Some("Channel".into()),
            upload_date: Some("20261110".into()),
            ..Default::default()
        }),
    )
    .unwrap();
    id
}

fn answer(q: &mut Queue, id: u64, raw: &str) -> String {
    // Use the real AI parser before the queue, as the API/local namers do.
    let parsed = ai::parse(&serde_json::json!({"filename": raw}).to_string(), None).unwrap();
    let generation = q.naming_job(id).unwrap().1;
    q.ai_named(id, generation, Ok(parsed))
        .unwrap()
        .name
        .unwrap()
}

#[test]
fn every_starter_phrase_matches_each_bracket_pair_with_unicode_spacing_and_case() {
    let tags = AdTags::default();
    assert_eq!(tags.list().len(), 57);
    for phrase in DEFAULTS {
        for (open, close) in [
            ('(', ')'),
            ('[', ']'),
            ('（', '）'),
            ('［', '］'),
            ('【', '】'),
        ] {
            let spaced = phrase.to_uppercase().replace(' ', "\u{2003}\t");
            let title = format!("Björk 日本語 {open}\u{a0}{spaced}\u{a0}{close}");
            assert_eq!(tags.strip(&title), "Björk 日本語", "{title}");
        }
    }
}

#[test]
fn unknown_version_notes_and_whole_unknown_groups_are_preserved() {
    for title in [
        "LSD – Audio",
        "Song Official MV",
        "Song – Official MV",
        "Song 「MV」",
        "Song (Trailer)",
        "Song (Teaser)",
        "Song (Live at Wembley)",
        "Song (Remix)",
        "Song (Remastered)",
        "Song (Explicit)",
        "Song (Clean)",
        "Song (Performance ver.1)",
        "Song (Acoustic)",
        "Song (Extended)",
        "Song (2026)",
        "Song (Official Video Remix)",
        "Song (Live [MV])",
        "Song 【Live (MV)】",
        "Song (MV]",
        "Song (Official Video",
        "Song [Official Video)",
        "  Song\tunknown  ",
    ] {
        assert_eq!(AdTags::default().strip(title), title, "{title}");
    }
    assert_eq!(
        AdTags::default().strip("日本語 (Remix) [MV] (Live)"),
        "日本語 (Remix) (Live)"
    );
}

#[test]
fn empty_and_only_tag_answers_keep_the_answer_and_reset_is_reversible() {
    let mut q = Queue::default();
    let id = ready(&mut q, "Original");
    for raw in ["(Official Video)", "[4K] (MV)", "【MV】"] {
        assert_eq!(answer(&mut q, id, raw), raw);
    }
    assert_eq!(answer(&mut q, id, "Song (MV)"), "Song");
    assert_eq!(
        q.set_ad_tags(AdTags::new(Vec::<String>::new()))[0]
            .name
            .as_deref(),
        Some("Song (MV)")
    );
    assert_eq!(
        q.set_ad_tags(AdTags::default())[0].name.as_deref(),
        Some("Song")
    );
}

#[test]
fn limits_count_unicode_characters_and_deduplicate_after_tidying() {
    let hundred = "日".repeat(MAX_CHARS);
    // Typed in Settings (since review round 3 only typed text loses its surrounding brackets).
    let tags = [
        " [ Promo   Tag ] ",
        "promo tag",
        "",
        "\u{2003}",
        &hundred,
        &"日".repeat(MAX_CHARS + 1),
    ]
    .into_iter()
    .fold(AdTags::new(Vec::<String>::new()), |tags, phrase| {
        tags.with_added(phrase)
    });
    assert_eq!(tags.list(), ["Promo Tag".to_string(), hundred.clone()]);
    assert_eq!(tags.strip(&format!("Song ({hundred})")), "Song");
    let tags =
        AdTags::new((0..MAX_TAGS + 2).flat_map(|i| [format!("tag {i}"), format!("TAG {i}")]));
    assert_eq!(tags.list().len(), MAX_TAGS);
    assert_eq!(tags.strip("Song [tag 299] [tag 300]"), "Song [tag 300]");
}

#[test]
fn unreadable_setting_values_restore_defaults_without_losing_other_settings() {
    for invalid in [
        serde_json::Value::Null,
        serde_json::json!(42),
        serde_json::json!({}),
        serde_json::json!(["MV", null]),
        serde_json::json!([false]),
        serde_json::json!([["MV"]]),
    ] {
        let settings: queue_lib::settings::Settings = serde_json::from_value(serde_json::json!({
            "adTags": invalid, "format": "audio", "apiModel": "keep-me"
        }))
        .unwrap();
        assert_eq!(settings.ad_tags, AdTags::default());
        assert_eq!(settings.format, download::Format::Audio);
        assert_eq!(settings.api_model, "keep-me");
    }
    let settings: queue_lib::settings::Settings =
        serde_json::from_value(serde_json::json!({"adTags": []})).unwrap();
    assert!(settings.ad_tags.list().is_empty());
    assert_eq!(
        serde_json::to_value(settings).unwrap()["adTags"],
        serde_json::json!([])
    );
}

#[test]
fn list_changes_leave_rules_fallback_hand_edits_and_nonqueued_rows_alone() {
    let mut q = Queue::default();
    let rules = ready(&mut q, "Rules (Trailer)");
    let fallback = ready(&mut q, "Fallback (Trailer)");
    let generation = q.naming_job(fallback).unwrap().1;
    q.ai_named(fallback, generation, Err("timeout".into()))
        .unwrap();
    let edited = ready(&mut q, "Original");
    answer(&mut q, edited, "AI (Trailer)");
    q.rename(edited, "Mine (Trailer)").unwrap();
    let failed = ready(&mut q, "Original");
    answer(&mut q, failed, "Failed (Trailer)");
    q.fail_queued(failed, "fixture".into()).unwrap();
    let done = ready(&mut q, "Original");
    answer(&mut q, done, "Done (Trailer)");
    q.start(done).unwrap();
    q.finish(done, Ok("done.mp4".into())).unwrap();
    let running = ready(&mut q, "Original");
    answer(&mut q, running, "Running (Trailer)");
    q.start(running).unwrap();
    q.processing(running).unwrap();
    let before = q.items().to_vec();
    let changes = q.changes();
    assert!(q.set_ad_tags(AdTags::new(["Trailer"])).is_empty());
    assert_eq!(q.items(), before);
    assert_eq!(q.changes(), changes);
    assert_eq!(
        q.items()
            .iter()
            .find(|i| i.id == rules)
            .unwrap()
            .name_source,
        Some(NameSource::Rules)
    );
    assert_eq!(
        q.items()
            .iter()
            .find(|i| i.id == fallback)
            .unwrap()
            .name_source,
        Some(NameSource::Fallback)
    );
    assert_eq!(
        q.cancelled(running).unwrap().name.as_deref(),
        Some("Running")
    );
    assert_eq!(q.retry(failed).unwrap().name.as_deref(), Some("Failed"));
}

#[test]
fn pending_ai_answers_use_the_latest_list_without_a_second_request() {
    let q = Mutex::new(Queue::default());
    let id = ready(&mut q.lock().unwrap(), "Original");
    struct Pending<'a>(&'a Mutex<Queue>);
    impl Namer for Pending<'_> {
        fn name(&self, _: &Meta) -> Result<String, String> {
            self.0.lock().unwrap().set_ad_tags(AdTags::new(["Trailer"]));
            Ok("Song (Trailer) (MV)".into())
        }
    }
    let item = run_naming_job(&q, id, &Pending(&q)).unwrap();
    assert_eq!(item.name.as_deref(), Some("Song (MV)"));
}

#[test]
fn templates_restart_and_revision_updates_keep_the_unstripped_ai_answer() {
    let mut q = Queue::default();
    let id = ready(&mut q, "Original");
    answer(&mut q, id, "Song (MV)");
    let revision = q.items()[0].rev;
    let changes = q.changes();
    let changed = q.set_ad_tags(AdTags::new(Vec::<String>::new()));
    assert_eq!(changed[0].rev, revision + 1);
    assert_eq!(q.changes(), changes + 1);
    assert!(q.set_ad_tags(AdTags::new(Vec::<String>::new())).is_empty());
    q.set_template(Template::ChannelTitle);
    assert_eq!(q.items()[0].name.as_deref(), Some("Channel - Song (MV)"));
    let mut restored = Queue::from_json(&q.to_json()).unwrap();
    restored.set_template(Template::DateTitle);
    restored.set_ad_tags(AdTags::default());
    assert_eq!(restored.items()[0].name.as_deref(), Some("2026-11-10 Song"));
    restored.set_ad_tags(AdTags::new(Vec::<String>::new()));
    assert_eq!(
        restored.items()[0].name.as_deref(),
        Some("2026-11-10 Song (MV)")
    );
}

#[test]
fn removal_that_exposes_a_windows_device_name_is_sanitized_again() {
    let mut q = Queue::default();
    let id = ready(&mut q, "Original");
    for raw in ["CON", "NUL", "aux", "COM¹", "LPT³", "con.txt"] {
        assert_eq!(
            answer(&mut q, id, &format!("{raw} (MV)")),
            naming::sanitize(raw)
        );
    }
    assert_eq!(
        answer(&mut q, id, " .. Café 日本語/α\\β:*? (MV) "),
        "Café 日本語⧸α⧹β：＊？"
    );
    let long = format!("{} (MV)", "日".repeat(60));
    assert_eq!(answer(&mut q, id, &long), "日".repeat(60));
}

#[test]
fn newly_identical_names_still_get_a_collision_suffix_and_preserve_payloads() {
    let root = std::env::temp_dir().join(format!("queue-104-collision-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let result = std::panic::catch_unwind(|| {
        let mut q = Queue::default();
        let id = ready(&mut q, "Original");
        for (i, raw) in ["日本語 (MV)", "日本語 [4K]"].into_iter().enumerate() {
            let stem = answer(&mut q, id, raw);
            let src = root.join(format!("src-{i}"));
            std::fs::write(&src, format!("payload {i}")).unwrap();
            let path =
                download::place_unique(&src, &root, OsStr::new(&format!("{stem}.mp4"))).unwrap();
            let expected = if i == 0 {
                "日本語.mp4"
            } else {
                "日本語 (2).mp4"
            };
            assert_eq!(path.file_name().unwrap(), expected);
        }
        assert_eq!(
            std::fs::read_to_string(root.join("日本語.mp4")).unwrap(),
            "payload 0"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("日本語 (2).mp4")).unwrap(),
            "payload 1"
        );
    });
    std::fs::remove_dir_all(root).unwrap();
    result.unwrap();
}

#[test]
fn stripping_a_trailing_tag_must_not_leave_a_media_extension_in_the_stem() {
    let mut q = Queue::default();
    let id = ready(&mut q, "Original");
    assert_eq!(
        answer(&mut q, id, "Song.mp4 (Official Video)"),
        "Song",
        "the final AI stem must remain extension-free after advertising cleanup"
    );
}

#[test]
fn user_phrases_with_timestamps_match_their_sanitized_ai_representation() {
    let mut q = Queue::default();
    let id = ready(&mut q, "Original");
    q.set_ad_tags(AdTags::new(["Promo 01:02"]));
    assert_eq!(
        answer(&mut q, id, "Song (Promo 01:02)"),
        "Song",
        "AI sanitization maps a timestamp colon to underscore before tag matching"
    );
}
