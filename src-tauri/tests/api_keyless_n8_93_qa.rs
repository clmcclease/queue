//! Independent #93 / N8 / D-035 regressions. Bounded loopback fixtures and fake stores only.
#[path = "support/accept.rs"]
mod accept;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use queue_lib::ai::{self, Provider};
use queue_lib::meta::Meta;
use queue_lib::queue::{NameSource, Queue};
use queue_lib::run_naming_job;
use queue_lib::secrets::{Memory, Secrets};
use queue_lib::settings::Settings;

fn chat(name: &str) -> String {
    let content = serde_json::json!({"filename": name}).to_string();
    serde_json::json!({"choices": [{"message": {"content": content}}]}).to_string()
}

/// The stop channel lets a successful no-request check finish without waiting for accept.
/// A deadline also bounds fixture cleanup when a regression prevents the expected request.
struct Service {
    url: String,
    stop: mpsc::Sender<()>,
    worker: thread::JoinHandle<Option<String>>,
}

impl Service {
    fn start(status: u16, body: String, stall: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let (stop, stopped) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut stream = accept::accept_until(&listener, || stopped.try_recv().is_ok())?;
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
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
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap();
                    if received.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            if stall {
                let _ = stopped.recv_timeout(Duration::from_secs(15));
            } else {
                let response = format!(
                    "HTTP/1.1 {status} QA\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
            Some(String::from_utf8(received).unwrap())
        });
        Self { url, stop, worker }
    }

    fn finish(self) -> Option<String> {
        let _ = self.stop.send(());
        self.worker.join().unwrap()
    }
}

fn settings(url: &str) -> Settings {
    Settings {
        naming_provider: Provider::Api,
        api_base_url: url.into(),
        api_model: "qa-model".into(),
        ..Default::default()
    }
}

fn queued(title: &str) -> (Mutex<Queue>, u64) {
    let mut queue = Queue::default();
    let id = queue.add("https://example.test/qa32").unwrap().id;
    queue
        .fetched(
            id,
            Ok(Meta {
                title: Some(title.into()),
                ..Default::default()
            }),
        )
        .unwrap();
    (Mutex::new(queue), id)
}

#[test]
fn unreadable_store_still_sends_unicode_metadata_and_sanitizes_reserved_answers() {
    for (title, answer, expected) in [
        (
            "  Café 日本語 [4K] 🔥  ",
            "Café 日本語/曲.MP4",
            "Café 日本語⧸曲",
        ),
        ("Original", "CON.mp4", "CON_"),
        ("Original", "LPT9.webm", "LPT9_"),
    ] {
        let service = Service::start(200, chat(answer), false);
        let setup = settings(&service.url);
        let store = Memory::broken();
        let resolve = ai::Resolving(|| ai::endpoint(&setup, &store, ai::LocalModel::Missing));
        let (queue, id) = queued(title);
        let row = run_naming_job(&queue, id, &resolve).expect("the service is asked");
        let request = service.finish().expect("request received");
        assert_eq!(row.name.as_deref(), Some(expected));
        assert_eq!(row.name_source, Some(NameSource::Ai));
        assert!(!request.to_ascii_lowercase().contains("authorization:"));
        assert!(!request.to_ascii_lowercase().contains("x-api-key:"));
        assert!(
            request.contains(title.trim()),
            "metadata survives JSON encoding"
        );
        assert_eq!(
            ai::unavailable(&setup, &store, ai::LocalModel::Missing),
            None
        );
        assert!(ai::store_unreachable(&setup, &store));
    }
}

#[test]
fn refused_keyless_requests_keep_the_rules_name_and_mark_fallback() {
    for status in [401, 403, 500] {
        let service = Service::start(
            status,
            r#"{"error":"private-response-marker"}"#.into(),
            false,
        );
        let setup = settings(&service.url);
        let store = Memory::broken();
        let resolve = ai::Resolving(|| ai::endpoint(&setup, &store, ai::LocalModel::Missing));
        let (queue, id) = queued("  Café 日本語 [4K] 🔥  ");
        let original = queue.lock().unwrap().items()[0].name.clone();
        let row = run_naming_job(&queue, id, &resolve).expect("a failed ask still updates its tag");
        assert_eq!(row.name, original);
        assert_eq!(row.name_source, Some(NameSource::Fallback));
        assert!(
            !service
                .finish()
                .unwrap()
                .to_ascii_lowercase()
                .contains("authorization:")
        );
    }
}

#[test]
fn bad_ai_answers_from_a_keyless_service_fall_back() {
    for content in [
        "not JSON".to_owned(),
        r#"{"filename":""}"#.into(),
        r#"{"filename":false}"#.into(),
        serde_json::json!({"filename": "a".repeat(151)}).to_string(),
        r#"{"filename":"Translated title"}"#.into(),
    ] {
        let body = serde_json::json!({"choices":[{"message":{"content":content}}]}).to_string();
        let service = Service::start(200, body, false);
        let setup = settings(&service.url);
        let store = Memory::broken();
        let resolve = ai::Resolving(|| ai::endpoint(&setup, &store, ai::LocalModel::Missing));
        let (queue, id) = queued("日本語 [4K] 🔥");
        let original = queue.lock().unwrap().items()[0].name.clone();
        let row = run_naming_job(&queue, id, &resolve).unwrap();
        service.finish().expect("asked without a key");
        assert_eq!(row.name, original);
        assert_eq!(row.name_source, Some(NameSource::Fallback));
    }
}

#[test]
fn stalled_keyless_service_times_out_and_falls_back() {
    let service = Service::start(200, chat("unused"), true);
    let setup = settings(&service.url);
    let store = Memory::broken();
    let resolve = ai::Resolving(|| ai::endpoint(&setup, &store, ai::LocalModel::Missing));
    let (queue, id) = queued("Original [4K]");
    let original = queue.lock().unwrap().items()[0].name.clone();
    let start = Instant::now();
    let row = run_naming_job(&queue, id, &resolve).unwrap();
    let elapsed = start.elapsed();
    service.finish().expect("asked");
    assert!(elapsed >= Duration::from_secs(9), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(14), "{elapsed:?}");
    assert_eq!(row.name, original);
    assert_eq!(row.name_source, Some(NameSource::Fallback));
}

#[test]
fn connection_errors_keep_the_unreadable_store_note_and_do_not_echo_response_bodies() {
    for (status, body) in [
        (401, "sensitive-body-marker".to_owned()),
        (200, "malformed JSON".into()),
    ] {
        let service = Service::start(status, body, false);
        let tested = ai::test_connection(&settings(&service.url), &Memory::broken());
        let request = service.finish().unwrap();
        assert!(!tested.ok);
        assert!(tested.message.contains(queue_lib::secrets::UNREACHABLE));
        assert!(tested.message.contains("No key was sent."));
        assert!(!tested.message.contains("sensitive-body-marker"));
        assert!(!tested.message.contains(&service_url_marker(&request)));
        assert!(!request.to_ascii_lowercase().contains("authorization:"));
    }
}

fn service_url_marker(request: &str) -> String {
    request
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("host:")
                .map(|s| s.trim().to_owned())
        })
        .unwrap()
}

#[test]
fn anthropic_never_sends_metadata_when_the_key_is_missing_or_unreadable() {
    for store in [Memory::default(), Memory::broken()] {
        let service = Service::start(200, "{}".into(), false);
        let setup = Settings {
            api_kind: ai::ApiKind::Anthropic,
            ..settings(&service.url)
        };
        let resolve = ai::Resolving(|| ai::endpoint(&setup, &store, ai::LocalModel::Missing));
        let (queue, id) = queued("private item metadata");
        assert!(run_naming_job(&queue, id, &resolve).is_none());
        assert!(!ai::test_connection(&setup, &store).ok);
        assert_eq!(
            queue.lock().unwrap().items()[0].name_source,
            Some(NameSource::Rules)
        );
        assert!(service.finish().is_none(), "not even a TCP connection");
    }
}

struct MustNotRead;
impl Secrets for MustNotRead {
    fn get(&self) -> Result<Option<String>, String> {
        panic!("Local and Off must not read the API store")
    }
    fn set(&self, _: &str) -> Result<(), String> {
        unreachable!()
    }
    fn remove(&self) -> Result<(), String> {
        unreachable!()
    }
}

#[test]
fn store_status_is_api_only_and_resets_after_recovery() {
    for provider in [Provider::Local, Provider::Off] {
        let setup = Settings {
            naming_provider: provider,
            ..settings("https://example.test/v1")
        };
        assert!(!ai::store_unreachable(&setup, &MustNotRead));
        assert!(ai::endpoint(&setup, &MustNotRead, ai::LocalModel::Missing).is_err());
    }
    let setup = settings("https://example.test/v1");
    assert!(ai::store_unreachable(&setup, &Memory::broken()));
    assert!(!ai::store_unreachable(&setup, &Memory::default()));
    assert!(!ai::store_unreachable(&setup, &Memory::with("fake-key")));
}

struct RecoveringStore(std::sync::atomic::AtomicUsize);
impl Secrets for RecoveringStore {
    fn get(&self) -> Result<Option<String>, String> {
        if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            Err(queue_lib::secrets::UNREACHABLE.into())
        } else {
            Ok(Some("fake-key-after-recovery".into()))
        }
    }
    fn set(&self, _: &str) -> Result<(), String> {
        unreachable!()
    }
    fn remove(&self) -> Result<(), String> {
        unreachable!()
    }
}

#[test]
fn test_connection_reports_the_store_failure_that_actually_caused_its_keyless_request() {
    let service = Service::start(200, chat("Me at the zoo"), false);
    let store = RecoveringStore(std::sync::atomic::AtomicUsize::new(0));
    let tested = ai::test_connection(&settings(&service.url), &store);
    let request = service.finish().unwrap();
    assert!(tested.ok, "{tested:?}");
    assert!(!request.to_ascii_lowercase().contains("authorization:"));
    assert!(
        tested.message.contains(queue_lib::secrets::UNREACHABLE),
        "the request went without a key because the first store read failed: {tested:?}"
    );
    assert!(tested.message.contains("No key was sent."));
}

#[test]
fn colliding_names_from_a_keyless_service_never_overwrite_existing_media() {
    struct Temp(std::path::PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let temp = Temp(std::env::temp_dir().join(
        format!("queue-93-collision-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()),
    ));
    std::fs::create_dir(&temp.0).unwrap();
    std::fs::write(temp.0.join("Café 日本語.webm"), b"original").unwrap();
    for (number, bytes) in [(2, b"second".as_slice()), (3, b"third".as_slice())] {
        let service = Service::start(200, chat("Café 日本語"), false);
        let setup = settings(&service.url);
        let store = Memory::broken();
        let resolve = ai::Resolving(|| ai::endpoint(&setup, &store, ai::LocalModel::Missing));
        let (queue, id) = queued("Café 日本語 [4K]");
        let row = run_naming_job(&queue, id, &resolve).unwrap();
        service.finish().unwrap();
        let source = temp.0.join("incoming.webm");
        std::fs::write(&source, bytes).unwrap();
        let name = format!("{}.webm", row.name.unwrap());
        let saved =
            queue_lib::download::place_unique(&source, &temp.0, std::ffi::OsStr::new(&name))
                .unwrap();
        assert_eq!(
            saved.file_name().unwrap(),
            std::ffi::OsStr::new(&format!("Café 日本語 ({number}).webm"))
        );
        assert_eq!(std::fs::read(saved).unwrap(), bytes);
    }
    assert_eq!(
        std::fs::read(temp.0.join("Café 日本語.webm")).unwrap(),
        b"original"
    );
    assert_eq!(
        std::fs::read(temp.0.join("Café 日本語 (2).webm")).unwrap(),
        b"second"
    );
}
