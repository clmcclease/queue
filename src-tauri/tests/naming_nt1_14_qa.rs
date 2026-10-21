//! Independent QA for #14 / N-T1. Exercise the public pure engine, metadata ingestion,
//! and the existing N2 placement boundary. No real network, model, or user folder is used.
use naming::{Template, name, sanitize};
use queue_lib::{download, meta, naming};
use std::{ffi::OsStr, fs, path::PathBuf};

fn titled(title: &str) -> meta::Meta {
    meta::Meta {
        title: Some(title.into()),
        ..Default::default()
    }
}

#[test]
fn nt1_messy_real_titles_preserve_the_subject_and_unicode() {
    for (input, expected) in [
        ("Björk — Jóga [4K] (Official Video)", "Björk — Jóga"),
        (
            "  宇多田ヒカル　-　光 【Official Audio】🔥 ",
            "宇多田ヒカル - 光",
        ),
        (
            "Sigur Rós - Svefn-g-englar (Live at the Museum)",
            "Sigur Rós - Svefn-g-englar (Live at the Museum)",
        ),
        (
            "Queen - Somebody To Love (Official Video) [HD] #SHORTS",
            "Queen - Somebody To Love",
        ),
        ("الأخبار — حلقة ١٢", "الأخبار — حلقة ١٢"),
    ] {
        assert_eq!(name(&titled(input), Template::Title), expected, "{input:?}");
    }
}

#[test]
fn nt1_emoji_only_brackets_leave_no_empty_noise() {
    for input in ["Song (🔥)", "Song [🎵]", "Song 【❤️】"] {
        assert_eq!(name(&titled(input), Template::Title), "Song", "{input:?}");
    }
}

#[test]
fn nt1_keycap_emoji_are_removed_as_whole_sequences() {
    for input in ["Song 1️⃣", "Song #️⃣", "Song *️⃣"] {
        assert_eq!(name(&titled(input), Template::Title), "Song", "{input:?}");
    }
}

#[test]
fn nt1_non_emoji_technical_symbols_keep_their_meaning() {
    for input in ["Keyboard shortcuts ⌘K", "Machining ⌀10 mm"] {
        assert_eq!(name(&titled(input), Template::Title), input);
    }
}

#[test]
fn nt1_official_noise_does_not_discard_live_version_information() {
    let input = "Song (Live at Wembley - Official Video)";
    // Removing the advertising words is fine; the performance's identity must survive.
    let output = name(&titled(input), Template::Title);
    assert!(
        output.contains("Live at Wembley"),
        "{input:?} became {output:?}"
    );
}

#[test]
fn nt1_channel_prefix_requires_a_word_boundary() {
    let metadata = meta::Meta {
        channel: Some("Art".into()),
        ..titled("Artificial Intelligence Explained")
    };
    assert_eq!(
        name(&metadata, Template::ChannelTitle),
        "Art - Artificial Intelligence Explained"
    );
}

#[test]
fn nt1_series_prefix_does_not_remove_part_of_the_episode_title() {
    let metadata = meta::Meta {
        series: Some("Dark".into()),
        season_number: Some(1),
        episode_number: Some(2),
        ..titled("Darkness Falls")
    };
    assert_eq!(
        name(&metadata, Template::Title),
        "Dark S01E02 - Darkness Falls"
    );
}

#[test]
fn nt1_reserved_devices_are_safe_with_extensions_case_and_spaces() {
    let mut devices: Vec<String> = ["CON", "prn", "AuX", "nul"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    for prefix in ["COM", "lpt"] {
        for digit in ["1", "2", "3", "4", "5", "6", "7", "8", "9", "¹", "²", "³"] {
            devices.push(format!("{prefix}{digit}"));
        }
    }
    for device in devices {
        for suffix in ["", ".mp3", " .archive.mp4", "..."] {
            let input = format!("{device}{suffix}");
            let output = sanitize(&input);
            let base = output.split('.').next().unwrap().trim_end();
            assert_ne!(
                base.to_lowercase(),
                device.to_lowercase(),
                "{input:?} -> {output:?}"
            );
            assert_eq!(sanitize(&output), output);
        }
    }
    for ordinary in ["CONCERT", "auxiliary", "LPT10", "COM1-song"] {
        assert_eq!(sanitize(ordinary), ordinary);
    }
}

#[test]
fn nt1_adversarial_ai_strings_cannot_create_paths_or_controls() {
    for input in [
        "../../NUL.txt\0",
        "C:\\temp\\CON",
        "../日本語/../../file",
        "\u{202e}fdp.exe\u{2066}",
        "\r\n\t",
        " . . ",
        "<script>\"x\"|?*",
        "\\\\server\\share\\file",
    ] {
        let output = sanitize(input);
        assert!(!output.is_empty(), "{input:?}");
        assert!(!output.contains(['<', '>', ':', '"', '/', '\\', '|', '?', '*']));
        assert!(!output.chars().any(char::is_control));
        assert!(!output.contains(['\u{202e}', '\u{2066}']));
        assert!(!output.ends_with(['.', ' ']));
        assert_eq!(sanitize(&output), output);
    }
    assert_eq!(sanitize("\r\n\t"), naming::UNTITLED);
}

#[test]
fn nt1_length_limits_hold_at_utf8_boundaries_for_every_template() {
    for seed in ["a", "é", "日", "𐐀", "e\u{301}"] {
        for count in [0, 149, 150, 151, 199, 200, 201, 501] {
            let metadata = meta::Meta {
                channel: Some(seed.repeat(count)),
                upload_date: Some("20261020".into()),
                ..titled(&seed.repeat(count))
            };
            for template in [Template::Title, Template::ChannelTitle, Template::DateTitle] {
                let output = name(&metadata, template);
                assert!(!output.is_empty());
                assert!(
                    output.len() <= naming::MAX_BYTES,
                    "{seed:?}, {count}, {template:?}"
                );
                assert!(output.chars().count() <= naming::MAX_CHARS);
                assert_eq!(sanitize(&output), output);
            }
        }
    }
}

#[test]
fn nt1_composed_and_decomposed_accents_are_preserved() {
    for title in ["Café", "Cafe\u{301}", "日本語 — 한국어"] {
        assert_eq!(name(&titled(title), Template::Title), title);
    }
}

#[test]
fn nt1_parsed_structured_fields_win_and_partial_fields_fall_back() {
    let music = meta::parse(
        br#"{"title":"Ignore me [4K]", "artists":["A", "B"], "track":"Tune (Official Audio)"}"#,
    )
    .unwrap();
    assert_eq!(name(&music, Template::Title), "A, B - Tune");
    let show = meta::parse(
        br#"{"title":"Show - Pilot", "series":"Show", "season_number":"02", "episode_number":"7"}"#,
    )
    .unwrap();
    assert_eq!(name(&show, Template::Title), "Show S02E07 - Pilot");
    for input in [
        br#"{"title":"Keep title", "artist":" ", "track":"Tune"}"#.as_slice(),
        br#"{"title":"Keep title", "artist":"A"}"#.as_slice(),
        br#"{"title":"Keep title", "track":"Tune"}"#.as_slice(),
        br#"{"title":"Keep title", "series":"Show"}"#.as_slice(),
    ] {
        assert_eq!(
            name(&meta::parse(input).unwrap(), Template::Title),
            "Keep title"
        );
    }
}

#[test]
fn nt1_odd_naming_metadata_does_not_fail_an_otherwise_valid_fetch() {
    for bad in [
        serde_json::Value::Null,
        serde_json::json!([]),
        serde_json::json!({"x":1}),
        serde_json::json!(true),
    ] {
        let json = serde_json::json!({
            "title":"Keep title", "artists":bad, "artist":bad, "track":bad,
            "series":bad, "season_number":bad, "episode_number":bad,
            "description":bad, "upload_date":bad,
        });
        let metadata = meta::parse(&serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(name(&metadata, Template::Title), "Keep title");
    }
    for bad in [
        serde_json::json!(-1),
        serde_json::json!(1.5),
        serde_json::json!(4294967296_u64),
        serde_json::json!("NaN"),
    ] {
        let json = serde_json::json!({"title":"Title", "season_number":bad, "episode_number":bad});
        let metadata = meta::parse(&serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(
            (metadata.season_number, metadata.episode_number),
            (None, None)
        );
    }
}

#[test]
fn nt1_all_new_metadata_survives_saved_json_roundtrip() {
    let input = serde_json::json!({
        "title":"T", "artists":["日本語", "Björk"], "track":"Café",
        "series":"S", "season_number":1, "episode_number":2,
        "description":"説明".repeat(301), "upload_date":"20261020",
    });
    let metadata = meta::parse(&serde_json::to_vec(&input).unwrap()).unwrap();
    assert_eq!(metadata.description.as_ref().unwrap().chars().count(), 300);
    let restored: meta::Meta =
        serde_json::from_slice(&serde_json::to_vec(&metadata).unwrap()).unwrap();
    assert_eq!(restored, metadata);
    for template in [Template::Title, Template::ChannelTitle, Template::DateTitle] {
        assert_eq!(name(&restored, template), name(&metadata, template));
    }
}

struct Sandbox(PathBuf);
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn nt1_titles_that_clean_to_the_same_name_keep_every_download() {
    let root = std::env::temp_dir().join(format!("queue-nt1-14-qa-{}", std::process::id()));
    // Deliberately fail rather than delete a pre-existing directory.
    fs::create_dir(&root).unwrap();
    let sandbox = Sandbox(root);
    let output = sandbox.0.join("output 日本語");
    fs::create_dir(&output).unwrap();
    for (index, title) in ["日本語 [4K]", "日本語 (Official Video)", "日本語 🔥"]
        .into_iter()
        .enumerate()
    {
        let source = sandbox.0.join(format!("source-{index}"));
        let payload = format!("payload {index}");
        fs::write(&source, &payload).unwrap();
        let stem = name(&titled(title), Template::Title);
        let filename = format!("{stem}.mp4");
        let saved = download::place_unique(&source, &output, OsStr::new(&filename)).unwrap();
        let expected = if index == 0 {
            "日本語.mp4".into()
        } else {
            format!("日本語 ({}).mp4", index + 1)
        };
        assert_eq!(saved, output.join(expected));
    }
    for (index, filename) in ["日本語.mp4", "日本語 (2).mp4", "日本語 (3).mp4"]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            fs::read_to_string(output.join(filename)).unwrap(),
            format!("payload {index}")
        );
    }
    assert_eq!(fs::read_dir(output).unwrap().count(), 3);
}
