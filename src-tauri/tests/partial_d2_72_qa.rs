//! Independent QA for #72 / D2 / D-014, with N2 collision protection.
//! Use only disposable files; a killed copy runs in a child test process.
use queue_lib::download;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "queue-partial-72-qa-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("cache/downloads/item-72")).unwrap();
        Self(root)
    }

    fn work(&self) -> PathBuf {
        self.0.join("cache/downloads/item-72")
    }

    fn out(&self) -> PathBuf {
        let out = self.0.join("out 日本語 e\u{301}");
        fs::create_dir_all(&out).unwrap();
        out
    }

    /// A note as production writes it (`download::partial_note`): a claim with the file ID it has
    /// now, as when Queue made it. Without file IDs (Windows) no claim is recorded.
    fn note(&self, n: u32, temp: &Path, claim: Option<&Path>) {
        let id = claim.and_then(|c| download::file_id(&fs::symlink_metadata(c).unwrap()));
        let name = format!("queue-partial-72-{n}.txt");
        let text = download::partial_note(temp, claim.zip(id));
        fs::write(self.work().join(name), text).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn d72_recovery_removes_recorded_partial_and_claim_but_preserves_completed_and_unrecorded_files() {
    let f = Fixture::new();
    let out = f.out();
    let partial = out.join(".queue-partial-72-0");
    let claim = out.join("_CON 日本語 (2).webm");
    let finished = out.join("日本語 🎵 (OFFICIAL VIDEO).webm");
    let unrecorded = out.join(".queue-partial-user-file");
    let empty = out.join("_NUL.opus");
    for (path, bytes) in [
        (&partial, b"cut off".as_slice()),
        (&claim, b""),
        (&finished, b"whole media"),
        (&unrecorded, b"unrelated"),
        (&empty, b""),
    ] {
        fs::write(path, bytes).unwrap();
    }
    f.note(0, &partial, Some(&claim));
    // A claim the copy was already renamed over is a finished download now: it's kept.
    f.note(1, &out.join(".queue-partial-72-1"), Some(&finished));
    download::remove_stale(&f.work()).unwrap();
    assert!(!partial.exists());
    assert_eq!(
        claim.exists(),
        cfg!(not(unix)),
        "kept only where it can't be recorded"
    );
    assert_eq!(fs::read(&finished).unwrap(), b"whole media");
    assert_eq!(fs::read(&unrecorded).unwrap(), b"unrelated");
    assert!(empty.is_file());
    assert!(download::stale_work_dirs(&f.0.join("cache")).is_empty());
}

#[cfg(unix)]
#[test]
fn d72_recovery_does_not_follow_note_or_leaf_symlinks_or_remove_directories() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let out = f.out();
    let target = out.join("keep-empty.webm");
    fs::write(&target, b"").unwrap();
    let partial = out.join(".queue-partial-link");
    let claim = out.join("Claim.webm");
    symlink(&target, &partial).unwrap();
    symlink(&target, &claim).unwrap();
    let folder = out.join(".queue-partial-folder");
    fs::create_dir(&folder).unwrap();
    f.note(0, &partial, Some(&claim));
    f.note(1, &folder, None);
    let unrecorded = out.join(".queue-partial-not-from-a-real-note");
    fs::write(&unrecorded, b"keep").unwrap();
    let outside_note = f.0.join("outside-note.txt");
    fs::write(&outside_note, download::partial_note(&unrecorded, None)).unwrap();
    symlink(&outside_note, f.work().join("queue-partial-link.txt")).unwrap();
    download::remove_stale(&f.work()).unwrap();
    assert_eq!(fs::read_link(partial).unwrap(), target);
    assert_eq!(fs::read_link(claim).unwrap(), target);
    assert!(target.is_file());
    assert!(folder.is_dir());
    assert_eq!(fs::read(unrecorded).unwrap(), b"keep");
}

// A FIFO keeps the actual copy_unless_quitting blocked after its first bytes. Killing that
// child bypasses Rust destructors, exercising the production note writer and launch cleanup.
#[cfg(unix)]
#[test]
#[ignore = "subprocess entry; invoked by interruption tests"]
fn d72_blocked_copy_child() {
    let src = std::env::var_os("QUEUE_QA72_FIFO_SRC").expect("child source");
    let dest = std::env::var_os("QUEUE_QA72_COPY_DEST").expect("child destination");
    let quitting = std::env::var("QUEUE_QA72_QUIT").as_deref() == Ok("true");
    let watcher = quitting.then(|| {
        let work = Path::new(&src).parent().unwrap().to_path_buf();
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !work.join("quit-request").exists() {
                assert!(std::time::Instant::now() < deadline, "no quit request");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            queue_lib::proc::shut_down();
            fs::write(work.join("quit-started"), b"").unwrap();
        })
    });
    let result = download::copy_no_clobber(Path::new(&src), Path::new(&dest));
    if let Some(watcher) = watcher {
        watcher.join().unwrap();
        assert_eq!(result.unwrap_err().to_string(), download::QUITTING);
    } else {
        panic!("the parent must kill this blocked copy: {result:?}");
    }
}

#[cfg(unix)]
fn interrupt_copy_then_recover(folder: &std::ffi::OsStr, quitting: bool) {
    use std::{
        ffi::CString,
        io::Write,
        os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
        process::{Child, Command, Stdio},
        thread,
        time::{Duration, Instant},
    };
    struct KillOnDrop(Child);
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let f = Fixture::new();
    let out = f.0.join(folder);
    fs::create_dir(&out).unwrap();
    let src = f.work().join("source.fifo");
    let dest = out.join("_CON 日本語.webm");
    let c_src = CString::new(src.as_os_str().as_bytes()).unwrap();
    // SAFETY: valid NUL-terminated disposable path, with owner-only permissions.
    assert_eq!(unsafe { libc::mkfifo(c_src.as_ptr(), 0o600) }, 0);
    let mut child = KillOnDrop(
        Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "d72_blocked_copy_child"])
            .env("QUEUE_QA72_FIFO_SRC", &src)
            .env("QUEUE_QA72_COPY_DEST", &dest)
            .env("QUEUE_QA72_QUIT", quitting.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut writer = loop {
        match fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&src)
        {
            Ok(file) => break file,
            Err(e) if e.raw_os_error() == Some(libc::ENXIO) => {
                assert!(
                    Instant::now() < deadline,
                    "copy child never opened the FIFO"
                );
                assert!(
                    child.0.try_wait().unwrap().is_none(),
                    "copy child exited early"
                );
                thread::sleep(Duration::from_millis(5));
            }
            Err(e) => panic!("opening FIFO: {e}"),
        }
    };
    writer.write_all(b"incomplete media").unwrap();
    let partial = loop {
        let found = fs::read_dir(&out).unwrap().map(Result::unwrap).find(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".queue-partial-")
                && e.metadata().unwrap().len() == 16
        });
        if let Some(entry) = found {
            break entry.path();
        }
        assert!(
            Instant::now() < deadline,
            "copy child never wrote the partial bytes"
        );
        thread::sleep(Duration::from_millis(5));
    };
    assert!(
        !dest.exists(),
        "a blocked copy must never expose a final filename"
    );
    assert_eq!(fs::read_dir(&out).unwrap().count(), 1);
    if quitting {
        fs::write(f.work().join("quit-request"), b"").unwrap();
        while !f.work().join("quit-started").exists() {
            assert!(Instant::now() < deadline, "child never started quitting");
            thread::sleep(Duration::from_millis(5));
        }
        // The FIFO read may be blocked. Let it return so the next chunk observes shutdown.
        // Keep the writer open: EOF would instead report a successfully completed copy.
        if let Err(error) = writer.write_all(b"one more chunk") {
            // It may have noticed shutdown before starting its next read.
            assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        }
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "copy did not stop after quitting"
            );
            thread::sleep(Duration::from_millis(5));
        };
        assert!(status.success(), "child did not return the quitting error");
        assert!(!dest.exists());
        assert!(
            !partial.exists(),
            "graceful quit removes the partial immediately"
        );
        assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
        assert!(src.exists(), "quitting preserves the source");
        assert!(!fs::read_dir(f.work()).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("queue-partial-")
        }));
        return;
    }
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    drop(writer);
    assert!(
        partial.is_file(),
        "a crash bypasses temporary-file destructors"
    );
    assert!(!dest.exists());
    let stale = download::stale_work_dirs(&f.0.join("cache"));
    assert_eq!(stale, [f.work()]);
    download::remove_stale(&stale[0]).unwrap();
    assert!(
        !partial.exists(),
        "startup lost the actual recorded path and left a Queue partial behind: {partial:?}"
    );
    assert!(!f.work().exists());
}

#[cfg(unix)]
#[test]
fn d72_killed_copy_never_exposes_final_name_and_launch_removes_its_real_partial() {
    interrupt_copy_then_recover(std::ffi::OsStr::new("out 日本語 e\u{301}"), false);
}

#[cfg(unix)]
#[test]
fn d72_quitting_mid_copy_removes_partial_and_note_without_exposing_a_final_name() {
    interrupt_copy_then_recover(std::ffi::OsStr::new("out 日本語 e\u{301}"), true);
}

#[cfg(unix)]
#[test]
fn d72_launch_recovers_a_killed_copy_in_a_folder_containing_a_newline() {
    interrupt_copy_then_recover(std::ffi::OsStr::new("out\n日本語"), false);
}

// The local macOS filesystem refuses this byte sequence at create_dir. Linux permits it.
#[cfg(target_os = "linux")]
#[test]
fn d72_launch_recovers_a_killed_copy_in_a_non_utf8_folder() {
    use std::os::unix::ffi::OsStrExt;
    interrupt_copy_then_recover(std::ffi::OsStr::from_bytes(b"out-\xff"), false);
}
