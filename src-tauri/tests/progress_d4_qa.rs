//! Independent QA: #8 / D4 progress parsing and the Q5 Processing transition.
use queue_lib::{
    download::{self, Event, Progress},
    meta::Meta,
    queue::{Queue, Status, run_all},
};

fn progress(json: &str, finished: &mut u32) -> Progress {
    match download::parse_line(&format!("QUEUE-PROGRESS {json}"), finished).unwrap() {
        Event::Progress(p) => p,
        other => panic!("expected progress, got {other:?}"),
    }
}

#[test]
fn zero_total_uses_a_positive_estimate() {
    let p = progress(
        r#"{"status":"downloading","downloaded":25,"total":0,"total_estimate":100,"speed":5,"eta":15}"#,
        &mut 0,
    );
    assert_eq!(p.total, Some(100), "zero is not a usable byte total");
    assert_eq!(p.percent, Some(25.0));
}

#[test]
fn bytes_take_precedence_over_estimates_and_fragments() {
    let p = progress(
        r#"{"status":"downloading","downloaded":30,"total":120,"total_estimate":300,"fragment":9,"fragments":10}"#,
        &mut 0,
    );
    assert_eq!((p.percent, p.total), (Some(25.0), Some(120)));
}

#[test]
fn unknown_totals_still_report_speed_and_eta_without_inventing_percent() {
    let p = progress(
        r#"{"status":"downloading","downloaded":1024,"total":null,"total_estimate":null,"speed":512.5,"eta":3.9}"#,
        &mut 0,
    );
    assert_eq!((p.percent, p.total), (None, None));
    assert_eq!((p.speed, p.eta), (Some(512.5), Some(3)));
}

#[test]
fn unusable_byte_counts_fall_back_to_fragments_and_clamp() {
    for (fragment, expected) in [(0, 0.0), (3, 25.0), (24, 100.0)] {
        let json = format!(
            r#"{{"downloaded":-1,"total":-10,"total_estimate":1e308,"fragment":{fragment},"fragments":12}}"#
        );
        assert_eq!(progress(&json, &mut 0).percent, Some(expected));
    }
    assert_eq!(
        progress(r#"{"fragment":3,"fragments":0}"#, &mut 0).percent,
        None
    );
}

#[test]
fn malformed_lines_do_not_advance_streams_and_a_later_valid_line_recovers() {
    let mut finished = 0;
    for json in [
        "not json",
        "null",
        "{",
        "{}{}",
        r#"{"status":"finished","downloaded":"ten"}"#,
        r#"{"status":7}"#,
        r#"{"speed":true}"#,
        r#"{"eta":[]}"#,
        r#"{"downloaded":1e309}"#,
    ] {
        assert_eq!(
            download::parse_line(&format!("QUEUE-PROGRESS {json}"), &mut finished),
            None,
            "{json}"
        );
        assert_eq!(finished, 0);
    }
    assert_eq!(
        progress(
            r#"{"status":"finished","downloaded":4,"total":4}"#,
            &mut finished
        )
        .part,
        1
    );
    assert_eq!(finished, 1);
    assert_eq!(
        progress(
            r#"{"status":"downloading","downloaded":1,"total":4}"#,
            &mut finished
        )
        .part,
        2
    );
}

#[test]
fn negative_and_out_of_range_numbers_are_removed_before_serializing() {
    let p = progress(
        r#"{"downloaded":1e308,"total":18446744073709551616,"speed":-1,"eta":-1}"#,
        &mut 0,
    );
    assert_eq!(
        p,
        Progress {
            part: 1,
            ..Progress::default()
        }
    );
    let value = serde_json::to_value(p).unwrap();
    assert!(value["percent"].is_null() && value["speed"].is_null() && value["eta"].is_null());
}

#[test]
fn postprocessors_only_start_processing_and_do_not_change_stream_number() {
    let mut finished = 2;
    for json in [
        r#"{"status":"processing","postprocessor":"Merger"}"#,
        r#"{"status":"finished","postprocessor":"Merger"}"#,
        r#"{"status":"started","postprocessor":"MoveFiles"}"#,
        r#"{"status":"started","postprocessor":null}"#,
        r#"{"status":"started","postprocessor":3}"#,
    ] {
        assert_eq!(
            download::parse_line(&format!("QUEUE-POST {json}"), &mut finished),
            None
        );
        assert_eq!(finished, 2);
    }
    for name in ["Merger", "ExtractAudio", "FixupM4a"] {
        let json = format!(r#"QUEUE-POST {{"status":"started","postprocessor":"{name}"}}"#);
        assert_eq!(
            download::parse_line(&json, &mut finished),
            Some(Event::Processing(name.into()))
        );
    }
}

fn started_queue() -> (Queue, u64, u64) {
    let mut q = Queue::default();
    let a = q.add("https://example.test/a").unwrap().id;
    let b = q.add("https://example.test/b").unwrap().id;
    for id in [a, b] {
        q.fetched(id, Ok(Meta::default())).unwrap();
    }
    q.start(a).unwrap();
    (q, a, b)
}

#[test]
fn processing_blocks_other_downloads_then_failure_releases_the_slot() {
    let (mut q, a, b) = started_queue();
    let p = q
        .progressed(
            a,
            Progress {
                percent: Some(99.0),
                part: 2,
                ..Progress::default()
            },
        )
        .unwrap();
    let processing = q.processing(a).unwrap();
    assert_eq!(processing.rev, p.rev + 1);
    assert!(processing.progress.is_none());
    assert!(q.start(b).is_err());
    assert!(
        q.processing(a).is_none(),
        "duplicate Processing has no new revision"
    );
    let failed = q.finish(a, Err("merge failed".into())).unwrap();
    assert_eq!(
        failed.status,
        Status::Failed {
            reason: "merge failed".into()
        }
    );
    assert_eq!(failed.attempt, 0);
    assert!(failed.progress.is_none());
    assert!(!q.is_busy());
    q.start(b).unwrap();
    assert!(
        q.progressed(a, Progress::default()).is_none(),
        "late progress cannot revive a failed row"
    );
}

#[test]
fn retry_from_processing_clears_progress_and_accepts_new_stream_updates() {
    let (mut q, a, _) = started_queue();
    q.processing(a).unwrap();
    let retry = q.retrying(a, 2).unwrap();
    assert_eq!(
        (retry.status, retry.attempt, retry.progress),
        (Status::Downloading, 2, None)
    );
    let p = q
        .progressed(
            a,
            Progress {
                part: 1,
                percent: Some(5.0),
                ..Progress::default()
            },
        )
        .unwrap();
    let wire = serde_json::to_value(p).unwrap();
    assert_eq!(wire["progress"]["percent"], 5.0);
    assert_eq!(wire["status"]["state"], "downloading");
    assert!(wire.get("info").is_none());
}

#[test]
fn a_row_that_fails_while_processing_does_not_stop_the_next_one() {
    let mut q = Queue::default();
    let mut ready = |url: &str| {
        let id = q.add(url).unwrap().id;
        q.fetched(id, Ok(Meta::default())).unwrap();
        id
    };
    let (a, b) = (ready("https://e.test/a"), ready("https://e.test/b"));
    let q = std::sync::Mutex::new(q);
    let mut order = Vec::new();
    run_all(
        &q,
        |id| {
            let mut q = q.lock().unwrap();
            q.start(id).unwrap();
            order.push(id);
            if id == a {
                assert!(q.processing(id).is_some());
                q.finish(id, Err("Postprocessing: ffmpeg failed".into()))
                    .unwrap();
            } else {
                q.finish(id, Ok("/d/b.webm".into())).unwrap();
            }
            true
        },
        || panic!("nothing pending"),
    );
    assert_eq!(order, [a, b]);
    let q = q.lock().unwrap();
    assert!(matches!(&q.items()[0].status, Status::Failed { reason } if reason.contains("ffmpeg")));
    assert!(matches!(q.items()[1].status, Status::Done { .. }));
}
