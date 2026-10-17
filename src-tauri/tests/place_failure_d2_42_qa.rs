//! Independent QA for D2 / #42: exercise the real copy fallback with filesystem failures.
//! Cross-device, mid-write and sync fault injection are covered by download's unit tests.
use queue_lib::download;
use std::{
    fs, io,
    path::PathBuf,
    sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    },
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "queue-place-42-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn copy_fallback_preserves_empty_and_large_unicode_files_byte_for_byte() {
    let f = Fixture::new();
    // Placement receives names after yt-dlp's Windows sanitizer; it must keep them verbatim.
    for name in [
        "日本語 🎵 e\u{301} & 'quote' (OFFICIAL VIDEO).webm",
        "_CON.m4a",
        "_NUL.opus",
    ] {
        for bytes in [
            Vec::new(),
            (0..2_000_003).map(|i| (i % 251) as u8).collect(),
        ] {
            let src = f.path("staged");
            let dest = f.path(name);
            fs::write(&src, &bytes).unwrap();
            download::copy_no_clobber(&src, &dest).unwrap();
            assert_eq!(fs::read(&dest).unwrap(), bytes);
            assert!(!src.exists());
            fs::remove_file(dest).unwrap();
        }
    }
}

#[test]
fn missing_source_removes_the_new_destination_and_returns_the_read_error() {
    let f = Fixture::new();
    let dest = f.path("dest.webm");
    let error = download::copy_no_clobber(&f.path("missing"), &dest).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert!(
        !dest.exists(),
        "opening the source failed after the destination was created"
    );
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 0);
}

#[test]
fn occupied_file_or_directory_is_never_removed_on_copy_failure() {
    let f = Fixture::new();
    let src = f.path("source");
    fs::write(&src, b"new bytes").unwrap();
    let file = f.path("existing.webm");
    let dir = f.path("existing-directory.webm");
    fs::write(&file, b"keep original").unwrap();
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("child"), b"keep child").unwrap();
    for dest in [&file, &dir, &src] {
        assert!(download::copy_no_clobber(&src, dest).is_err());
        assert_eq!(fs::read(&src).unwrap(), b"new bytes");
    }
    assert_eq!(fs::read(file).unwrap(), b"keep original");
    assert_eq!(fs::read(dir.join("child")).unwrap(), b"keep child");
}

#[test]
fn concurrent_copy_fallbacks_have_one_winner_and_preserve_every_losing_source() {
    let f = Fixture::new();
    let dest = f.path("日本語.webm");
    let barrier = Arc::new(Barrier::new(12));
    let handles: Vec<_> = (0..12)
        .map(|n| {
            let src = f.path(&format!("source-{n}"));
            let payload = vec![n as u8; 512 * 1024];
            fs::write(&src, &payload).unwrap();
            let dest = dest.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                (src.clone(), payload, download::copy_no_clobber(&src, &dest))
            })
        })
        .collect();
    let mut winners = 0;
    for handle in handles {
        let (src, payload, result) = handle.join().unwrap();
        match result {
            Ok(()) => {
                winners += 1;
                assert!(!src.exists());
                assert_eq!(fs::read(&dest).unwrap(), payload);
            }
            Err(error) => {
                assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
                assert_eq!(fs::read(src).unwrap(), payload);
            }
        }
    }
    assert_eq!(winners, 1);
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 12);
}

#[cfg(unix)]
#[test]
fn source_read_failure_removes_destination_but_preserves_source_directory() {
    let f = Fixture::new();
    let src = f.path("not-a-regular-file");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("child"), b"keep").unwrap();
    let dest = f.path("partial.webm");
    assert!(download::copy_no_clobber(&src, &dest).is_err());
    assert!(!dest.exists());
    assert_eq!(fs::read(src.join("child")).unwrap(), b"keep");
}

#[cfg(unix)]
#[test]
fn copy_fallback_does_not_follow_or_replace_live_or_dangling_destination_symlinks() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let src = f.path("source");
    let target = f.path("existing-target");
    let missing = f.path("missing-target");
    fs::write(&src, b"new").unwrap();
    fs::write(&target, b"original").unwrap();
    for (name, target) in [("live.webm", &target), ("dangling.webm", &missing)] {
        let dest = f.path(name);
        symlink(target, &dest).unwrap();
        let error = download::copy_no_clobber(&src, &dest).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_link(dest).unwrap(), *target);
        assert_eq!(fs::read(&src).unwrap(), b"new");
    }
    assert_eq!(fs::read(target).unwrap(), b"original");
    assert!(!missing.exists());
}
