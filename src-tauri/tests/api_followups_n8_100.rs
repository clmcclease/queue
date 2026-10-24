//! #100 part 2: tests Codex suggested in #91's reviews, through the real naming job. The third
//! one (a failed `settings_set_api` save during a kind change) needs the Tauri command layer and
//! is #84's.
#[path = "support/accept.rs"]
mod accept;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use queue_lib::ai::{self, ApiKind, Provider};
use queue_lib::meta::Meta;
use queue_lib::queue::{NameSource, Queue};
use queue_lib::run_naming_job;
use queue_lib::secrets::Memory;
use queue_lib::settings::Settings;

const KEY: &str = "sk-ant-qa-100";

/// Answers one request with `status` and `body`, and hands back what it received.
fn serve(status: u16, body: String) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let request = thread::spawn(move || {
        let mut stream = accept::accept(&listener);
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut received = Vec::new();
        let mut buf = [0; 4096];
        loop {
            let n = stream.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            received.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&received);
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text[..end]
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if received.len() >= end + 4 + length {
                    break;
                }
            }
        }
        let reply = format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(reply.as_bytes());
        String::from_utf8_lossy(&received).into_owned()
    });
    (url, request)
}

/// A port that records whether anything connected, and never answers.
fn recorder() -> (String, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!(
        "http://127.0.0.1:{}/v1",
        listener.local_addr().unwrap().port()
    );
    (url, listener)
}

fn anthropic_answer(name: &str) -> String {
    let text = serde_json::json!({ "filename": name }).to_string();
    serde_json::json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": text}]})
        .to_string()
}

fn settings(kind: ApiKind, base_url: &str) -> Settings {
    Settings {
        naming_provider: Provider::Api,
        api_kind: kind,
        api_base_url: base_url.into(),
        api_model: "qa-model".into(),
        ..Default::default()
    }
}

fn queued(q: &mut Queue, url: &str, title: &str) -> u64 {
    let id = q.add(url).unwrap().id;
    let meta = Meta {
        title: Some(title.into()),
        ..Default::default()
    };
    q.fetched(id, Ok(meta)).unwrap();
    id
}

/// The user switches the service from OpenAI-compatible to Anthropic while a job is still
/// reading the key: that job sends nothing anywhere, and the next generation asks Anthropic.
#[test]
fn n8_a_kind_switched_while_the_key_is_read_never_sends_the_stale_job() {
    let (openai, old) = recorder();
    let (anthropic, request) = serve(200, anthropic_answer("From Anthropic"));
    let current = Mutex::new(settings(ApiKind::OpenaiCompatible, &openai));
    let store = Memory::with(KEY);
    let mut q = Queue::default();
    let id = queued(&mut q, "https://e.test/kind", "Raw title [4K]");
    let q = Mutex::new(q);
    let stale = ai::Resolving(|| {
        // The endpoint was resolved for the old kind; the switch lands before the send.
        let endpoint = ai::endpoint(&current.lock().unwrap(), &store, ai::LocalModel::Missing);
        *current.lock().unwrap() = settings(ApiKind::Anthropic, &anthropic);
        q.lock().unwrap().renaming();
        endpoint
    });
    assert!(
        run_naming_job(&q, id, &stale).is_none(),
        "the stale answer is dropped"
    );
    assert!(old.accept().is_err(), "nothing reached the old service");
    assert_eq!(
        q.lock().unwrap().items()[0].name_source,
        Some(NameSource::Rules)
    );

    let fresh =
        ai::Resolving(|| ai::endpoint(&current.lock().unwrap(), &store, ai::LocalModel::Missing));
    assert!(run_naming_job(&q, id, &fresh).is_some());
    let received = request.join().unwrap();
    assert!(received.starts_with("POST /v1/messages "), "{received}");
    assert!(
        received
            .to_ascii_lowercase()
            .contains(&format!("x-api-key: {}", KEY.to_ascii_lowercase()))
    );
    let row = q.lock().unwrap().items()[0].clone();
    assert_eq!(row.name.as_deref(), Some("From Anthropic"));
    assert_eq!(row.name_source, Some(NameSource::Ai));
}

/// An Anthropic failure tags its row "rules" and doesn't stop the next row being named.
#[test]
fn n8_an_anthropic_failure_then_a_success_tags_each_row_correctly() {
    let store = Memory::with(KEY);
    let mut q = Queue::default();
    let first = queued(&mut q, "https://e.test/first", "First [HD]");
    let second = queued(&mut q, "https://e.test/second", "Second [HD]");
    let q = Mutex::new(q);

    let (failing, request) = serve(529, r#"{"type":"error"}"#.into());
    let setup = settings(ApiKind::Anthropic, &failing);
    let resolve = ai::Resolving(|| ai::endpoint(&setup, &store, ai::LocalModel::Missing));
    assert!(
        run_naming_job(&q, first, &resolve).is_some(),
        "asked, and failed"
    );
    request.join().unwrap();

    let (working, request) = serve(200, anthropic_answer("Second"));
    let setup = settings(ApiKind::Anthropic, &working);
    let resolve = ai::Resolving(|| ai::endpoint(&setup, &store, ai::LocalModel::Missing));
    assert!(run_naming_job(&q, second, &resolve).is_some());
    request.join().unwrap();

    let q = q.lock().unwrap();
    let rows = q.items();
    assert_eq!(
        (rows[0].name.as_deref(), rows[0].name_source),
        (Some("First"), Some(NameSource::Fallback))
    );
    assert_eq!(
        (rows[1].name.as_deref(), rows[1].name_source),
        (Some("Second"), Some(NameSource::Ai))
    );
}
