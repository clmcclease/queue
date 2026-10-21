//! Independent #32(a) / N8 QA: saved endpoint resolution, optional keys, Test connection,
//! and provider switches before the request starts. Only fake credentials and loopback HTTP.
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

const KEY: &str = "sk-qa-32-private";

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
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if stopped.try_recv().is_ok() || Instant::now() >= deadline {
                            return None;
                        }
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(e) => panic!("fixture accept: {e}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
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
fn n8_saved_keyless_setup_names_unicode_and_reserved_output_through_the_real_job() {
    for (answer, expected) in [
        ("Café 日本語/曲.MP4", "Café 日本語⧸曲"),
        ("NUL.mp4", "NUL_"),
    ] {
        let service = Service::start(200, chat(answer), false);
        let setup = settings(&service.url);
        let store = Memory::default();
        let resolve = ai::Resolving(|| ai::endpoint(&setup, &store, ai::LocalModel::Missing));
        let title = if answer.starts_with("NUL") {
            "Original"
        } else {
            "  Café 日本語 [4K] 🔥  "
        };
        let (queue, id) = queued(title);
        let row = run_naming_job(&queue, id, &resolve).unwrap();
        let request = service.finish().unwrap();
        assert_eq!(row.name.as_deref(), Some(expected));
        assert_eq!(row.name_source, Some(NameSource::Ai));
        assert_eq!(queue.lock().unwrap().start(id).unwrap().name, row.name);
        assert!(!request.to_ascii_lowercase().contains("authorization:"));
        assert!(request.starts_with("POST /v1/chat/completions "));
        assert!(request.contains("qa-model"));
    }
}

#[test]
fn n8_resolving_each_job_uses_the_replaced_key_then_no_key_after_removal() {
    let store = Memory::with("old-key");
    for key in [Some(KEY), None] {
        match key {
            Some(key) => store.set(key).unwrap(),
            None => store.remove().unwrap(),
        }
        let service = Service::start(200, chat("Café 日本語"), false);
        let setup = settings(&service.url);
        let resolve = ai::Resolving(|| ai::endpoint(&setup, &store, ai::LocalModel::Missing));
        let (queue, id) = queued("  Café 日本語 [4K] 🔥  ");
        assert!(run_naming_job(&queue, id, &resolve).is_some());
        let request = service.finish().unwrap().to_ascii_lowercase();
        assert!(!request.contains("old-key"));
        assert_eq!(request.contains("authorization:"), key.is_some());
        if let Some(key) = key {
            assert!(request.contains(&format!("bearer {key}")));
        }
    }
}

#[test]
fn n8_test_connection_rejects_bad_output_and_redacts_error_bodies() {
    for (status, body, expected) in [
        (
            403,
            format!("refused {KEY}"),
            "The service refused the key.",
        ),
        (429, format!("quota {KEY}"), "HTTP 429."),
        (503, format!("down {KEY}"), "HTTP 503."),
        (200, format!("not JSON {KEY}"), "unreadable answer."),
        (200, chat("   "), "wasn't usable (Empty)."),
        (200, chat(&"日".repeat(151)), "wasn't usable (TooLong)."),
    ] {
        let service = Service::start(status, body, false);
        let tested = ai::test_connection(&settings(&service.url), &Memory::with(KEY));
        service.finish().unwrap();
        assert!(!tested.ok);
        assert!(tested.message.ends_with(expected), "{tested:?}");
        assert!(!tested.message.contains(KEY));
        assert!(!tested.message.contains("127.0.0.1"));
    }
}

#[test]
fn n8_test_connection_obeys_the_default_ten_second_budget() {
    let service = Service::start(200, String::new(), true);
    let started = Instant::now();
    let tested = ai::test_connection(&settings(&service.url), &Memory::default());
    let elapsed = started.elapsed();
    service.finish().unwrap();
    assert!(!tested.ok);
    assert_eq!(tested.message, "The naming service didn't answer in time.");
    assert!(
        (Duration::from_secs(8)..Duration::from_secs(15)).contains(&elapsed),
        "{elapsed:?}"
    );
}

/// A deterministic slow-key-store interleaving: the UI saves Off and invalidates the naming
/// generation while endpoint() is retrieving the credential, before HTTP starts.
struct SwitchOff<'a> {
    settings: &'a Mutex<Settings>,
    queue: &'a Mutex<Queue>,
}
impl Secrets for SwitchOff<'_> {
    fn get(&self) -> Result<Option<String>, String> {
        self.settings.lock().unwrap().naming_provider = Provider::Off;
        self.queue.lock().unwrap().renaming();
        Ok(Some(KEY.into()))
    }
    fn set(&self, _: &str) -> Result<(), String> {
        panic!("unexpected credential write")
    }
    fn remove(&self) -> Result<(), String> {
        panic!("unexpected credential removal")
    }
}

#[test]
fn n8_switching_off_during_key_resolution_must_prevent_the_old_request() {
    let service = Service::start(200, chat("Café 日本語"), false);
    let current = Mutex::new(settings(&service.url));
    let (queue, id) = queued("  Café 日本語 [4K] 🔥  ");
    let store = SwitchOff {
        settings: &current,
        queue: &queue,
    };
    let resolve = ai::Resolving(|| {
        let snapshot = current.lock().unwrap().clone(); // same ordering as naming_worker
        ai::endpoint(&snapshot, &store, ai::LocalModel::Missing)
    });
    let result = run_naming_job(&queue, id, &resolve);
    let received = service.finish();
    assert_eq!(current.lock().unwrap().naming_provider, Provider::Off);
    assert!(
        result.is_none(),
        "an invalidated answer must not be applied"
    );
    assert!(
        received.is_none(),
        "the old API received metadata and a key after Off was saved"
    );
}

#[test]
fn n8_a_job_already_queued_when_off_is_saved_must_keep_an_unasked_rules_name() {
    let (queue, id) = queued("  Café 日本語 [4K] 🔥  ");
    queue.lock().unwrap().renaming(); // settings_set_naming_provider invalidates existing jobs
    let setup = Settings {
        naming_provider: Provider::Off,
        ..Default::default()
    };
    let store = Memory::broken();
    let resolve = ai::Resolving(|| ai::endpoint(&setup, &store, ai::LocalModel::Missing));
    let _ = run_naming_job(&queue, id, &resolve); // the worker still receives the queued id
    let queue = queue.lock().unwrap();
    assert_eq!(
        queue.items()[0].name_source,
        Some(NameSource::Rules),
        "no ask ran; D-029 reserves Fallback/the rules badge for an actual failed ask"
    );
}
