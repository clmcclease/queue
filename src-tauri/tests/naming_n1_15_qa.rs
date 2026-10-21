//! Independent QA for #15 / N1: edited names across queue transitions and subprocess retries.
use queue_lib::{
    download::{self, Format, Source},
    meta::Meta,
    naming,
    queue::{NameSource, Queue, Status},
};
use std::path::Path;

fn ready() -> (Queue, u64) {
    let mut queue = Queue::default();
    let id = queue.add("https://example.test/video").unwrap().id;
    queue
        .fetched(
            id,
            Ok(Meta {
                title: Some("🔥 Café 日本語 [4K] (Official Video)".into()),
                ..Meta::default()
            }),
        )
        .unwrap();
    assert_eq!(queue.items()[0].name.as_deref(), Some("Café 日本語"));
    (queue, id)
}

#[test]
fn edited_names_keep_wording_but_reject_paths_controls_and_windows_device_names() {
    let (mut queue, id) = ready();
    for (input, expected) in [
        (
            " \tCafé e\u{301} 日本語 🎵 (Official Video)  ",
            "Café e\u{301} 日本語 🎵 (Official Video)",
        ),
        ("con", "con_"),
        ("NuL.txt", "NuL_.txt"),
        ("COM¹", "COM¹_"),
        ("LPT³.mp4", "LPT³_.mp4"),
        ("../evil\\name:*?\"<>|", "⧸evil⧹name：＊？＂＜＞｜"),
        ("a\0b\nc\u{202e}d", "a b cd"),
        (". .", "Untitled"),
    ] {
        let before = (queue.items()[0].rev, queue.changes());
        let row = queue.rename(id, input).unwrap();
        assert_eq!(row.name.as_deref(), Some(expected), "{input:?}");
        assert_eq!(row.name_source, Some(NameSource::Edited));
        assert!(row.rev > before.0 && queue.changes() > before.1);
        assert_eq!(row.status, Status::Queued);
        assert_eq!(
            row.meta.as_ref().unwrap().title.as_deref(),
            Some("🔥 Café 日本語 [4K] (Official Video)")
        );
    }
}

#[test]
fn edited_unicode_names_fit_the_limits_even_at_utf8_boundaries_and_survive_restart() {
    let (mut queue, id) = ready();
    for input in [
        "x".repeat(151),
        "日".repeat(151),
        "🎵".repeat(151),
        format!("{}日", "é".repeat(99)),
        format!("{}🎵", "日".repeat(66)),
    ] {
        let edited = queue.rename(id, &input).unwrap();
        let name = edited.name.as_ref().unwrap();
        assert!(!name.is_empty() && name.len() <= naming::MAX_BYTES);
        assert!(name.chars().count() <= naming::MAX_CHARS);
        assert!(
            input.starts_with(name),
            "truncation must preserve complete characters"
        );
        let restored = Queue::from_json(&queue.to_json()).unwrap();
        assert_eq!(restored.items()[0].name, edited.name);
        assert_eq!(restored.items()[0].name_source, Some(NameSource::Edited));
    }
}

#[test]
fn edited_name_survives_cancel_processing_retry_failure_and_interrupted_restart() {
    let (mut queue, id) = ready();
    queue.rename(id, "My 日本語 100% %(title)s").unwrap();
    let expected = queue.items()[0].name.clone();
    assert_eq!(queue.start(id).unwrap().name, expected);
    queue.processing(id).unwrap();
    queue.retrying(id, 2).unwrap();
    assert_eq!(queue.cancelled(id).unwrap().name, expected);
    queue.start(id).unwrap();
    queue.finish(id, Err("network timeout".into())).unwrap();
    assert_eq!(queue.retry(id).unwrap().name, expected);
    assert!(
        queue.fetched(id, Ok(Meta::default())).is_none(),
        "a late info result must not overwrite an edit"
    );
    queue.start(id).unwrap();
    let mut restored = Queue::from_json(&queue.to_json()).unwrap();
    assert_eq!(restored.items()[0].status, Status::Queued);
    assert_eq!(restored.items()[0].name, expected);
    assert_eq!(restored.items()[0].name_source, Some(NameSource::Edited));
    assert_eq!(restored.start(id).unwrap().name, expected);
}

#[test]
fn rejected_renames_in_every_nonqueued_state_leave_the_queue_unchanged() {
    let (mut queue, id) = ready();
    queue.rename(id, "Keep me").unwrap();
    for state in ["fetching", "downloading", "processing", "failed", "done"] {
        let mut json: serde_json::Value = serde_json::from_slice(&queue.to_json()).unwrap();
        json["items"][0]["status"] = match state {
            "failed" => serde_json::json!({"state":state, "reason":"offline"}),
            "done" => serde_json::json!({"state":state, "path":"/tmp/Keep me.wav"}),
            // Running states restore as Queued; transition them below instead.
            "downloading" | "processing" => serde_json::json!({"state":"queued"}),
            _ => serde_json::json!({"state":state}),
        };
        let mut q = Queue::from_json(&serde_json::to_vec(&json).unwrap()).unwrap();
        if state == "downloading" || state == "processing" {
            q.start(id).unwrap();
            if state == "processing" {
                q.processing(id).unwrap();
            }
        }
        let before = (q.to_json(), q.items()[0].rev, q.changes());
        assert!(q.rename(id, "Replace me").is_err(), "{state}");
        assert!(q.rename(id + 999, "Replace me").is_err());
        assert_eq!(
            (q.to_json(), q.items()[0].rev, q.changes()),
            before,
            "{state}"
        );
    }
}

#[test]
fn edited_name_is_one_literal_output_argument_for_every_format_and_info_source() {
    let (mut queue, id) = ready();
    let row = queue
        .rename(id, "-x 100% %(title)s %% $(touch hacked); 日本語")
        .unwrap();
    let name = row.name.as_deref().unwrap();
    for format in [
        Format::Default,
        Format::Audio,
        Format::CompatibleVideo,
        Format::CompatibleAudio,
    ] {
        for source in [
            Source::Url("https://example.test/v?x=1&y=2"),
            Source::Info(Path::new("info 日本語.json")),
        ] {
            let args = download::args_named(
                Path::new("bin"),
                Path::new("work"),
                source,
                format,
                Some(name),
            );
            let output: Vec<_> = args.windows(2).filter(|a| a[0] == "-o").collect();
            assert_eq!(output.len(), 1);
            assert_eq!(
                output[0][1],
                "-x 100%% %%(title)s %%%% $(touch hacked); 日本語.%(ext)s"
            );
            assert!(!args.iter().any(|a| a == "--replace-in-metadata"));
        }
    }
}

#[cfg(unix)]
mod subprocess {
    use super::*;
    use queue_lib::sidecar::Sidecar;
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn named_retry_keeps_the_edited_template_when_saved_info_falls_back_to_url() {
        let root =
            Temp(std::env::temp_dir().join(format!("queue-n1-15-qa-{}", std::process::id())));
        fs::create_dir(&root.0).unwrap();
        let bin = root.0.join("bin");
        let out = root.0.join("out");
        fs::create_dir(&bin).unwrap();
        fs::create_dir(&out).unwrap();
        let exe = Sidecar::YtDlp.path_in(&bin);
        // Capture arguments individually, avoiding interpolation of untrusted filename text.
        fs::write(
            &exe,
            r##"#!/bin/sh
base=$(dirname "$0")
if [ -e "$base/once" ]; then n=2; else n=1; fi
printf '%s\n' "$@" > "$base/args-$n"
p=; prev=
for a in "$@"; do
  if [ "$prev" = '-P' ]; then p=$a; fi
  prev=$a
done
if [ "$n" = 1 ]; then
  touch "$base/once"
  printf '%s\n' 'ERROR: HTTP Error 403: Forbidden' >&2
  exit 1
fi
printf '%s\n' 'QUEUE-PROGRESS {broken' 'QUEUE-PROGRESS {"status":7}'
printf '%s\n' 'QUEUE-PROGRESS {"status":"finished","downloaded":5,"total":5}'
printf media > "$p/Edited 日本語.webm"
"##,
        )
        .unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
        let info = root.0.join("info.json");
        fs::write(&info, "{}").unwrap();
        fs::write(out.join("Edited 日本語.webm"), "original").unwrap();
        let (mut queue, id) = ready();
        let name = queue.rename(id, "Edited 日本語").unwrap().name.unwrap();
        let mut retries = Vec::new();
        let mut waits = Vec::new();
        let mut events = Vec::new();
        let saved = download::run_with_retries_named(
            &bin,
            &root.0.join("work"),
            &out,
            "https://example.test/v",
            Some(&info),
            Format::Default,
            Some(&name),
            |delay| waits.push(delay.as_secs()),
            |attempt| retries.push(attempt),
            |event| events.push(event),
        )
        .unwrap();
        for n in [1, 2] {
            let args = fs::read_to_string(bin.join(format!("args-{n}"))).unwrap();
            let args: Vec<_> = args.lines().collect();
            let at = args.iter().position(|a| *a == "-o").unwrap();
            assert_eq!(args[at + 1], "Edited 日本語.%(ext)s");
            assert_eq!(args.contains(&"--load-info-json"), n == 1);
            if n == 2 {
                assert_eq!(&args[args.len() - 2..], ["--", "https://example.test/v"]);
            }
        }
        assert_eq!(retries, [2]);
        assert_eq!(waits, [10]);
        assert_eq!(
            events.len(),
            1,
            "malformed progress cannot forge state changes"
        );
        assert!(matches!(&events[0], download::Event::Progress(p) if p.percent == Some(100.0)));
        assert_eq!(Path::new(&saved.path), out.join("Edited 日本語 (2).webm"));
        assert_eq!(
            fs::read(out.join("Edited 日本語.webm")).unwrap(),
            b"original"
        );
        assert_eq!(fs::read(saved.path).unwrap(), b"media");
        assert!(!root.0.join("work").exists());
    }
}
