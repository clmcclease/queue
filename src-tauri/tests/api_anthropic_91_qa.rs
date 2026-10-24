//! Independent #91 / N8 QA: hostile Messages responses, transport limits, and real naming jobs.
//! Loopback fixtures and fake credentials only; production code is unchanged.
#[path = "support/accept.rs"]
mod accept;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use queue_lib::ai::{self, ApiKind, Endpoint, Provider};
use queue_lib::meta::Meta;
use queue_lib::queue::{NameSource, Queue};
use queue_lib::secrets::{Memory, Secrets};
use queue_lib::{download, naming, run_naming_job, settings::Settings};

const KEY: &str = "sk-ant-independent-91";

struct Service {
    base: String,
    stop: mpsc::Sender<()>,
    worker: thread::JoinHandle<Option<String>>,
}

impl Service {
    // A bounded accept loop and stop channel prevent fixture leaks if a regression sends nothing.
    // `stall_body` sends the headers but withholds the body until the client times out.
    fn start(status: u16, body: String, headers: &str, stall_body: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let headers = headers.to_owned();
        let (stop, stopped) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut stream = accept::accept_until(&listener, || stopped.try_recv().is_ok())?;
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut received = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let count = stream.read(&mut buffer).unwrap_or(0);
                if count == 0 {
                    break;
                }
                received.extend_from_slice(&buffer[..count]);
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
            let head = format!(
                "HTTP/1.1 {status} QA\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            if stall_body {
                let _ = stopped.recv_timeout(Duration::from_secs(15));
            } else {
                let _ = stream.write_all(body.as_bytes());
            }
            Some(String::from_utf8(received).unwrap())
        });
        Self { base, stop, worker }
    }

    fn finish(self) -> Option<String> {
        let _ = self.stop.send(());
        self.worker.join().unwrap()
    }

    fn endpoint(&self) -> Endpoint {
        Endpoint {
            kind: ApiKind::Anthropic,
            base_url: self.base.clone(),
            model: "qa-model".into(),
            key: Some(KEY.into()),
        }
    }
}

fn envelope(text: &str) -> String {
    serde_json::json!({"stop_reason":"end_turn", "content":[
        {"type":"thinking", "thinking":"private reasoning"},
        {"type":"text", "text":text}
    ]})
    .to_string()
}

fn answer(name: &str) -> String {
    envelope(&serde_json::json!({"filename":name}).to_string())
}

fn queued(title: &str) -> (Mutex<Queue>, u64) {
    let mut queue = Queue::default();
    let id = queue.add("https://example.test/qa91").unwrap().id;
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
fn n8_anthropic_names_survive_unicode_cleanup_reserved_names_and_collisions() {
    for (title, output, expected) in [
        (
            "  Café 日本語 [4K] 🔥  ",
            "Café 日本語/曲.MP4",
            "Café 日本語⧸曲",
        ),
        ("Original", "NUL.mp4", "NUL_"),
        ("Original", "COM¹ .mp3", "COM¹_"),
    ] {
        let service = Service::start(200, answer(output), "", false);
        let (queue, id) = queued(title);
        let row = run_naming_job(&queue, id, &service.endpoint()).unwrap();
        service.finish().unwrap();
        assert_eq!(row.name.as_deref(), Some(expected));
        assert_eq!(row.name_source, Some(NameSource::Ai));
        assert_eq!(queue.lock().unwrap().start(id).unwrap().name, row.name);

        // The actual AI-derived name still follows N2's atomic no-overwrite placement path.
        let scratch = Scratch::new();
        let filename = format!("{expected}.mp4");
        let original = scratch.0.join(&filename);
        std::fs::write(&original, b"existing").unwrap();
        let source = scratch.0.join("download.tmp");
        std::fs::write(&source, b"new").unwrap();
        let placed = download::place_unique(&source, &scratch.0, filename.as_ref()).unwrap();
        assert_eq!(
            placed.file_name().unwrap(),
            format!("{expected} (2).mp4").as_str()
        );
        assert_eq!(std::fs::read(&original).unwrap(), b"existing");
        assert_eq!(std::fs::read(&placed).unwrap(), b"new");
    }
}

#[test]
fn n8_anthropic_bad_output_keeps_the_tier_one_name_and_tags_the_row_rules() {
    for content in [
        "not JSON".to_owned(),
        "null".into(),
        r#"{"filename":null}"#.into(),
        r#"{"filename":"first","filename":"second"}"#.into(),
        serde_json::json!({"filename":"\u{202e}\0 ..."}).to_string(),
        serde_json::json!({"filename":"Untitled.mp4"}).to_string(),
        serde_json::json!({"filename":"日".repeat(151)}).to_string(),
        serde_json::json!({"filename":"Translated title"}).to_string(),
    ] {
        let service = Service::start(200, envelope(&content), "", false);
        let (queue, id) = queued("  日本語 [4K] 🔥 ");
        let before = queue.lock().unwrap().items()[0].name.clone();
        let row = run_naming_job(&queue, id, &service.endpoint()).unwrap();
        service.finish().unwrap();
        assert_eq!(row.name, before, "{content}");
        assert_eq!(row.name_source, Some(NameSource::Fallback), "{content}");
    }
}

#[test]
fn n8_anthropic_malformed_envelopes_are_redacted_errors() {
    for body in [
        "null",
        "[]",
        "{}",
        r#"{"content":[],"stop_reason":"end_turn"}"#,
        r#"{"content":[{"type":"text","text":42}],"stop_reason":"end_turn"}"#,
        r#"{"content":[{"type":"text","text":"sk-ant-independent-91"}]}"#,
        r#"{"content":[{"type":"tool_use","input":{"filename":"X"}}],"stop_reason":"end_turn"}"#,
        r#"{"content":[{"type":"text","text":"secret"}],"stop_reason":null}"#,
    ] {
        let service = Service::start(200, body.into(), "", false);
        let error = ai::ask(&service.endpoint(), &naming::sample()).unwrap_err();
        service.finish().unwrap();
        assert_eq!(
            error, "The naming service sent an unreadable answer.",
            "{body}"
        );
    }
}

#[test]
fn n8_anthropic_uses_the_first_text_block_and_does_not_search_for_a_valid_later_one() {
    for (first, second, expected) in [
        (
            r#"{"filename":"First"}"#,
            r#"{"filename":"Second"}"#,
            Some("First"),
        ),
        ("broken", r#"{"filename":"Second"}"#, None),
    ] {
        let body = serde_json::json!({"stop_reason":"end_turn", "content":[
            {"type":"thinking"}, {"type":"text","text":first}, {"type":"text","text":second}
        ]})
        .to_string();
        let service = Service::start(200, body, "", false);
        let actual = ai::ask(&service.endpoint(), &naming::sample());
        service.finish().unwrap();
        match expected {
            Some(name) => assert_eq!(actual, Ok(name.into())),
            None => assert!(actual.is_err()),
        }
    }
}

#[test]
fn n8_anthropic_redirects_never_forward_metadata_or_the_key() {
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    target.set_nonblocking(true).unwrap();
    let location = format!(
        "Location: http://{}/capture\r\n",
        target.local_addr().unwrap()
    );
    for status in [301, 302, 303, 307, 308] {
        let service = Service::start(status, answer("Redirected"), &location, false);
        let result = ai::ask(&service.endpoint(), &naming::sample());
        service.finish().unwrap();
        assert!(result.is_err(), "{status}: {result:?}");
        assert_eq!(
            target.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn n8_anthropic_body_limit_counts_the_whole_envelope() {
    // ureq's LimitReader errors once the configured byte budget is exhausted, so the
    // boundary itself is refused too. The requirement is a cap, not inclusive acceptance.
    for (size, accepted) in [(65_535, true), (65_536, false), (65_537, false)] {
        let mut body = answer("Small name");
        body.push_str(&" ".repeat(size - body.len()));
        let service = Service::start(200, body, "", false);
        let result = ai::ask(&service.endpoint(), &naming::sample());
        service.finish().unwrap();
        if accepted {
            assert_eq!(result, Ok("Small name".into()));
        } else {
            assert_eq!(
                result,
                Err("The naming service's answer was too long.".into())
            );
        }
    }
}

#[test]
fn n8_anthropic_budget_also_bounds_reading_a_stalled_response_body() {
    let service = Service::start(200, answer("Too late"), "", true);
    let started = Instant::now();
    let result = ai::ask_within(
        &service.endpoint(),
        &naming::sample(),
        Duration::from_millis(250),
    );
    let elapsed = started.elapsed();
    service.finish().unwrap();
    assert_eq!(
        result,
        Err("The naming service didn't answer in time.".into())
    );
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    assert_eq!(ai::TIMEOUT, Duration::from_secs(10));
}

#[test]
fn n8_anthropic_test_connection_obeys_the_real_default_budget() {
    let service = Service::start(200, answer("Too late"), "", true);
    let setup = Settings {
        api_kind: ApiKind::Anthropic,
        api_base_url: service.base.clone(),
        api_model: "qa-model".into(),
        ..Default::default()
    };
    let started = Instant::now();
    let tested = ai::test_connection(&setup, &Memory::with(KEY));
    let elapsed = started.elapsed();
    service.finish().unwrap();
    assert!(!tested.ok);
    assert_eq!(tested.message, "The naming service didn't answer in time.");
    assert!(
        (Duration::from_secs(8)..Duration::from_secs(15)).contains(&elapsed),
        "{elapsed:?}"
    );
}

#[test]
fn n8_anthropic_key_removal_and_broken_store_prevent_any_request() {
    let service = Service::start(200, answer("Must not be asked"), "", false);
    let setup = Settings {
        naming_provider: Provider::Api,
        api_kind: ApiKind::Anthropic,
        api_base_url: service.base.clone(),
        api_model: "qa-model".into(),
        ..Default::default()
    };
    let store = Memory::with(KEY);
    assert!(ai::api_endpoint(&setup, &store).is_ok());
    store.remove().unwrap();
    for secrets in [&store, &Memory::broken()] {
        let resolve = ai::Resolving(|| ai::api_endpoint(&setup, secrets));
        let (queue, id) = queued("Messy [4K] 🔥");
        assert!(run_naming_job(&queue, id, &resolve).is_none());
        let row = queue.lock().unwrap().items()[0].clone();
        assert_eq!(row.name.as_deref(), Some("Messy"));
        assert_eq!(row.name_source, Some(NameSource::Rules));
        assert!(!ai::test_connection(&setup, secrets).ok);
    }
    let mut direct = service.endpoint();
    direct.key = None;
    assert_eq!(
        ai::ask(&direct, &naming::sample()),
        Err("Anthropic needs an API key.".into())
    );
    assert!(service.finish().is_none());
}

struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "queue-qa-91-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn n8_anthropic_settings_round_trip_without_serializing_the_key() {
    let scratch = Scratch::new();
    let path = scratch.0.join("settings.json");
    let setup = Settings {
        naming_provider: Provider::Api,
        api_kind: ApiKind::Anthropic,
        api_base_url: "https://api.anthropic.com".into(),
        api_model: "qa-model".into(),
        ..Default::default()
    };
    let store = Memory::with(KEY);
    setup.save(&path).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains(r#""apiKind": "anthropic""#));
    assert!(!saved.contains(KEY));
    let restored = Settings::load(&path);
    assert_eq!(restored, setup);
    assert_eq!(
        ai::api_endpoint(&restored, &store).unwrap().kind,
        ApiKind::Anthropic
    );
    let old: Settings =
        serde_json::from_str(r#"{"apiBaseUrl":"https://old.example/v1","apiModel":"old"}"#)
            .unwrap();
    assert_eq!(old.api_kind, ApiKind::OpenaiCompatible);
    assert_eq!(old.api_model, "old");
}
