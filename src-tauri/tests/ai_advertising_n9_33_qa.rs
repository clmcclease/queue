//! Independent N9 / #33 QA for the advertising cleanup added on 2026-10-22.
//! Pure parser tests and a temporary-directory collision check; no models or external calls.
use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use queue_lib::{ai, download, naming};

fn parsed(name: &str) -> Result<String, ai::Reject> {
    ai::parse(&serde_json::json!({ "filename": name }).to_string(), None)
}

#[test]
fn n9_cleanup_rechecks_windows_devices_after_removing_advertising() {
    for (answer, expected) in [
        ("CON (Official Video)", "CON_"),
        ("nul – Official Music Video", "nul_"),
        ("COM¹ [Clip officiel]", "COM¹_"),
        ("LPT³ Official MV", "LPT³_"),
        ("aux.txt (Official Video)", "aux_.txt"),
    ] {
        assert_eq!(parsed(answer), Ok(expected.into()), "{answer}");
    }
}

#[test]
fn n9_cleanup_preserves_episode_numbers_featured_artists_and_version_notes() {
    for name in [
        "Show #2045 – Guest (Live at Studio 4)",
        "Series – S02E04 – Café (Director's Cut)",
        "Artist – Song (feat. A & B) (Remastered)",
        "K-Pop – 3-cup Chicken (2021)",
        "Björk – Jóga (Live in Reykjavík)",
    ] {
        assert_eq!(parsed(name), Ok(name.into()), "{name}");
    }
}

#[test]
fn n9_cleanup_is_idempotent_for_multilingual_messy_answers() {
    for raw in [
        "  Café\t–\t日本語 [Official Video] 🔥 ",
        "Кино - Группа крови (Официальный клип, 2018) [HQ]",
        "가수 - 노래 (Official Audio)",
        "Artist - Track (feat. A & B) #shorts",
    ] {
        let clean = parsed(raw).unwrap();
        assert_eq!(parsed(&clean), Ok(clean.clone()), "{raw}");
        assert_eq!(naming::sanitize(&clean), clean);
        assert!(clean.len() <= naming::MAX_BYTES);
    }
}

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn n9_distinct_advertised_answers_that_clean_to_one_stem_keep_both_files() {
    let root = Temp(std::env::temp_dir().join(format!(
        "queue-33-advertising-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    std::fs::create_dir(&root.0).unwrap();
    let out = root.0.join("out");
    std::fs::create_dir(&out).unwrap();
    let mut paths = Vec::new();
    for (i, raw) in [
        "Café - 日本語 (Official Video)",
        "Café – 日本語 [Clip officiel]",
    ]
    .into_iter()
    .enumerate()
    {
        let stem = parsed(raw).unwrap();
        assert_eq!(stem, "Café – 日本語");
        let source = root.0.join(format!("source-{i}"));
        std::fs::write(&source, format!("payload-{i}")).unwrap();
        paths.push(
            download::place_unique(&source, &out, OsStr::new(&format!("{stem}.mp4"))).unwrap(),
        );
    }
    assert_eq!(paths[0].file_name().unwrap(), "Café – 日本語.mp4");
    assert_eq!(paths[1].file_name().unwrap(), "Café – 日本語 (2).mp4");
    assert_eq!(std::fs::read_to_string(&paths[0]).unwrap(), "payload-0");
    assert_eq!(std::fs::read_to_string(&paths[1]).unwrap(), "payload-1");
}

#[test]
fn n9_nested_advertising_brackets_leave_no_orphaned_closer() {
    assert_eq!(
        parsed("Artist – Song (Official (Video))"),
        Ok("Artist – Song".into())
    );
}

#[test]
fn n9_an_unmatched_bracket_does_not_hide_later_advertising_groups() {
    assert_eq!(
        parsed("Artist – Song (Live [4K]"),
        Ok("Artist – Song (Live".into())
    );
}

#[test]
fn n9_the_actual_track_title_official_is_not_an_upload_suffix() {
    let name = "Artist – Official";
    assert_eq!(
        ai::parse(
            &serde_json::json!({ "filename": name }).to_string(),
            Some(name)
        ),
        Ok(name.into())
    );
}

#[test]
fn n9_enclosed_letters_and_chess_symbols_are_content_not_emoji() {
    let name = "Lesson 🄰 – Chess 🨀";
    assert_eq!(parsed(name), Ok(name.into()));
}

#[test]
fn n9_symbol_emoji_are_removed_as_well_as_supplementary_plane_emoji() {
    assert_eq!(
        parsed("Artist – Song ❤️ ☀️ ⚽ 🔥"),
        Ok("Artist – Song".into())
    );
}

#[test]
fn n9_keycap_emoji_are_removed_without_leaving_a_digit_or_combining_mark() {
    assert_eq!(parsed("Song 1️⃣"), Ok("Song".into()));
}

#[test]
fn n9_a_media_extension_exposed_by_advertising_cleanup_is_removed() {
    let clean = parsed("Artist – Song.mp4 (Official Video)").unwrap();
    assert_eq!(clean, "Artist – Song");
    assert!(!ai::has_media_extension(&clean));
}
