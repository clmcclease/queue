//! Independent QA for #10 / D1. Disposable filesystem and subprocess fixtures only.
use queue_lib::settings::{FILE, Settings};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "queue-d1-qa-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn d1_empty_null_and_partial_settings_use_default_folder() {
    let f = Fixture::new();
    let path = f.0.join(FILE);
    for json in [
        "",
        "null",
        "{}",
        r#"{"downloadDir":null}"#,
        r#"{"future":true}"#,
    ] {
        fs::write(&path, json).unwrap();
        assert_eq!(Settings::load(&path), Settings::default(), "{json}");
    }
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert_eq!(Settings::load(&path), Settings::default());
}

#[test]
fn d1_repeated_saves_remember_exact_unicode_and_metacharacter_paths() {
    let f = Fixture::new();
    let path = f.0.join("nested").join(FILE);
    for name in [
        "Vidéos 日本語 🎵",
        "e\u{301} 'quote' & $(literal) %",
        "Second folder",
    ] {
        let dir = f.0.join(name);
        fs::create_dir(&dir).unwrap();
        let settings = Settings {
            download_dir: Some(dir.clone()),
        };
        settings.save(&path).unwrap();
        let reloaded = Settings::load(&path);
        assert_eq!(reloaded, settings);
        assert_eq!(
            reloaded
                .download_dir(|| panic!("must not use default"))
                .unwrap(),
            dir
        );
        let temps = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .count();
        assert_eq!(temps, 0, "no temp file left behind");
    }
}

#[cfg(unix)]
#[test]
fn d1_failed_write_preserves_previously_remembered_choice() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let dir = f.0.join("config");
    let path = dir.join(FILE);
    let first = Settings {
        download_dir: Some(f.0.join("first")),
    };
    first.save(&path).unwrap();
    // A read-only settings folder deterministically prevents writing the temp file.
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
    let second = Settings {
        download_dir: Some(f.0.join("second")),
    };
    let result = second.save(&path);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    assert_eq!(Settings::load(&path), first);
    let names: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, [FILE], "no temp file left behind");
}

#[test]
fn d1_interrupted_save_temp_file_does_not_override_completed_settings() {
    let f = Fixture::new();
    let path = f.0.join(FILE);
    let settings = Settings {
        download_dir: Some(f.0.join("original")),
    };
    settings.save(&path).unwrap();
    // Left over by a save that crashed mid-write: never loaded, and it can't block later saves.
    let leftover = path.with_extension("json.1-0-0.tmp");
    fs::write(&leftover, b"{unfinished").unwrap();
    assert_eq!(Settings::load(&path), settings);
    Settings::default().save(&path).unwrap();
    assert_eq!(Settings::load(&path), Settings::default());
    assert_eq!(
        fs::read(&leftover).unwrap(),
        b"{unfinished",
        "someone else's file is left alone"
    );
}

#[test]
fn d1_chosen_file_or_missing_folder_never_falls_back_or_gets_created() {
    let f = Fixture::new();
    let file = f.0.join("regular-file");
    fs::write(&file, b"unrelated").unwrap();
    let missing = f.0.join("offline").join("Videos 日本語");
    for path in [&file, &missing] {
        let settings = Settings {
            download_dir: Some(path.clone()),
        };
        let err = settings
            .download_dir(|| panic!("must not silently fall back"))
            .unwrap_err();
        assert!(err.contains("isn't available"), "{err}");
        assert!(err.contains(&path.display().to_string()), "{err}");
    }
    assert_eq!(fs::read(file).unwrap(), b"unrelated");
    assert!(!missing.exists());
}

#[cfg(unix)]
#[test]
fn d1_dangling_folder_symlink_is_unavailable_without_recreating_its_target() {
    let f = Fixture::new();
    let target = f.0.join("unmounted");
    let link = f.0.join("chosen");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let settings = Settings {
        download_dir: Some(link.clone()),
    };
    assert!(
        settings
            .download_dir(|| panic!("must not fall back"))
            .is_err()
    );
    assert_eq!(fs::read_link(link).unwrap(), target);
    assert!(!target.exists());
}

#[cfg(unix)]
#[test]
fn d1_chosen_folder_disappearing_after_start_is_not_recreated_at_completion() {
    use queue_lib::{download, sidecar::Sidecar};
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let out = f.0.join("selected 日本語");
    fs::create_dir(&out).unwrap();
    let settings = Settings {
        download_dir: Some(out.clone()),
    };
    // begin_download captures this path; the volume can disappear while yt-dlp runs.
    let captured = settings
        .download_dir(|| panic!("must not fall back"))
        .unwrap();
    let bin = f.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let script = Sidecar::YtDlp.path_in(&bin);
    fs::write(&script, "#!/bin/sh\nset -eu\nP=; prev=; for a in \"$@\"; do [ \"$prev\" = -P ] && P=$a; prev=$a; done\nprintf media > \"$P/Title.webm\"\n").unwrap();
    fs::set_permissions(script, fs::Permissions::from_mode(0o700)).unwrap();
    fs::remove_dir(&out).unwrap();
    let work = f.0.join("work");
    let result = download::run(&bin, &work, &captured, "https://example.test/v");
    assert!(
        !out.exists(),
        "D1 / D-020: never recreate a chosen unavailable folder; result={result:?}"
    );
    assert!(
        result.is_err(),
        "report the unavailable folder instead of saving locally"
    );
    assert!(!work.exists(), "staging must be cleaned on failure");
}
