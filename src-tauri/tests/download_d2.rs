//! Independent QA for #4 / D2. No production edits or external network requests.
//! Unix-only executable fixtures exercise the actual subprocess adapter. Pure checks run everywhere.
use queue_lib::{
    download,
    meta::Meta,
    queue::{Queue, Status},
};
use std::ffi::OsString;
use std::path::Path;

#[test]
fn d2_keeps_unicode_paths_and_hostile_urls_as_single_arguments() {
    let sidecars = Path::new("Queue's 日本語 & tools");
    let output = Path::new("Downloads 日本語 & spaces");
    for url in [
        "--exec=echo unsafe",
        "https://例え.test/🎵?title=CON&x=$(touch sentinel);`echo bad`",
        "https://example.test/?title=a\"b'c\\d\nsecond line",
    ] {
        let args = download::args(sidecars, output, url);
        assert_eq!(args.last(), Some(&OsString::from(url)));
        assert_eq!(args[args.len() - 2], "--");
        let at = args.iter().position(|a| a == "-P").unwrap();
        assert_eq!(args[at + 1], output.as_os_str());
        for forced in [
            "-f",
            "--format",
            "--recode-video",
            "--remux-video",
            "--merge-output-format",
            "--audio-format",
            "-x",
        ] {
            assert!(
                !args.iter().any(|a| a == forced),
                "Default must not force {forced}"
            );
        }
    }
}

#[test]
fn d2_errors_prefer_last_error_over_trailing_warnings_and_malformed_progress() {
    assert_eq!(
        download::failure_reason(
            "ERROR: first\r\n[download] ??% NaN ETA ???\n  ERROR: 日本語: unavailable 🎵  \r\nWARNING: cleanup\n\n",
            "exit status: 1",
        ),
        "日本語: unavailable 🎵",
    );
    assert_eq!(
        download::failure_reason("\t\r\n  ", "signal: 9"),
        "yt-dlp exited with signal: 9"
    );
}

#[test]
fn d2_failed_item_does_not_change_another_items_state_or_revision() {
    let mut q = Queue::default();
    let a = q.add("https://example.test/a").unwrap();
    let b = q.add("https://example.test/b").unwrap();
    // #6: rows fetch their info before they can download.
    let a = q.fetched(a.id, Ok(Meta::default())).unwrap();
    let b = q.fetched(b.id, Ok(Meta::default())).unwrap();
    q.start(a.id).unwrap();
    q.finish(a.id, Err("Video unavailable".into())).unwrap();
    assert_eq!(q.items()[1], b);
    q.start(b.id).unwrap();
    let done = q
        .finish(b.id, Ok("C:\\Downloads\\日本語 🎵.webm".into()))
        .unwrap();
    assert_eq!(done.rev, 4);
    assert!(matches!(done.status, Status::Done { .. }));
    assert!(matches!(q.items()[0].status, Status::Failed { .. }));
    assert_eq!(q.finish(a.id, Ok("late.webm".into())), None);
    assert!(q.start(a.id).is_err());
    assert_eq!(q.items()[0].rev, 4);
}

#[cfg(unix)]
mod subprocess {
    //! A fake yt-dlp (a shell script) writes into the `-P` working folder, as the real one does.
    use super::*;
    use queue_lib::sidecar::Sidecar;
    use std::fs::{self, Permissions};
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    /// Prepended to every script: `$P` is the working folder passed with `-P`.
    const FIND_P: &str = "P=; prev=; for a in \"$@\"; do [ \"$prev\" = -P ] && P=$a; prev=$a; done";

    struct Fixture {
        root: PathBuf,
        bin: PathBuf,
        work: PathBuf,
        out: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "queue-d2-{}-{}-Queue's 日本語 & spaces",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            let bin = root.join("tools");
            let out = root.join("output");
            fs::create_dir_all(&bin).unwrap();
            fs::create_dir_all(&out).unwrap();
            Self {
                work: root.join("cache").join("work"),
                root,
                bin,
                out,
            }
        }

        fn script(&self, body: &str) {
            let path = Sidecar::YtDlp.path_in(&self.bin);
            fs::write(&path, format!("#!/bin/sh\nset -eu\n{FIND_P}\n{body}\n")).unwrap();
            fs::set_permissions(path, Permissions::from_mode(0o700)).unwrap();
        }

        /// A fake yt-dlp that saves `name` with `content` and exits 0.
        fn saves(&self, name: &str, content: &str) {
            self.script(&format!(
                "printf {} > \"$P\"/{}",
                quote(content),
                quote(name)
            ));
        }

        fn run(&self) -> Result<String, String> {
            self.run_url("https://example.test/v")
        }

        fn run_url(&self, url: &str) -> Result<String, String> {
            let result = download::run(&self.bin, &self.work, &self.out, url);
            assert!(!self.work.exists(), "the working folder is always removed");
            result
        }

        fn out_names(&self) -> Vec<String> {
            let mut names: Vec<_> = fs::read_dir(&self.out)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    #[test]
    fn d2_launch_failure_has_a_reason() {
        let f = Fixture::new();
        assert!(f.run().unwrap_err().starts_with("Couldn't start yt-dlp:"));
        assert!(f.out_names().is_empty());
    }

    #[test]
    fn d2_actual_process_receives_argv_without_shell_expansion() {
        let f = Fixture::new();
        let name = "日本語 e\u{301} 🎵 & title.webm";
        let log = f.root.join("argv.txt");
        f.script(&format!(
            "printf '%s\\n' \"$@\" > {}\nprintf media > \"$P\"/{}",
            quote(log.to_str().unwrap()),
            quote(name),
        ));
        let marker = f.root.join("SHOULD-NOT-EXIST");
        let url = format!(
            "--exec=touch {}; $(touch {}) `touch {}` & 'quoted' 🎵",
            marker.display(),
            marker.display(),
            marker.display()
        );
        assert_eq!(f.run_url(&url).unwrap(), f.out.join(name).to_str().unwrap());
        let expected = download::args(&f.bin, &f.work, &url)
            .iter()
            .map(|s| s.to_str().unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        assert_eq!(fs::read_to_string(log).unwrap(), expected);
        assert!(!marker.exists());
    }

    #[test]
    fn d2_success_ignores_stdout_noise_and_moves_the_file_out() {
        let f = Fixture::new();
        let name = "日本語 🎵 (OFFICIAL VIDEO) & e\u{301}.webm";
        f.script(&format!(
            "printf media > \"$P\"/{}\nprintf '[download] ??%% speed NaN ETA ???\\r\\nmalformed {{json\\n'\nprintf '/elsewhere/garbage.webm\\r\\n'",
            quote(name),
        ));
        assert_eq!(f.run().unwrap(), f.out.join(name).to_str().unwrap());
        assert_eq!(fs::read(f.out.join(name)).unwrap(), b"media");
        assert_eq!(f.out_names(), [name]);
    }

    #[test]
    fn d2_nonzero_exit_fails_and_leaves_no_partial_files() {
        let f = Fixture::new();
        f.script("printf partial > \"$P\"/partial.webm.part\nprintf 'ERROR: first\\nERROR: final failure 日本語\\nWARNING: cleanup\\n' >&2\nexit 1");
        assert_eq!(f.run().unwrap_err(), "final failure 日本語");
        assert!(f.out_names().is_empty());
    }

    #[test]
    fn d2_no_file_or_an_ambiguous_result_is_failed() {
        let f = Fixture::new();
        f.script("printf ' \\r\\n\\n'");
        assert_eq!(
            f.run().unwrap_err(),
            "yt-dlp finished without saving a file"
        );
        f.script("printf a > \"$P\"/a.webm\nprintf b > \"$P\"/b.webm");
        assert!(f.run().unwrap_err().contains("2 files"));
        f.script("mkdir \"$P\"/Title.webm");
        assert!(f.run().unwrap_err().contains("other than a file"));
        assert!(f.out_names().is_empty());
    }

    #[test]
    fn d2_existing_file_is_never_overwritten_whenever_it_was_created() {
        let f = Fixture::new();
        fs::write(f.out.join("existing.webm"), b"original bytes").unwrap();
        f.saves("existing.webm", "new bytes");
        assert!(
            f.run()
                .unwrap_err()
                .contains("already exists (not overwritten)")
        );
        assert_eq!(
            fs::read(f.out.join("existing.webm")).unwrap(),
            b"original bytes"
        );
    }

    #[test]
    fn d2_a_directory_at_the_target_name_is_not_replaced() {
        let f = Fixture::new();
        fs::create_dir(f.out.join("Title.webm")).unwrap();
        f.saves("Title.webm", "media");
        assert!(f.run().unwrap_err().contains("already exists"));
        assert!(f.out.join("Title.webm").is_dir());
    }

    #[test]
    fn d2_lookalike_names_in_the_output_folder_are_untouched() {
        // Review P1: yt-dlp's merge temp files (e.g. "Foo.temp.webm") used to share the user's
        // folder. Now they live in the working folder, so same-stem files can't collide.
        let f = Fixture::new();
        for (name, bytes) in [("Foo.temp.webm", "earlier"), ("Foo.f251.webm", "other")] {
            fs::write(f.out.join(name), bytes).unwrap();
        }
        f.script("printf tmp > \"$P\"/Foo.temp.webm\nmv \"$P\"/Foo.temp.webm \"$P\"/Foo.webm");
        f.run().unwrap();
        assert_eq!(fs::read(f.out.join("Foo.temp.webm")).unwrap(), b"earlier");
        assert_eq!(fs::read(f.out.join("Foo.f251.webm")).unwrap(), b"other");
        assert_eq!(
            f.out_names(),
            ["Foo.f251.webm", "Foo.temp.webm", "Foo.webm"]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn d2_case_and_unicode_equivalent_names_count_as_existing_on_macos() {
        // APFS/HFS+ defaults treat these as the same name, so the OS itself refuses the move.
        let f = Fixture::new();
        fs::write(f.out.join("Song.webm"), b"original").unwrap();
        fs::write(f.out.join("Cafe\u{301}.webm"), b"decomposed").unwrap();
        f.saves("song.WEBM", "new");
        assert!(f.run().unwrap_err().contains("already exists"));
        f.saves("Caf\u{e9}.webm", "new");
        assert!(f.run().unwrap_err().contains("already exists"));
        assert_eq!(fs::read(f.out.join("Song.webm")).unwrap(), b"original");
        assert_eq!(
            fs::read(f.out.join("Cafe\u{301}.webm")).unwrap(),
            b"decomposed"
        );
    }

    #[test]
    fn d2_missing_output_folder_is_created() {
        let f = Fixture::new();
        fs::remove_dir(&f.out).unwrap();
        f.saves("Title.webm", "media");
        f.run().unwrap();
        assert_eq!(f.out_names(), ["Title.webm"]);
    }

    #[test]
    fn d2_an_existing_working_folder_is_refused_not_reused() {
        let f = Fixture::new();
        fs::create_dir_all(&f.work).unwrap();
        fs::write(f.work.join("stale.webm"), b"stale").unwrap();
        f.saves("Title.webm", "media");
        let result = download::run(&f.bin, &f.work, &f.out, "https://example.test/v");
        assert!(result.unwrap_err().contains("working folder"));
        assert!(f.out_names().is_empty());
    }

    #[test]
    fn d2_process_signal_and_invalid_utf8_stderr_do_not_panic() {
        let f = Fixture::new();
        f.script("kill -TERM $$");
        assert!(f.run().unwrap_err().contains("yt-dlp exited with"));
        f.script("printf 'ERROR: bad \\377 output\\n' >&2\nexit 1");
        assert_eq!(f.run().unwrap_err(), "bad \u{fffd} output");
    }

    #[test]
    fn d2_stdout_and_stderr_larger_than_pipe_buffers_are_drained() {
        let f = Fixture::new();
        f.script("i=0\nwhile [ \"$i\" -lt 4000 ]; do\n printf 'noise-noise-noise-noise-noise-noise-noise-noise\\n'\n printf 'warning-warning-warning-warning-warning-warning\\n' >&2\n i=$((i + 1))\ndone\nprintf media > \"$P\"/large-output.webm");
        assert!(f.run().unwrap().ends_with("large-output.webm"));
    }

    #[test]
    fn d2_finite_slow_process_completes_after_it_writes_the_file() {
        let f = Fixture::new();
        f.script("sleep 0.05\nprintf media > \"$P\"/slow.webm");
        assert!(f.run().unwrap().ends_with("slow.webm"));
    }
}
