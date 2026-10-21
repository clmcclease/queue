//! #25 / N4: the naming worker's job (`run_naming_job`) with fake namers, and with the real
//! `ai::ask` against a fake OpenAI server on 127.0.0.1. No model, no network.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Mutex;

use queue_lib::ai::{self, Endpoint, Namer};
use queue_lib::meta::Meta;
use queue_lib::queue::{NameSource, Queue, Status};
use queue_lib::run_naming_job;

/// A namer from a closure.
struct Fake<F: Fn(&Meta) -> Result<String, String>>(F);

impl<F: Fn(&Meta) -> Result<String, String>> Namer for Fake<F> {
    fn name(&self, meta: &Meta) -> Result<String, String> {
        (self.0)(meta)
    }
}

fn queued(title: &str) -> (Mutex<Queue>, u64) {
    let mut q = Queue::default();
    let id = q.add("https://e.test/v").unwrap().id;
    q.fetched(
        id,
        Ok(Meta {
            title: Some(title.into()),
            ..Default::default()
        }),
    )
    .unwrap();
    (Mutex::new(q), id)
}

fn row(q: &Mutex<Queue>, id: u64) -> (Option<String>, Option<NameSource>, Status) {
    let q = q.lock().unwrap();
    let item = q.items().iter().find(|i| i.id == id).unwrap();
    (item.name.clone(), item.name_source, item.status.clone())
}

#[test]
fn n4_an_answer_names_the_row_and_a_failure_tags_it_rules() {
    let (q, id) = queued("daft punk get lucky [4K]");
    let item = run_naming_job(
        &q,
        id,
        &Fake(|m: &Meta| {
            assert_eq!(m.title.as_deref(), Some("daft punk get lucky [4K]"));
            Ok("Daft Punk – Get Lucky".into())
        }),
    )
    .unwrap();
    assert_eq!(item.name.as_deref(), Some("Daft Punk – Get Lucky"));
    assert_eq!(item.name_source, Some(NameSource::Ai));

    let (q, id) = queued("daft punk get lucky [4K]");
    let item = run_naming_job(&q, id, &Fake(|_: &Meta| Err("timeout".into()))).unwrap();
    assert_eq!(
        item.name.as_deref(),
        Some("daft punk get lucky"),
        "the Tier-1 name"
    );
    assert_eq!(item.name_source, Some(NameSource::Fallback));
}

#[test]
fn n4_a_panicking_namer_fails_only_its_row() {
    let (q, id) = queued("Title");
    let item = run_naming_job(&q, id, &Fake(|_: &Meta| panic!("model crashed"))).unwrap();
    assert_eq!(item.name_source, Some(NameSource::Fallback));
    // The same queue serves the next row: the panic happened with its lock released.
    let id2 = {
        let mut queue = q.lock().unwrap();
        let id2 = queue.add("https://e.test/next").unwrap().id;
        let meta = Meta {
            title: Some("Next".into()),
            ..Default::default()
        };
        queue.fetched(id2, Ok(meta)).unwrap();
        id2
    };
    assert!(!q.is_poisoned());
    let next = run_naming_job(&q, id2, &Fake(|_: &Meta| Ok("Next one".into()))).unwrap();
    assert_eq!(
        next.name.as_deref(),
        Some("Next one"),
        "the worker carries on"
    );
}

#[test]
fn n4_the_lock_is_free_while_the_namer_works_and_late_answers_never_win() {
    // The namer edits the row (as the user would, mid-answer): proves the lock isn't held, and
    // the late answer must not replace the hand-edited name.
    let (q, id) = queued("Title");
    let late = run_naming_job(
        &q,
        id,
        &Fake(|_: &Meta| {
            q.lock().unwrap().rename(id, "Typed by hand").unwrap();
            Ok("AI name".into())
        }),
    );
    assert!(late.is_none());
    assert_eq!(row(&q, id).0.as_deref(), Some("Typed by hand"));

    // A download that started meanwhile keeps the name it started with.
    let (q, id) = queued("Title");
    let late = run_naming_job(
        &q,
        id,
        &Fake(|_: &Meta| {
            q.lock().unwrap().start(id).unwrap();
            Ok("AI name".into())
        }),
    );
    assert!(late.is_none());
    assert_eq!(row(&q, id).0.as_deref(), Some("Title"));
    assert_eq!(row(&q, id).2, Status::Downloading);

    // A provider change meanwhile drops the answer; the row is listed to ask again.
    let (q, id) = queued("Title");
    let late = run_naming_job(
        &q,
        id,
        &Fake(|_: &Meta| {
            assert_eq!(q.lock().unwrap().renaming(), [id]);
            Ok("From the old provider".into())
        }),
    );
    assert!(late.is_none());
    assert_eq!(row(&q, id).1, Some(NameSource::Rules));
}

#[test]
fn n4_a_row_that_is_gone_or_edited_is_not_asked() {
    let (q, id) = queued("Title");
    q.lock().unwrap().rename(id, "Mine").unwrap();
    let asked = std::cell::Cell::new(false);
    assert!(
        run_naming_job(
            &q,
            id,
            &Fake(|_: &Meta| {
                asked.set(true);
                Ok("x".into())
            })
        )
        .is_none()
    );
    assert!(!asked.get(), "no request for a hand-edited row");
    assert!(run_naming_job(&q, 999, &Fake(|_: &Meta| panic!("never asked"))).is_none());
}

/// The real request path: an `Endpoint` (as #30/#32 will resolve) is a `Namer` that calls
/// `ai::ask`, here against a loopback fake.
#[test]
fn n4_the_real_ask_path_names_the_row() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://127.0.0.1:{}/v1",
        listener.local_addr().unwrap().port()
    );
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0; 65536];
        let mut seen = Vec::new();
        while !String::from_utf8_lossy(&seen).contains("\r\n\r\n") || !seen.ends_with(b"}") {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            seen.extend_from_slice(&buf[..n]);
        }
        let body = serde_json::json!({"choices": [{"message": {"content": r#"{"filename": "Clean Name"}"#}}]}).to_string();
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
    });
    let (q, id) = queued("messy title");
    let endpoint = Endpoint {
        base_url: url,
        model: "local".into(),
        key: None,
    };
    let item = run_naming_job(&q, id, &endpoint).unwrap();
    server.join().unwrap();
    assert_eq!(
        (item.name.as_deref(), item.name_source),
        (Some("Clean Name"), Some(NameSource::Ai))
    );
    assert!(
        ai::endpoint(
            &queue_lib::settings::Settings::default(),
            &queue_lib::secrets::Memory::default()
        )
        .is_err(),
        "and the local model isn't set up yet"
    );
}
