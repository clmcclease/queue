//! Independent #100 part 2 / N8 QA. Only loopback HTTP and fake credential reads.
#[path = "support/accept.rs"]
mod accept;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Mutex, mpsc};
use std::thread;
use std::time::Duration;

use queue_lib::ai::{self, ApiKind, Provider};
use queue_lib::meta::Meta;
use queue_lib::queue::{NameSource, Queue, Status};
use queue_lib::run_naming_job;
use queue_lib::secrets::{Memory, Secrets};
use queue_lib::settings::Settings;

const KEY: &str = "qa-100-fake-key";

fn setup(kind: ApiKind, url: &str) -> Settings {
    Settings {
        naming_provider: Provider::Api,
        api_kind: kind,
        api_base_url: url.into(),
        api_model: "qa-model".into(),
        ..Default::default()
    }
}

fn queued(queue: &mut Queue, url: &str, title: &str) -> u64 {
    let id = queue.add(url).unwrap().id;
    queue
        .fetched(
            id,
            Ok(Meta {
                title: Some(title.into()),
                ..Default::default()
            }),
        )
        .unwrap();
    id
}

fn answer(name: &str) -> String {
    let text = serde_json::json!({"filename": name}).to_string();
    serde_json::json!({"stop_reason":"end_turn", "content":[{"type":"text", "text":text}]})
        .to_string()
}

/// Sequential responses on the same address: a failure cannot be hidden by changing settings.
fn service(responses: Vec<(u16, String)>) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let worker = thread::spawn(move || {
        responses.into_iter().map(|(status, body)| {
            let mut stream = accept::accept(&listener);
            stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            stream.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 4096];
            loop {
                let count = stream.read(&mut buf).unwrap();
                assert_ne!(count, 0, "incomplete fixture request");
                request.extend_from_slice(&buf[..count]);
                assert!(request.len() < 64 * 1024);
                let text = String::from_utf8_lossy(&request);
                if let Some(end) = text.find("\r\n\r\n") {
                    let length: usize = text[..end].lines().find_map(|line| {
                        line.to_ascii_lowercase().strip_prefix("content-length:")
                            .map(|value| value.trim().parse().unwrap())
                    }).unwrap();
                    if request.len() >= end + 4 + length { break; }
                }
            }
            write!(stream, "HTTP/1.1 {status} QA\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            String::from_utf8(request).unwrap()
        }).collect()
    });
    (url, worker)
}

/// Holds the actual Secrets::get call until the test has changed the kind and generation.
struct PausedStore {
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl Secrets for PausedStore {
    fn get(&self) -> Result<Option<String>, String> {
        self.entered.send(()).unwrap();
        self.release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        Ok(Some(KEY.into()))
    }
    fn set(&self, _: &str) -> Result<(), String> {
        panic!("unexpected credential write")
    }
    fn remove(&self) -> Result<(), String> {
        panic!("unexpected credential deletion")
    }
}

#[test]
fn a_kind_switch_inside_a_blocked_credential_read_drops_the_stale_job_in_both_directions() {
    for (old_kind, new_kind) in [
        (ApiKind::OpenaiCompatible, ApiKind::Anthropic),
        (ApiKind::Anthropic, ApiKind::OpenaiCompatible),
    ] {
        let old = TcpListener::bind("127.0.0.1:0").unwrap();
        old.set_nonblocking(true).unwrap();
        let old_url = format!("http://{}", old.local_addr().unwrap());
        let new = TcpListener::bind("127.0.0.1:0").unwrap();
        new.set_nonblocking(true).unwrap();
        let new_url = format!("http://{}", new.local_addr().unwrap());
        let settings = Mutex::new(setup(old_kind, &old_url));
        let mut queue = Queue::default();
        let id = queued(
            &mut queue,
            "https://e.test/blocked",
            "  Café 日本語 [4K] 🔥  ",
        );
        let queue = Mutex::new(queue);
        let before = queue.lock().unwrap().items()[0].clone();
        let (entered, waiting) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let store = PausedStore {
            entered,
            release: Mutex::new(released),
        };
        thread::scope(|scope| {
            let job = scope.spawn(|| {
                let resolve = ai::Resolving(|| {
                    let snapshot = settings.lock().unwrap().clone();
                    ai::endpoint(&snapshot, &store, ai::LocalModel::Missing)
                });
                run_naming_job(&queue, id, &resolve)
            });
            waiting.recv_timeout(Duration::from_secs(3)).unwrap();
            *settings.lock().unwrap() = setup(new_kind, &new_url);
            queue.lock().unwrap().renaming();
            release.send(()).unwrap();
            assert!(job.join().unwrap().is_none());
        });
        for listener in [&old, &new] {
            assert!(
                matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
            );
        }
        let row = queue.lock().unwrap().items()[0].clone();
        assert_eq!(row.name, before.name);
        assert_eq!(row.name_source, Some(NameSource::Rules));
        assert_eq!(row.status, Status::Queued);
        assert_eq!(
            row.rev, before.rev,
            "a stale job must not emit a row change"
        );
    }
}

#[test]
fn an_anthropic_failure_then_success_on_the_same_endpoint_preserves_each_rows_result() {
    for (status, bad) in [
        (401, r#"{"type":"error"}"#.into()),
        (529, r#"{"type":"error"}"#.into()),
        (200, "not an API envelope".into()),
        (200, answer("\u{202e}\0 ...")),
        (200, answer(&"日".repeat(151))),
    ] {
        let (url, worker) = service(vec![(status, bad), (200, answer("Café 日本語/曲.MP4"))]);
        let settings = setup(ApiKind::Anthropic, &url);
        let store = Memory::with(KEY);
        let resolve = ai::Resolving(|| ai::endpoint(&settings, &store, ai::LocalModel::Missing));
        let mut queue = Queue::default();
        let first = queued(&mut queue, "https://e.test/fail", "  日本語 [4K] 🔥  ");
        let second = queued(&mut queue, "https://e.test/ok", "Café 日本語 曲");
        let fallback = queue.items()[0].name.clone();
        let queue = Mutex::new(queue);
        let failed = run_naming_job(&queue, first, &resolve).unwrap();
        assert_eq!(failed.name, fallback);
        assert_eq!(failed.name_source, Some(NameSource::Fallback));
        let good = run_naming_job(&queue, second, &resolve).unwrap();
        assert_eq!(good.name.as_deref(), Some("Café 日本語⧸曲"));
        assert_eq!(good.name_source, Some(NameSource::Ai));
        let requests = worker.join().unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests {
            assert!(request.starts_with("POST /v1/messages "));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains(&format!("x-api-key: {KEY}"))
            );
        }
        let queue = queue.lock().unwrap();
        assert_eq!(queue.items()[0].name, failed.name);
        assert_eq!(queue.items()[0].name_source, Some(NameSource::Fallback));
        assert!(queue.items().iter().all(|row| row.status == Status::Queued));
    }
}
