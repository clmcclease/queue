//! Independent #33(b) / N9 collector regressions. No real sidecars, network, or model.
use super::{app_layout, collect, links, write_new};
#[cfg(unix)]
use serde_json::Value;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "queue-33b-independent-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    #[cfg(unix)]
    fn script(&self, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = queue_lib::sidecar::Sidecar::YtDlp.path_in(&self.0);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn n9_collect_links_handles_crlf_comments_unicode_and_url_fragments() {
    assert_eq!(
        links(
            " \r\n\t# skip me\r\n https://例.example/日本?q=a%20b#part \r\n\thttps://e.test/b\t\n"
        ),
        ["https://例.example/日本?q=a%20b#part", "https://e.test/b"]
    );
    assert!(links("\n  # comment\r\n\t").is_empty());
}

#[test]
fn n9_collect_empty_input_needs_no_executable() {
    let f = Fixture::new();
    assert_eq!(collect(&f.0, &[]), (vec![], vec![]));
}

#[test]
fn n9_collect_missing_executable_reports_each_link_without_aborting() {
    let f = Fixture::new();
    let urls = ["https://e.test/a", "https://e.test/b"];
    let (entries, failed) = collect(&f.0, &urls);
    assert!(entries.is_empty());
    assert_eq!(failed.len(), 2);
    for ((url, reason), expected) in failed.iter().zip(urls) {
        assert_eq!(url, expected);
        assert!(reason.starts_with("Couldn't start yt-dlp:"), "{reason}");
    }
}

#[cfg(unix)]
#[test]
fn n9_collect_preserves_messy_unicode_reserved_titles_and_structured_fields() {
    let f = Fixture::new();
    let raw = json!({
        "title": "  CON / Café 日本語 🎵 [4K]  ",
        "channel": "Björk", "description": "日".repeat(350),
        "artists": ["Björk", "宇多田ヒカル"], "track": "Jóga",
        "series": "シリーズ", "season_number": "2", "episode_number": 17,
        "upload_date": "20261021", "extractor_key": "Generic", "duration": 42
    });
    fs::write(f.0.join("metadata.json"), serde_json::to_vec(&raw).unwrap()).unwrap();
    f.script("cd -- \"$(dirname -- \"$0\")\"\ncat metadata.json");
    let (entries, failed) = collect(&f.0, &["https://e.test/music"]);
    assert!(failed.is_empty());
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry["meta"]["title"], "CON / Café 日本語 🎵 [4K]");
    assert_eq!(entry["meta"]["artist"], "Björk, 宇多田ヒカル");
    assert_eq!(entry["meta"]["track"], "Jóga");
    assert_eq!(entry["meta"]["series"], "シリーズ");
    assert_eq!(entry["meta"]["season_number"], 2);
    assert_eq!(entry["meta"]["episode_number"], 17);
    assert_eq!(entry["meta"]["upload_date"], "20261021");
    assert_eq!(entry["meta"]["site"], "Generic");
    assert_eq!(entry["meta"]["description"], "日".repeat(300));
    assert_eq!(
        (
            entry["lang"].clone(),
            entry["expected"].clone(),
            entry["accept"].clone()
        ),
        (json!(""), json!(""), json!([]))
    );
    let out = f.0.join("to-label 日本語.json");
    write_new(&out, &entries).unwrap();
    let saved: Vec<Value> = serde_json::from_slice(&fs::read(out).unwrap()).unwrap();
    assert_eq!(saved, entries);
}

#[cfg(unix)]
#[test]
fn n9_collect_skips_malformed_progress_playlist_and_nonzero_then_recovers() {
    let f = Fixture::new();
    f.script(
        r#"for a; do last=$a; done
case "$last" in
  *malformed) printf '{';;
  *progress) printf 'QUEUE-PROGRESS {"status":"finished"}\n{"title":"wrong"}';;
  *playlist) printf '{"_type":"playlist","title":"wrong"}';;
  *error) printf '{"title":"wrong"}'; echo 'ERROR: [generic] fixture failure' >&2; exit 1;;
  *) printf '{"title":"NUL","uploader":"频道"}';;
esac"#,
    );
    let urls = [
        "https://e.test/malformed",
        "https://e.test/progress",
        "https://e.test/playlist",
        "https://e.test/error",
        "https://e.test/ok",
    ];
    let (entries, failed) = collect(&f.0, &urls);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["url"], urls[4]);
    assert_eq!(entries[0]["meta"]["title"], "NUL");
    assert_eq!(entries[0]["meta"]["channel"], "频道");
    assert_eq!(
        failed
            .iter()
            .map(|(url, _)| url.as_str())
            .collect::<Vec<_>>(),
        urls[..4]
    );
    assert!(failed[0].1.contains("unreadable info"));
    assert!(failed[1].1.contains("unreadable info"));
    assert!(failed[2].1.contains("Playlists"));
    assert_eq!(failed[3].1, "fixture failure");
}

#[cfg(unix)]
#[test]
fn n9_collect_untrusted_link_stays_one_literal_argument_after_separator() {
    let f = Fixture::new();
    f.script(
        r#"for a; do previous=$last; last=$a; done
[ "$previous" = -- ] || exit 4
printf '%s' "$last" > "$(dirname -- "$0")/argument.txt"
printf '{"title":"literal"}'"#,
    );
    let url = "--exec=touch SHOULD_NOT_EXIST;$(echo unsafe) https://e.test/日本?q='x'";
    let (entries, failed) = collect(&f.0, &[url]);
    assert!(failed.is_empty(), "{failed:?}");
    assert_eq!(entries[0]["url"], url);
    assert_eq!(fs::read_to_string(f.0.join("argument.txt")).unwrap(), url);
}

#[test]
fn n9_collect_output_collision_and_seed_guard_preserve_existing_bytes() {
    let f = Fixture::new();
    let out = f.0.join("to-label.json");
    fs::write(&out, b"owner labels, never replace").unwrap();
    assert_eq!(
        write_new(&out, &[json!({})]).unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read(&out).unwrap(), b"owner labels, never replace");
    let seed = f.0.join("titles.json");
    assert!(write_new(&seed, &[]).is_err());
    assert!(!seed.exists());
    fs::write(&seed, b"seed bytes").unwrap();
    assert!(write_new(&seed, &[]).is_err());
    assert_eq!(fs::read(&seed).unwrap(), b"seed bytes");
}

#[cfg(unix)]
#[test]
fn n9_collect_output_refuses_live_and_dangling_symlinks() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let existing = f.0.join("labels.json");
    let missing = f.0.join("missing.json");
    fs::write(&existing, b"labels").unwrap();
    for (name, target) in [("live.json", &existing), ("dangling.json", &missing)] {
        let link = f.0.join(name);
        symlink(target, &link).unwrap();
        assert!(write_new(&link, &[]).is_err());
        assert_eq!(fs::read_link(link).unwrap(), *target);
    }
    assert_eq!(fs::read(existing).unwrap(), b"labels");
    assert!(!missing.exists());
}

#[test]
fn n9_collect_app_layout_copies_all_four_pinned_tools_without_changing_source() {
    let f = Fixture::new();
    let triple = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(target_os = "macos") {
        "x86_64-apple-darwin"
    } else if cfg!(target_os = "windows") {
        "x86_64-pc-windows-msvc"
    } else {
        "x86_64-unknown-linux-gnu"
    };
    let exe = std::env::consts::EXE_SUFFIX;
    let mut paths = Vec::new();
    for stem in [
        "queue-yt-dlp",
        "queue-ffmpeg",
        "queue-ffprobe",
        "queue-deno",
    ] {
        let source = f.0.join(format!("{stem}-{triple}{exe}"));
        fs::write(&source, stem).unwrap();
        paths.push((source, format!("{stem}{exe}"), stem));
    }
    let out = app_layout(&f.0);
    for (source, name, bytes) in paths {
        assert_eq!(fs::read(source).unwrap(), bytes.as_bytes());
        assert_eq!(fs::read(out.join(name)).unwrap(), bytes.as_bytes());
    }
    fs::remove_dir_all(out).unwrap();
}
