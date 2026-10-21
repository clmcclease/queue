//! Independent #77 / D2, N2 checks, included in CI's existing loopback target.
use super::*;
use std::ffi::OsStr;

#[test]
fn d77_numbering_preserves_utf8_and_extension_at_each_suffix_width() {
    for alphabet in ["日", "🎵", "e\u{301}", "日🎵a"] {
        for budget in [197, 198, 199, 200, 201, 247, 248, 249, 250] {
            let mut stem = String::new();
            while stem.len() + alphabet.len() <= budget {
                stem.push_str(alphabet);
            }
            let name = format!("{stem}.webm");
            for n in [2, 9, 10, 99, 100, 999] {
                let numbered = download::numbered(OsStr::new(&name), n);
                let numbered = numbered
                    .to_str()
                    .expect("numbering must retain valid UTF-8");
                let suffix = format!(" ({n}).webm");
                assert!(numbered.len() <= download::MAX_NAME_BYTES, "{numbered}");
                let prefix = numbered.strip_suffix(&suffix).unwrap();
                assert!(
                    stem.starts_with(prefix),
                    "must preserve the original prefix"
                );
                assert!(!prefix.contains('\u{fffd}'), "no lossy decoding");
                if name.len() + suffix.len() - ".webm".len() <= download::MAX_NAME_BYTES {
                    assert_eq!(prefix, stem, "short enough names must not be shortened");
                }
            }
        }
    }
}

/// Ask upstream to evaluate the production argv; comparing against the old argv catches
/// accidental changes to short names without implementing another filename sanitizer here.
#[test]
#[ignore = "requires installed pinned sidecars; saved-info simulation only"]
fn d77_short_reserved_and_messy_names_match_previous_windows_rules() {
    let f = LocalDownload::new("filename simulation");
    let info = f.root.join("filename-info.json");
    let work = f.root.join("simulated");
    fs::create_dir(&work).unwrap();
    for title in [
        "CON",
        "NUL",
        "AUX",
        "PRN",
        "COM1",
        "LPT9",
        "con.txt",
        "NUL.日本語",
        " AC/DC: \"Back\" <in> Black? | a*b \\ c ",
        " 日本語 🎵 e\u{301} (OFFICIAL VIDEO) ",
        "%(title)s %s $HOME `literal`",
        "trailing dots... ",
        "line\nwith\ttabs",
        "Podcast 01:02:03",
    ] {
        fs::write(
            &info,
            serde_json::to_vec(&serde_json::json!({
                "id": "qa77", "title": title, "url": "http://127.0.0.1:9/media.wav",
                "ext": "wav", "extractor": "generic", "extractor_key": "Generic"
            }))
            .unwrap(),
        )
        .unwrap();
        let args = download::args_for(&f.bin, &work, download::Source::Info(&info));
        let mut legacy = Vec::new();
        let mut i = 0;
        while i < args.len() {
            if args[i] == "--replace-in-metadata" {
                i += 4;
            } else if args[i] == "-o" {
                legacy.extend(["-o".into(), "%(title)s.%(ext)s".into()]);
                i += 2;
            } else {
                legacy.push(args[i].clone());
                i += 1;
            }
        }
        let evaluate = |argv: &[std::ffi::OsString]| {
            let result = std::process::Command::new(f.bin.join(Sidecar::YtDlp.file_name()))
                .args(argv)
                .args(["--simulate", "--print", "filename"])
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{title:?}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            String::from_utf8(result.stdout).unwrap().trim().to_owned()
        };
        let current = evaluate(&args);
        assert_eq!(current, evaluate(&legacy), "short title {title:?}");
        let name = Path::new(&current).file_name().unwrap().to_str().unwrap();
        assert!(!name.contains(['<', '>', ':', '"', '/', '\\', '|', '?', '*']));
    }
}

/// The generic HTML extractor adds " (1)", hiding bare device names in earlier fixtures.
/// Saved metadata can contain the actual title without that suffix.
#[test]
#[ignore = "requires installed pinned sidecars; loopback HTTP only"]
fn d77_saved_info_bare_windows_device_title_is_sanitized_before_download() {
    // #15: downloads are named by the naming engine (#14), which handles device names. The
    // gate this test had until #15 (`QUEUE_QA_KNOWN_GAPS`) is gone.
    let f = LocalDownload::new("reserved title source");
    let info = f.root.join("reserved-info.json");
    fs::write(
        &info,
        serde_json::to_vec(&serde_json::json!({
            "id": "qa77", "title": "CON", "url": f.url.replace("/watch", "/media.wav"),
            "ext": "wav", "extractor": "generic", "extractor_key": "Generic"
        }))
        .unwrap(),
    )
    .unwrap();
    let meta = queue_lib::meta::Meta {
        title: Some("CON".into()),
        ..Default::default()
    };
    let stem = queue_lib::naming::name(&meta, queue_lib::naming::Template::Title);
    let result = download::run_with_retries_named(
        &f.bin,
        &f.root.join("reserved-run"),
        &f.out,
        &f.url,
        Some(&info),
        download::Format::Default,
        Some(&stem),
        |_| panic!("loopback must not retry"),
        |_| panic!("loopback must not retry"),
        |_| {},
    )
    .unwrap();
    let saved = PathBuf::from(result.path);
    assert_eq!(fs::read(&saved).unwrap(), silence_wav());
    // Safety, not a spelling: Windows checks the name up to the first dot, ignoring trailing
    // spaces, case-insensitively (so `CON.wav` and `con .wav` are both the device).
    let name = saved.file_name().unwrap().to_str().unwrap();
    let base = name.split('.').next().unwrap().trim_end().to_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&base.as_str())
        || ["COM", "LPT"].iter().any(|dev| {
            base.strip_prefix(dev)
                .is_some_and(|n| n.chars().count() == 1 && "0123456789¹²³".contains(n))
        });
    assert!(
        !reserved,
        "a real download must not use a Windows device name: {}",
        saved.display()
    );
}

/// WAV-only tests never exercise .f<format-id> and .temp names. Merge long mixed titles,
/// including widening characters, then collide through saved metadata and preserve both files.
#[test]
#[ignore = "requires installed pinned sidecars; loopback HTTP only"]
fn d77_long_mixed_titles_merge_and_skip_occupied_collision_names() {
    for (title, format) in [
        ("日🎵e\u{301}:?*|/\\".repeat(45), download::Format::Default),
        ("🎵日".repeat(80), download::Format::CompatibleVideo),
    ] {
        let f = LocalDownload::serve(
            &title,
            r#"<video><source src="/media.mpd" type="application/dash+xml"></video>"#,
            |bin| merge_routes(bin, true),
        );
        let (_, raw) = queue_lib::meta::fetch_raw(&f.bin, &f.url, Duration::from_secs(30)).unwrap();
        let info = f.root.join("saved.json");
        fs::write(&info, raw).unwrap();
        let run = |tag: &str, source| {
            let work = f.root.join(tag);
            let mut events = Vec::new();
            let saved = download::run_with_retries_saved(
                &f.bin,
                &work,
                &f.out,
                &f.url,
                source,
                format,
                |_| panic!("loopback must not retry"),
                |_| panic!("loopback must not retry"),
                |event| events.push(event),
            )
            .unwrap();
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, download::Event::Processing(p) if p == "Merger"))
            );
            assert!(
                !work.exists(),
                "stream/merge temporary files must be cleaned"
            );
            PathBuf::from(saved.path)
        };
        let first = run("url-run", None);
        let original = fs::read(&first).unwrap();
        let name = first.file_name().unwrap().to_str().unwrap();
        let (stem, ext) = name.rsplit_once('.').unwrap();
        assert!(stem.len() <= 200, "byte budget: {name}");
        for suffix in [".part", ".f137.mp4.part", ".temp.mkv", ".ytdl"] {
            assert!(format!("{stem}{suffix}").len() <= download::MAX_NAME_BYTES);
        }
        assert_eq!(
            probe_streams(&f.bin, &first),
            if format == download::Format::Default {
                "mjpeg,flac"
            } else {
                "h264,aac"
            }
        );
        let second = f.out.join(format!("{stem} (2).{ext}"));
        fs::write(&second, b"another writer's file").unwrap();
        let third = run("saved-info-run", Some(info.as_path()));
        assert_eq!(third, f.out.join(format!("{stem} (3).{ext}")));
        assert!(third.file_name().unwrap().to_str().unwrap().len() <= download::MAX_NAME_BYTES);
        assert_eq!(fs::read(&first).unwrap(), original);
        assert_eq!(fs::read(&second).unwrap(), b"another writer's file");
        assert_eq!(probe_streams(&f.bin, &third), probe_streams(&f.bin, &first));
        assert_eq!(fs::read_dir(&f.out).unwrap().count(), 3);
    }
}
