//! #32 / N8: the OpenAI-compatible API provider against a fake server on 127.0.0.1, with the key
//! in an in-memory store. No network, and the real credential store is never touched.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use queue_lib::ai::{self, Provider};
use queue_lib::meta::Meta;
use queue_lib::queue::{NameSource, Queue};
use queue_lib::run_naming_job;
use queue_lib::secrets::{Memory, UNREACHABLE};
use queue_lib::settings::Settings;

const KEY: &str = "sk-secret-32-do-not-echo";

/// Answers one request with `status` and `body`, and hands back what it received.
fn serve(status: u16, body: String) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://127.0.0.1:{}/v1",
        listener.local_addr().unwrap().port()
    );
    let request = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
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

fn connected(listener: &TcpListener) -> bool {
    listener.accept().is_ok()
}

fn chat(content: &str) -> String {
    serde_json::json!({"choices": [{"message": {"role": "assistant", "content": content}}]})
        .to_string()
}

fn api(url: &str) -> Settings {
    Settings {
        naming_provider: Provider::Api,
        api_base_url: url.into(),
        api_model: "small-model".into(),
        ..Default::default()
    }
}

fn queued(q: &mut Queue, title: &str) -> u64 {
    let id = q
        .add(&format!("https://e.test/{}", title.len()))
        .unwrap()
        .id;
    let meta = Meta {
        title: Some(title.into()),
        ..Default::default()
    };
    q.fetched(id, Ok(meta)).unwrap();
    id
}

#[test]
fn n8_test_connection_names_the_sample_with_the_stored_key() {
    let (url, request) = serve(200, chat(r#"{"filename": "Me at the zoo"}"#));
    let tested = ai::test_connection(&api(&url), &Memory::with(KEY));
    let received = request.join().unwrap();
    assert!(tested.ok, "{tested:?}");
    assert_eq!(
        tested.message,
        "Connected. It would name the sample \u{201c}Me at the zoo\u{201d}."
    );
    assert!(
        received.starts_with("POST /v1/chat/completions "),
        "{received}"
    );
    assert!(
        received.to_ascii_lowercase().contains(&format!(
            "authorization: bearer {}",
            KEY.to_ascii_lowercase()
        )),
        "the key comes from the store"
    );
    assert!(
        received.contains("Me at the zoo [HD]"),
        "the preview sample"
    );
    assert!(received.contains("small-model"), "the saved model");
}

#[test]
fn n8_a_refused_key_says_so_without_echoing_it() {
    let (url, request) = serve(401, format!(r#"{{"error": "bad key {KEY}"}}"#));
    let tested = ai::test_connection(&api(&url), &Memory::with(KEY));
    request.join().unwrap();
    assert!(!tested.ok);
    assert_eq!(
        tested.message,
        "The naming service answered with HTTP 401. The service refused the key."
    );
    assert!(!tested.message.contains(KEY));
    assert!(!tested.message.contains("127.0.0.1"), "no URL");

    let (url, request) = serve(404, "{}".into());
    let tested = ai::test_connection(&api(&url), &Memory::default());
    request.join().unwrap();
    assert!(tested.message.ends_with("Check the base URL and model."));
}

#[test]
fn n8_test_connection_reports_an_unreachable_service_and_an_unset_or_broken_setup() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let closed = format!("http://127.0.0.1:{port}/v1");
    let tested = ai::test_connection(&api(&closed), &Memory::with(KEY));
    assert_eq!(
        (tested.ok, tested.message.as_str()),
        (false, "Couldn't reach the naming service.")
    );
    let tested = ai::test_connection(&Settings::default(), &Memory::with(KEY));
    assert_eq!(tested.message, "Save a base URL and a model first.");
    let (url, listener) = recorder();
    let tested = ai::test_connection(&api(&url), &Memory::broken());
    assert_eq!(tested.message, UNREACHABLE);
    assert!(
        !connected(&listener),
        "nothing is sent without the key store"
    );
}

/// #25's cross-wiring check: with Local chosen, a stored API service is never contacted, and
/// its key is never read.
#[test]
fn n8_local_never_contacts_the_stored_api_service() {
    let (url, listener) = recorder();
    let settings = Settings {
        naming_provider: Provider::Local,
        ..api(&url)
    };
    let store = Memory::broken(); // reading the key would fail loudly
    let mut q = Queue::default();
    let id = queued(&mut q, "Title");
    let q = Mutex::new(q);
    let resolve = ai::Resolving(|| ai::endpoint(&settings, &store));
    assert!(run_naming_job(&q, id, &resolve).is_none(), "nothing asked");
    assert_eq!(
        ai::endpoint(&settings, &store).err(),
        Some(ai::Unavailable::ModelMissing)
    );
    assert!(!connected(&listener), "no connection to the API service");
    assert!(!ai::askable(&settings));
    assert_eq!(
        q.lock().unwrap().items()[0].name_source,
        Some(NameSource::Rules),
        "no ask ran, so no rules tag"
    );
}

/// #25's review, fixed in #32: the endpoint is resolved after the row's generation is taken.
/// A provider switched to Off while the job runs (save, then a new generation, as the command
/// does) is seen by the resolve, so nothing is sent and the name stays.
#[test]
fn n8_a_provider_changed_during_the_job_is_never_sent_to() {
    let (url, listener) = recorder();
    let provider = Mutex::new(Provider::Api);
    let mut q = Queue::default();
    let id = queued(&mut q, "raw title [4K]");
    let q = Mutex::new(q);
    let store = Memory::with(KEY);
    let resolve = ai::Resolving(|| {
        // The user switches to Off just as the job starts.
        *provider.lock().unwrap() = Provider::Off;
        q.lock().unwrap().renaming();
        let settings = Settings {
            naming_provider: *provider.lock().unwrap(),
            ..api(&url)
        };
        ai::endpoint(&settings, &store)
    });
    assert!(
        run_naming_job(&q, id, &resolve).is_none(),
        "the answer is dropped"
    );
    assert!(!connected(&listener), "nothing reaches the old provider");
    let q = q.lock().unwrap();
    let row = &q.items()[0];
    assert_eq!(
        (row.name.as_deref(), row.name_source),
        (Some("raw title"), Some(NameSource::Rules))
    );
}

#[test]
fn n8_the_key_is_never_written_to_the_settings_file() {
    let dir = std::env::temp_dir().join(format!("queue-n8-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    let store = Memory::default();
    use queue_lib::secrets::Secrets;
    store.set(KEY).unwrap();
    api("https://api.example.test/v1").save(&path).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(!saved.contains(KEY), "{saved}");
    assert!(saved.contains("small-model"));
    std::fs::remove_dir_all(dir).unwrap();
}

/// Counts asks per title, and answers with a clean name.
struct Counting(Mutex<Vec<String>>);

impl ai::Namer for Counting {
    fn name(&self, meta: &Meta) -> Result<String, String> {
        let title = meta.title.clone().unwrap_or_default();
        self.0.lock().unwrap().push(title.clone());
        Ok(format!("{title} clean"))
    }
}

/// #32 review: an API change while B waits re-queues A and B under a new generation. The job B
/// was queued with before the change is dropped, so B is asked once, under the new setup.
#[test]
fn n8_a_job_queued_before_an_api_change_is_dropped_not_asked_twice() {
    let mut q = Queue::default();
    let (a, b) = (queued(&mut q, "Alpha"), queued(&mut q, "Bravo!"));
    let before = q.naming_job(b).unwrap().1;
    q.renaming(); // the model or key changed: both are asked again
    let after = q.naming_job(b).unwrap().1;
    let q = Mutex::new(q);
    let namer = Counting(Mutex::new(Vec::new()));
    // The worker's channel order: B's old job, then the new jobs for A and B.
    assert!(queue_lib::run_naming_job_at(&q, b, Some(before), &namer).is_none());
    let named_a = queue_lib::run_naming_job_at(&q, a, Some(after), &namer).unwrap();
    let named_b = queue_lib::run_naming_job_at(&q, b, Some(after), &namer).unwrap();
    assert_eq!(
        *namer.0.lock().unwrap(),
        ["Alpha", "Bravo!"],
        "one ask each"
    );
    assert_eq!(named_a.name.as_deref(), Some("Alpha clean"));
    assert_eq!(
        (named_b.name.as_deref(), named_b.name_source),
        (Some("Bravo! clean"), Some(NameSource::Ai))
    );
}

/// #32 review round 2: a fetch finishing as the API setup changes queues the same row twice for
/// one generation; the worker asks only the first.
#[test]
fn n8_a_row_is_asked_once_per_generation_even_if_queued_twice() {
    let mut claims = queue_lib::NamingClaims::default();
    assert!(claims.claim(7, 1));
    assert!(claims.claim(8, 1));
    assert!(!claims.claim(7, 1), "the duplicate is skipped");
    assert!(claims.claim(7, 2), "a newer generation asks again");
    assert!(!claims.claim(7, 2));
    assert!(claims.claim(8, 2));
}
