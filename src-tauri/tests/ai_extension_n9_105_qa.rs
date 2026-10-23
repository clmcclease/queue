//! Independent N9 / #105 regression tests. No model, API, or user download folder.
use queue_lib::ai::{self, Reject};
use queue_lib::{download, meta::Meta, naming, queue};
use std::ffi::OsStr;
use std::path::PathBuf;

fn parse(name: &str, title: Option<&str>) -> Result<String, Reject> {
    ai::parse(&serde_json::json!({"filename": name}).to_string(), title)
}

// Each dash adds two bytes. The tail is exactly what that expansion cuts away, exposing ext.
fn boundary(ext: &str, dashes: usize) -> (String, String) {
    let padding = 200 - 100 - 5 * dashes - 1 - ext.len();
    let raw = format!(
        "{}{}{}.{}{}",
        "é".repeat(50),
        " - ".repeat(dashes),
        "a".repeat(padding),
        ext,
        "X".repeat(2 * dashes)
    );
    assert_eq!(raw.len(), naming::MAX_BYTES);
    assert_eq!(raw.chars().count(), naming::MAX_CHARS);
    let stem = format!(
        "{}{}{}",
        "é".repeat(50),
        " – ".repeat(dashes),
        "a".repeat(padding)
    );
    (raw, stem)
}

#[test]
fn n9_105_every_supported_extension_is_removed_after_byte_truncation() {
    for ext in [
        "mp4", "mkv", "webm", "m4a", "mp3", "opus", "wav", "flac", "mov", "avi", "aac", "ogg",
        "m4v",
    ] {
        for ext in [ext.to_owned(), ext.to_uppercase()] {
            let (raw, expected) = boundary(&ext, 1);
            assert_eq!(parse(&raw, None), Ok(expected), "extension {ext}");
        }
    }
}

#[test]
fn n9_105_multiple_dash_expansions_strip_only_the_exposed_extension() {
    // Keep dashes separated by a word so whitespace collapsing doesn't affect the byte math.
    for count in 1..=8 {
        let prefix = format!("{}{}", "é".repeat(50), " - b".repeat(count));
        let pad = 200 - prefix.len() - 2 * count - ".webm".len();
        let raw = format!("{prefix}{}.webm{}", "a".repeat(pad), "X".repeat(2 * count));
        assert_eq!(raw.len(), 200);
        assert_eq!(raw.chars().count(), 150);
        let expected = format!("{}{}", prefix.replace(" - ", " – "), "a".repeat(pad));
        let actual = parse(&raw, None).unwrap();
        assert_eq!(actual, expected, "{count} dashes");
        assert!(actual.len() <= naming::MAX_BYTES);
        assert_eq!(naming::sanitize(&actual), actual);
    }
}

#[test]
fn n9_105_nonmedia_extensions_and_uncut_media_like_endings_are_preserved() {
    let (raw, stem) = boundary("txt", 1);
    assert_eq!(parse(&raw, None), Ok(format!("{stem}.txt")));
    for raw in [
        "Artist - Title.mp4XY",
        "K-Pop - v1.2 notes",
        "Artist - Title.MP4X",
    ] {
        assert_eq!(parse(raw, None), Ok(raw.replace(" - ", " – ")));
    }
}

#[test]
fn n9_105_initial_unicode_byte_cap_can_also_expose_an_extension() {
    let stem = "日".repeat(65); // 195 bytes, with room for .webm at exactly 200.
    let raw = format!("{stem}.webmXY");
    assert!(raw.chars().count() <= naming::MAX_CHARS);
    assert_eq!(parse(&raw, Some("日本語")), Ok(stem));
}

#[test]
fn n9_105_hidden_extensions_and_reserved_names_stay_windows_safe() {
    for (raw, expected) in [
        ("CON.mp4\u{202e}", "CON_"),
        ("LPT³.MP3\u{2066}", "LPT³_"),
        ("nul .wav\t.", "nul_"),
        (
            " .. Café\t日本語 - A/B\\C:*? .MP4\u{202e}",
            "Café 日本語 – A⧸B⧹C：＊？",
        ),
    ] {
        let actual = parse(raw, None).unwrap();
        assert_eq!(actual, expected, "{raw:?}");
        assert_eq!(naming::sanitize(&actual), actual);
        assert!(!ai::has_media_extension(&actual));
    }
}

#[test]
fn n9_105_validation_still_runs_after_final_cleanup() {
    let (raw, _) = boundary("mp4", 1);
    assert_eq!(parse(&raw, Some("日本語")), Err(Reject::LostScript));
    for (raw, rejection) in [
        ("...\u{202e}", Reject::Empty),
        ("Untitled.mp4\u{202e}", Reject::Empty),
    ] {
        assert_eq!(parse(raw, None), Err(rejection));
    }
    assert_eq!(parse(&"a".repeat(151), None), Err(Reject::TooLong));
}

#[test]
fn n9_105_rejected_answer_preserves_the_rules_name_and_fallback_tag() {
    let mut q = queue::Queue::default();
    let id = q.add("https://example.test/105").unwrap().id;
    let metadata = Meta {
        title: Some("日本語 - Original".into()),
        ..Default::default()
    };
    let rules = naming::name(&metadata, naming::Template::Title);
    q.fetched(id, Ok(metadata)).unwrap();
    let (_, generation) = q.naming_job(id).unwrap();
    let (raw, _) = boundary("mp4", 1);
    let answer = parse(&raw, Some("日本語")).map_err(|e| format!("{e:?}"));
    let item = q.ai_named(id, generation, answer).unwrap();
    assert_eq!(item.name.as_deref(), Some(rules.as_str()));
    assert_eq!(item.name_source, Some(queue::NameSource::Fallback));
}

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn n9_105_boundary_hyphen_and_dash_variants_keep_both_downloads_without_double_extensions() {
    let (raw, expected) = boundary("mp4", 1);
    let hyphen = parse(&raw, None).unwrap();
    let dash = parse(&format!("{expected}.MP4"), None).unwrap();
    assert_eq!(hyphen, expected);
    assert_eq!(dash, expected);
    let root = Scratch(std::env::temp_dir().join(format!("queue-105-qa-{}", std::process::id())));
    let out = root.0.join("output");
    std::fs::create_dir_all(&out).unwrap();
    for (i, stem) in [hyphen, dash].into_iter().enumerate() {
        let source = root.0.join(format!("source-{i}"));
        let payload = format!("payload-{i}");
        std::fs::write(&source, &payload).unwrap();
        let placed =
            download::place_unique(&source, &out, OsStr::new(&format!("{stem}.webm"))).unwrap();
        let expected_file = if i == 0 {
            format!("{expected}.webm")
        } else {
            format!("{expected} (2).webm")
        };
        assert_eq!(placed.file_name().unwrap(), OsStr::new(&expected_file));
        assert_eq!(std::fs::read_to_string(placed).unwrap(), payload);
        assert!(!source.exists());
    }
    assert_eq!(
        std::fs::read_to_string(out.join(format!("{expected}.webm"))).unwrap(),
        "payload-0"
    );
    assert_eq!(std::fs::read_dir(out).unwrap().count(), 2);
}
