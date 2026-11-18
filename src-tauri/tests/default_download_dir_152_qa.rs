//! Independent QA for D1 / #152. No environment changes or real user folders.
use queue_lib::settings::{FILE, Settings, default_download_dir};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "queue-default-152-qa-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn d1_152_os_folder_works_even_without_a_home_and_wins_over_the_fallback() {
    let os = PathBuf::from("root").join("Téléchargements 日本語");
    for home in [None, Some(PathBuf::from("another-home"))] {
        assert_eq!(
            default_download_dir(Some(os.clone()), home).unwrap(),
            os,
            "an OS-provided folder must retain its exact spelling"
        );
    }
}

#[test]
fn d1_152_fallback_preserves_unicode_spaces_and_literal_metacharacters() {
    for user in ["Vidéos 日本語 🎵", "e\u{301} 'quote' & $(literal) %"] {
        let home = PathBuf::from("users").join(user);
        assert_eq!(
            default_download_dir(None, Some(home.clone())).unwrap(),
            home.join("Downloads"),
            "a home path is not a media filename and must not be sanitized"
        );
    }
    let error = default_download_dir(None, None).unwrap_err();
    assert!(error.contains("home folder"), "{error}");
    assert!(error.contains("Choose a folder"), "{error}");
    assert!(error.contains("Change"), "{error}");
    assert!(!error.contains("unknown path"), "{error}");
}

#[test]
fn d1_152_resolving_the_fallback_does_not_turn_it_into_a_saved_choice() {
    let f = Fixture::new();
    let settings = Settings::default();
    let first_home = f.0.join("first user");
    assert_eq!(
        settings
            .download_dir(|| default_download_dir(None, Some(first_home.clone())))
            .unwrap(),
        first_home.join("Downloads")
    );
    let file = f.0.join(FILE);
    settings.save(&file).unwrap();
    let loaded = Settings::load(&file);
    assert_eq!(loaded.download_dir, None);
    let second_home = f.0.join("second user");
    assert_eq!(
        loaded
            .download_dir(|| default_download_dir(None, Some(second_home.clone())))
            .unwrap(),
        second_home.join("Downloads"),
        "the default is resolved afresh rather than persisted as a selected folder"
    );
}

#[test]
fn d1_152_saved_folder_bypasses_default_resolution_even_when_it_disappears() {
    let f = Fixture::new();
    let chosen = f.0.join("chosen 日本語");
    fs::create_dir(&chosen).unwrap();
    let settings = Settings {
        download_dir: Some(chosen.clone()),
        ..Default::default()
    };
    assert_eq!(
        settings
            .download_dir(|| default_download_dir(None, None))
            .unwrap(),
        chosen,
        "a chosen folder works even if neither default path can be resolved"
    );
    fs::remove_dir(&chosen).unwrap();
    let fallback_home = f.0.join("fallback home");
    let error = settings
        .download_dir(|| default_download_dir(None, Some(fallback_home.clone())))
        .unwrap_err();
    assert!(error.contains("isn't available"), "{error}");
    assert!(error.contains("chosen 日本語"), "{error}");
    assert!(!chosen.exists());
    assert!(!fallback_home.exists());
}

#[cfg(unix)]
#[test]
fn d1_152_non_utf8_os_and_home_paths_are_not_lossily_rewritten() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let home = PathBuf::from(OsString::from_vec(b"/users/name-\xff".to_vec()));
    let os = home.join(OsString::from_vec(b"downloads-\xfe".to_vec()));
    assert_eq!(default_download_dir(Some(os.clone()), None).unwrap(), os);
    assert_eq!(
        default_download_dir(None, Some(home.clone())).unwrap(),
        home.join("Downloads")
    );
}
