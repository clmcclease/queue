//! Independent QA for N2 / #16. Filesystem fixtures never use the user's Downloads folder.
use queue_lib::download;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    root: PathBuf,
    out: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "queue-n2-qa-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let out = root.join("output 日本語");
        fs::create_dir_all(&out).unwrap();
        Self { root, out }
    }
    fn source(&self, tag: &str, bytes: &[u8]) -> PathBuf {
        let src = self.root.join(tag);
        fs::write(&src, bytes).unwrap();
        src
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn n2_first_gap_wins_even_when_later_numbers_exist() {
    let f = Fixture::new();
    for name in ["Title.webm", "Title (2).webm", "Title (4).webm"] {
        fs::write(f.out.join(name), name.as_bytes()).unwrap();
    }
    let src = f.source("new", b"new bytes");
    let path = download::place_unique(&src, &f.out, OsStr::new("Title.webm")).unwrap();
    assert_eq!(path, f.out.join("Title (3).webm"));
    assert_eq!(fs::read(path).unwrap(), b"new bytes");
    assert!(!src.exists());
    for name in ["Title.webm", "Title (2).webm", "Title (4).webm"] {
        assert_eq!(fs::read(f.out.join(name)).unwrap(), name.as_bytes());
    }
}

#[test]
fn n2_suffix_preserves_messy_unicode_and_the_last_extension() {
    let f = Fixture::new();
    // Reserved stems are already sanitized by yt-dlp; N2 must preserve that safe result.
    for (name, expected) in [
        (
            " 日本語 🎵 e\u{301} & 'quote' (OFFICIAL VIDEO).opus",
            " 日本語 🎵 e\u{301} & 'quote' (OFFICIAL VIDEO) (2).opus",
        ),
        ("_CON.wav", "_CON (2).wav"),
        ("_LPT9.wav", "_LPT9 (2).wav"),
        ("Podcast.episode.12.m4a", "Podcast.episode.12 (2).m4a"),
        (".hidden", ".hidden (2)"),
        ("README", "README (2)"),
        ("Already (2).webm", "Already (2) (2).webm"),
    ] {
        fs::write(f.out.join(name), b"original").unwrap();
        let src = f.source("new", b"new");
        assert_eq!(
            download::place_unique(&src, &f.out, OsStr::new(name)).unwrap(),
            f.out.join(expected)
        );
        assert_eq!(fs::read(f.out.join(name)).unwrap(), b"original");
        assert_eq!(fs::read(f.out.join(expected)).unwrap(), b"new");
    }
}

#[test]
fn n2_simultaneous_finishers_claim_distinct_names_and_keep_every_payload() {
    let f = Fixture::new();
    fs::write(f.out.join("Title.webm"), b"original").unwrap();
    let barrier = Arc::new(Barrier::new(16));
    let handles: Vec<_> = (0..16)
        .map(|i| {
            let bytes = format!("payload {i}");
            let src = f.source(&format!("source-{i}"), bytes.as_bytes());
            let out = f.out.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let path = download::place_unique(&src, &out, OsStr::new("Title.webm")).unwrap();
                assert_eq!(fs::read(&path).unwrap(), bytes.as_bytes());
                assert!(!src.exists());
                path.file_name().unwrap().to_owned()
            })
        })
        .collect();
    let actual: std::collections::BTreeSet<_> =
        handles.into_iter().map(|h| h.join().unwrap()).collect();
    let expected: std::collections::BTreeSet<_> = (2..=17)
        .map(|n| format!("Title ({n}).webm").into())
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(fs::read(f.out.join("Title.webm")).unwrap(), b"original");
    assert_eq!(fs::read_dir(&f.out).unwrap().count(), 17);
}

#[test]
fn n2_999_is_usable_and_exhaustion_preserves_every_file_and_source() {
    let f = Fixture::new();
    fs::write(f.out.join("Title.webm"), b"original").unwrap();
    for n in 2..999 {
        fs::write(f.out.join(format!("Title ({n}).webm")), n.to_string()).unwrap();
    }
    let src = f.source("last", b"last available");
    assert_eq!(
        download::place_unique(&src, &f.out, OsStr::new("Title.webm")).unwrap(),
        f.out.join("Title (999).webm")
    );
    let src = f.source("overflow", b"keep source");
    let err = download::place_unique(&src, &f.out, OsStr::new("Title.webm")).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(src).unwrap(), b"keep source");
    assert_eq!(fs::read(f.out.join("Title.webm")).unwrap(), b"original");
    for n in 2..999 {
        assert_eq!(
            fs::read(f.out.join(format!("Title ({n}).webm"))).unwrap(),
            n.to_string().as_bytes()
        );
    }
    assert_eq!(
        fs::read(f.out.join("Title (999).webm")).unwrap(),
        b"last available"
    );
    assert_eq!(fs::read_dir(&f.out).unwrap().count(), 999);
}

#[test]
fn n2_noncollision_errors_keep_source_and_unrelated_output() {
    let f = Fixture::new();
    let blocked = f.source("not-a-folder", b"unrelated");
    let src = f.source("new", b"keep source");
    assert!(download::place_unique(&src, &blocked, OsStr::new("Title.webm")).is_err());
    assert_eq!(fs::read(src).unwrap(), b"keep source");
    assert_eq!(fs::read(blocked).unwrap(), b"unrelated");
    assert_eq!(fs::read_dir(&f.out).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn n2_live_and_dangling_symlink_collisions_are_not_followed_or_replaced() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let target = f.source("outside-output", b"outside bytes");
    let missing = f.root.join("missing");
    symlink(&target, f.out.join("Title.webm")).unwrap();
    symlink(&missing, f.out.join("Title (2).webm")).unwrap();
    let src = f.source("new", b"new");
    assert_eq!(
        download::place_unique(&src, &f.out, OsStr::new("Title.webm")).unwrap(),
        f.out.join("Title (3).webm")
    );
    assert_eq!(fs::read_link(f.out.join("Title.webm")).unwrap(), target);
    assert_eq!(
        fs::read_link(f.out.join("Title (2).webm")).unwrap(),
        missing
    );
    assert_eq!(fs::read(target).unwrap(), b"outside bytes");
    assert!(!missing.exists());
}

#[test]
fn n2_collision_at_filesystem_name_limit_still_keeps_both() {
    let f = Fixture::new();
    let name = format!("{}.webm", "a".repeat(250)); // A valid 255-byte ASCII component.
    fs::write(f.out.join(&name), b"original").unwrap();
    let src = f.source("new", b"new bytes");
    let result = download::place_unique(&src, &f.out, OsStr::new(&name));
    assert_eq!(fs::read(f.out.join(&name)).unwrap(), b"original");
    let path = result.expect("N2: a collision on a valid filename must save a numbered copy, shortening the stem if needed");
    assert!(
        path.file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with(" (2).webm")
    );
    assert_eq!(fs::read(path).unwrap(), b"new bytes");
    assert!(!src.exists());
}
