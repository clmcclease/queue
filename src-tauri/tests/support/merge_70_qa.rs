//! Independent QA for #70 / D2, D4. Keep these in the loopback target CI already invokes.
use super::*;
use download::{Event, Format};

const DASH_VIDEO: &str = r#"<video><source src="/media.mpd" type="application/dash+xml"></video>"#;

/// Compare encoded packet payloads, rather than just codec names: a successful remux must
/// preserve the source stream's data. Container timestamps and metadata may change.
fn packet_hashes(bin: &Path, file: &Path, stream: &str) -> Vec<String> {
    let output = std::process::Command::new(
        bin.join(format!("queue-ffprobe{}", std::env::consts::EXE_SUFFIX)),
    )
    .args([
        "-v",
        "error",
        "-select_streams",
        stream,
        "-show_packets",
        "-show_entries",
        "packet=data_hash",
        "-show_data_hash",
        "sha256",
        "-of",
        "json",
    ])
    .arg(file)
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "{}: {}",
        file.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let hashes: Vec<_> = json["packets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|packet| packet["data_hash"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        !hashes.is_empty(),
        "{} has no {stream} packets",
        file.display()
    );
    hashes
}

fn assert_merge_order(events: &[Event]) {
    let merger = events
        .iter()
        .position(|event| matches!(event, Event::Processing(name) if name == "Merger"))
        .expect("real ffmpeg merging must report Processing");
    for part in [1, 2] {
        assert!(
            events[..merger].iter().any(|event| {
                matches!(event, Event::Progress(p) if p.part == part && p.percent == Some(100.0))
            }),
            "stream {part} must reach 100% before Merger: {events:?}"
        );
    }
    assert!(
        events[merger + 1..]
            .iter()
            .all(|event| !matches!(event, Event::Progress(_))),
        "late progress must not replace Processing: {events:?}"
    );
}

#[test]
fn d70_malformed_merge_transcript_does_not_advance_parts_or_hide_processing() {
    let transcript = [
        "[download] preparing video and audio",
        r#"QUEUE-PROGRESS {"status":"finished","downloaded":"broken"}"#,
        r#"QUEUE-PROGRESS {"status":"finished","downloaded":4,"total":4}"#,
        "QUEUE-PROGRESS {",
        r#"QUEUE-POST {"status":"started","postprocessor":42}"#,
        r#"QUEUE-PROGRESS {"status":"finished","downloaded":8,"total":8}"#,
        r#"QUEUE-POST {"status":"started","postprocessor":"Merger"}"#,
        r#"QUEUE-POST {"status":"finished","postprocessor":"Merger"}"#,
        r#"QUEUE-POST {"status":"started","postprocessor":"MoveFiles"}"#,
    ];
    let mut finished = 0;
    let events: Vec<_> = transcript
        .iter()
        .filter_map(|line| download::parse_line(line, &mut finished))
        .collect();
    assert_eq!(finished, 2);
    assert_eq!(events.len(), 3);
    assert_merge_order(&events);
}

#[test]
#[ignore = "requires installed pinned sidecars; loopback HTTP only"]
fn d70_saved_info_merges_preserve_packets_and_sanitize_collision_names() {
    for (title, format, video, audio, streams) in [
        (
            "CON",
            Format::CompatibleVideo,
            "v_h264.mp4",
            "a_aac.m4a",
            "h264,aac",
        ),
        (
            " 日本語 🎵 é : * ? &lt;noise&gt; &amp; title ",
            Format::Default,
            "v_mjpeg.mp4",
            "a_flac.mp4",
            "mjpeg,flac",
        ),
    ] {
        let f = LocalDownload::serve(title, DASH_VIDEO, |bin| {
            let routes = merge_routes(bin, true);
            for (path, _, bytes) in &routes {
                if *path != "/media.mpd" {
                    fs::write(bin.join(path.trim_start_matches('/')), bytes).unwrap();
                }
            }
            routes
        });
        let (_, raw) = queue_lib::meta::fetch_raw(&f.bin, &f.url, Duration::from_secs(30)).unwrap();
        let info = f.root.join("saved merge 日本語.json");
        fs::write(&info, raw).unwrap();
        let run = |work_name: &str| {
            let work = f.root.join(work_name);
            let mut events = Vec::new();
            let saved = download::run_with_retries_saved(
                &f.bin,
                &work,
                &f.out,
                &f.url,
                Some(&info),
                format,
                |_| panic!("local merge must not back off"),
                |_| panic!("local merge must not retry"),
                |event| events.push(event),
            )
            .unwrap();
            assert!(!work.exists(), "staging must be removed");
            assert_merge_order(&events);
            let path = PathBuf::from(&saved.path);
            assert_eq!(path.parent(), Some(f.out.as_path()));
            assert_eq!(probe_streams(&f.bin, &path), streams);
            assert_eq!(format.notice_for(&path, &saved.codecs), None);
            for (kind, source) in [("v:0", video), ("a:0", audio)] {
                assert_eq!(
                    packet_hashes(&f.bin, &path, kind),
                    packet_hashes(&f.bin, &f.bin.join(source), kind),
                    "{format:?}: {kind} must be copied without re-encoding"
                );
            }
            path
        };
        let first = run("first merge");
        let original = fs::read(&first).unwrap();
        let name = first.file_name().unwrap().to_str().unwrap();
        assert!(!name.contains(['<', '>', ':', '"', '/', '\\', '|', '?', '*']));
        assert_ne!(name.split('.').next().unwrap().to_uppercase(), "CON");
        if title.contains("日本語") {
            assert!(name.contains("日本語"));
        }
        let (stem, ext) = name.rsplit_once('.').unwrap();
        let occupied = f.out.join(format!("{stem} (2).{ext}"));
        fs::write(&occupied, b"keep occupied second name").unwrap();
        let third = run("collision merge");
        assert_eq!(third, f.out.join(format!("{stem} (3).{ext}")));
        assert_eq!(fs::read(first).unwrap(), original);
        assert_eq!(fs::read(occupied).unwrap(), b"keep occupied second name");
        assert_eq!(fs::read_dir(&f.out).unwrap().count(), 3);
    }
}

#[test]
#[ignore = "requires installed pinned sidecars; loopback HTTP only"]
fn d70_h264_without_aac_merges_and_reports_the_audio_fallback() {
    let f = LocalDownload::serve("H264 without AAC", DASH_VIDEO, |bin| {
        let mut routes = merge_routes(bin, true);
        let (_, _, mpd) = routes
            .iter_mut()
            .find(|route| route.0 == "/media.mpd")
            .unwrap();
        let mut manifest = String::from_utf8(mpd.clone()).unwrap();
        let start = manifest.find("<Representation id=\"a_aac.m4a\"").unwrap();
        let end = start
            + manifest[start..].find("</Representation>").unwrap()
            + "</Representation>".len();
        manifest.replace_range(start..end, "");
        *mpd = manifest.into_bytes();
        routes
    });
    let work = f.root.join("no aac merge");
    let mut events = Vec::new();
    let saved = download::run_with_retries_saved(
        &f.bin,
        &work,
        &f.out,
        &f.url,
        None,
        Format::CompatibleVideo,
        |_| panic!("local merge must not back off"),
        |_| panic!("local merge must not retry"),
        |event| events.push(event),
    )
    .unwrap();
    let path = PathBuf::from(&saved.path);
    assert_eq!(probe_streams(&f.bin, &path), "h264,flac");
    assert_eq!(path.extension().unwrap(), "mkv");
    let notice = Format::CompatibleVideo
        .notice_for(&path, &saved.codecs)
        .unwrap();
    assert!(notice.starts_with("No AAC audio was available"), "{notice}");
    assert!(
        notice.contains("FLAC") && notice.contains(".mkv"),
        "{notice}"
    );
    assert_merge_order(&events);
    assert!(!work.exists());
    assert_eq!(fs::read_dir(&f.out).unwrap().count(), 1);
}
