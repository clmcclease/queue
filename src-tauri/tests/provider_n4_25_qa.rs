//! Independent QA for #25 / N4: real provider answers through the queue worker, not just
//! parser errors or a fake namer returning an error. Only loopback HTTP; no external service.
#[path = "support/accept.rs"]
mod accept;

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use queue_lib::ai::{self, Endpoint, Namer};
use queue_lib::meta::Meta;
use queue_lib::naming::{self, Template};
use queue_lib::queue::{NameSource, Queue, Status};
use queue_lib::{download, run_naming_job};

fn queued(template: Template) -> (Mutex<Queue>, u64, Meta) {
    let metadata = Meta {
        title: Some("🔥 Café 日本語 [4K] (Official Video)".into()),
        channel: Some("作者".into()),
        upload_date: Some("20261020".into()),
        ..Meta::default()
    };
    let mut queue = Queue::default();
    queue.set_template(template);
    let id = queue.add("https://example.test/n4-qa").unwrap().id;
    queue.fetched(id, Ok(metadata.clone())).unwrap();
    (Mutex::new(queue), id, metadata)
}

fn chat(content: &str) -> String {
    serde_json::json!({"choices":[{"message":{"content":content},"finish_reason":"stop"}]})
        .to_string()
}

/// Serve one complete request. A missing response stalls until the caller releases it, so
/// the real default timeout is exercised without a server-side sleep or a hanging thread.
fn service(body: Option<String>) -> (Endpoint, mpsc::Sender<()>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = Endpoint {
        kind: queue_lib::ai::ApiKind::OpenaiCompatible,
        base_url: format!("http://{}/v1", listener.local_addr().unwrap()),
        model: "qa-fixture".into(),
        key: None,
    };
    let (release, released) = mpsc::channel();
    let worker = thread::spawn(move || {
        let mut stream = accept::accept(&listener);
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = Vec::new();
        let mut buf = [0; 4096];
        loop {
            let n = stream.read(&mut buf).unwrap();
            assert!(n > 0, "client closed before sending the request");
            request.extend_from_slice(&buf[..n]);
            if let Some(end) = request.windows(4).position(|v| v == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]);
                let len = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|n| n.trim().parse::<usize>().ok())
                    })
                    .unwrap();
                if request.len() >= end + 4 + len {
                    break;
                }
            }
        }
        if let Some(body) = body {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        } else {
            let _ = released.recv_timeout(Duration::from_secs(20));
        }
    });
    (endpoint, release, worker)
}

#[test]
fn n4_invalid_real_responses_keep_the_current_tier_one_name_and_rules_tag() {
    let too_long = "日".repeat(151);
    let cases = [
        "not JSON".to_owned(),
        r#"{"choices":[]}"#.into(),
        chat("a plain name instead of JSON"),
        chat(r#"{"filename":null}"#),
        chat(r#"{"filename":"   "}"#),
        chat(r#"{"filename":"Untitled"}"#),
        chat(&serde_json::json!({"filename":too_long}).to_string()),
        chat(r#"{"filename":"Translated title"}"#),
        chat(r#"{"filename":"\u202e\u2066"}"#),
        r#"{"choices":[{"message":{"content":"{\"filename\":\"日本語\"}"},"finish_reason":"length"}]}"#.into(),
    ];
    for template in [Template::Title, Template::ChannelTitle, Template::DateTitle] {
        for body in &cases {
            let (queue, id, metadata) = queued(template);
            let expected = naming::name(&metadata, template);
            let (endpoint, _, server) = service(Some(body.clone()));
            let row = run_naming_job(&queue, id, &endpoint).unwrap();
            server.join().unwrap();
            assert_eq!(row.name.as_deref(), Some(expected.as_str()), "{body}");
            assert_eq!(row.name_source, Some(NameSource::Fallback), "{body}");
            assert_eq!(row.status, Status::Queued);
            assert!(row.notice.is_none(), "a naming error is a quiet fallback");
            assert!(queue.lock().unwrap().awaiting_ai().is_empty());
            let restored = Queue::from_json(&queue.lock().unwrap().to_json()).unwrap();
            assert_eq!(restored.items()[0].name, row.name);
            assert_eq!(restored.items()[0].name_source, Some(NameSource::Fallback));
        }
    }
}

#[test]
fn n4_real_answers_use_windows_rules_and_unicode_byte_limits_before_download() {
    for (answer, expected) in [
        (
            " .. Café\t日本語/α\\β:*?\u{202e} .MP4 ".to_owned(),
            "Café 日本語⧸α⧹β：＊？".to_owned(),
        ),
        ("日".repeat(150), "日".repeat(66)),
    ] {
        let (queue, id, _) = queued(Template::Title);
        let content = serde_json::json!({"filename":answer}).to_string();
        let (endpoint, _, server) = service(Some(chat(&content)));
        let row = run_naming_job(&queue, id, &endpoint).unwrap();
        server.join().unwrap();
        assert_eq!(row.name.as_deref(), Some(expected.as_str()));
        assert_eq!(row.name_source, Some(NameSource::Ai));
        assert!(expected.len() <= naming::MAX_BYTES);
        assert!(expected.chars().count() <= naming::MAX_CHARS);
        assert_eq!(queue.lock().unwrap().start(id).unwrap().name, row.name);
    }
    for (raw, safe) in [("CON.MP4", "CON_"), ("NuL", "NuL_"), ("COM¹.mp3", "COM¹_")] {
        let mut queue = Queue::default();
        let id = queue.add("https://example.test/reserved").unwrap().id;
        queue
            .fetched(
                id,
                Ok(Meta {
                    title: Some("Original title".into()),
                    ..Meta::default()
                }),
            )
            .unwrap();
        let queue = Mutex::new(queue);
        let (endpoint, _, server) =
            service(Some(chat(&serde_json::json!({"filename":raw}).to_string())));
        let row = run_naming_job(&queue, id, &endpoint).unwrap();
        server.join().unwrap();
        assert_eq!(row.name.as_deref(), Some(safe));
        assert_eq!(row.name_source, Some(NameSource::Ai));
    }
}

#[test]
fn n4_the_default_ten_second_timeout_reaches_the_row_as_a_quiet_fallback() {
    let (queue, id, metadata) = queued(Template::ChannelTitle);
    let (endpoint, release, server) = service(None);
    let started = Instant::now();
    let row = run_naming_job(&queue, id, &endpoint).unwrap();
    let elapsed = started.elapsed();
    release.send(()).unwrap();
    server.join().unwrap();
    assert_eq!(ai::TIMEOUT, Duration::from_secs(10));
    assert!(
        elapsed >= Duration::from_secs(8) && elapsed < Duration::from_secs(15),
        "{elapsed:?}"
    );
    assert_eq!(
        row.name,
        Some(naming::name(&metadata, Template::ChannelTitle))
    );
    assert_eq!(row.name_source, Some(NameSource::Fallback));
    assert_eq!(row.status, Status::Queued);
    assert!(row.notice.is_none());
}

struct Fake<F>(F);
impl<F: Fn(&Meta) -> Result<String, String>> Namer for Fake<F> {
    fn name(&self, metadata: &Meta) -> Result<String, String> {
        (self.0)(metadata)
    }
}

#[test]
fn n4_template_changes_during_an_answer_use_the_template_current_on_completion() {
    for succeeds in [true, false] {
        let (queue, id, metadata) = queued(Template::Title);
        let row = run_naming_job(
            &queue,
            id,
            &Fake(|_: &Meta| {
                queue.lock().unwrap().set_template(Template::DateTitle);
                if succeeds {
                    Ok("日本語 – AI".into())
                } else {
                    Err("failed".into())
                }
            }),
        )
        .unwrap();
        assert_eq!(
            row.name,
            Some(if succeeds {
                naming::with_title(&metadata, Template::DateTitle, "日本語 – AI")
            } else {
                naming::name(&metadata, Template::DateTitle)
            })
        );
        assert_eq!(
            row.name_source,
            Some(if succeeds {
                NameSource::Ai
            } else {
                NameSource::Fallback
            })
        );
    }
}

#[test]
fn n4_removing_a_row_mid_answer_does_not_resurrect_it_or_poison_the_queue() {
    let (queue, id, _) = queued(Template::Title);
    let answer = run_naming_job(
        &queue,
        id,
        &Fake(|_: &Meta| {
            queue.lock().unwrap().remove(id).unwrap();
            Ok("日本語 – Too late".into())
        }),
    );
    assert!(answer.is_none());
    assert!(queue.lock().unwrap().items().is_empty());
    assert!(
        queue
            .lock()
            .unwrap()
            .add("https://example.test/next")
            .is_ok()
    );
}

#[test]
fn n4_a_failed_name_does_not_prevent_the_next_row_from_being_named() {
    let (queue, first, metadata) = queued(Template::Title);
    let second = {
        let mut queue = queue.lock().unwrap();
        let id = queue.add("https://example.test/next").unwrap().id;
        queue.fetched(id, Ok(metadata)).unwrap();
        id
    };
    let failed = run_naming_job(&queue, first, &Fake(|_: &Meta| Err("bad output".into()))).unwrap();
    let next =
        run_naming_job(&queue, second, &Fake(|_: &Meta| Ok("日本語 – Next".into()))).unwrap();
    assert_eq!(failed.name_source, Some(NameSource::Fallback));
    assert_eq!(next.name_source, Some(NameSource::Ai));
    assert_eq!(next.name.as_deref(), Some("日本語 – Next"));
}

#[test]
fn n4_ai_names_that_collapse_to_one_filename_preserve_existing_files() {
    let root = std::env::temp_dir().join(format!("queue-n4-25-collision-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let existing = root.join("Café 日本語⧸曲.mp4");
    std::fs::write(&existing, "original").unwrap();
    for (index, raw) in ["Café 日本語/曲.mp4", "Café 日本語⧸曲"]
        .into_iter()
        .enumerate()
    {
        let (queue, id, _) = queued(Template::Title);
        let (endpoint, _, server) =
            service(Some(chat(&serde_json::json!({"filename":raw}).to_string())));
        let row = run_naming_job(&queue, id, &endpoint).unwrap();
        server.join().unwrap();
        let source = root.join(format!("source-{index}"));
        std::fs::write(&source, format!("payload-{index}")).unwrap();
        let basename = format!("{}.mp4", row.name.unwrap());
        let placed = download::place_unique(&source, &root, OsStr::new(&basename)).unwrap();
        assert_eq!(
            placed.file_name().unwrap(),
            OsStr::new(&format!("Café 日本語⧸曲 ({}).mp4", index + 2))
        );
        assert_eq!(
            std::fs::read_to_string(placed).unwrap(),
            format!("payload-{index}")
        );
        assert_eq!(std::fs::read_to_string(&existing).unwrap(), "original");
    }
    std::fs::remove_dir_all(root).unwrap();
}
