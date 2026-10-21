//! #33 / N9: `ai::ask` against a fake OpenAI-compatible server on 127.0.0.1. No network, no
//! model, no key.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};

use queue_lib::ai::{self, Endpoint};
use queue_lib::meta::Meta;

/// What the fake server sends back for one request, and what it received.
struct Fake {
    url: String,
    request: thread::JoinHandle<String>,
}

fn serve(status: u16, body: String, delay: Duration) -> Fake {
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
        // Headers, then as many body bytes as Content-Length says.
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
        thread::sleep(delay);
        let reply = format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(reply.as_bytes());
        String::from_utf8_lossy(&received).into_owned()
    });
    Fake { url, request }
}

fn chat(content: &str) -> String {
    serde_json::json!({"choices": [{"message": {"role": "assistant", "content": content}}]})
        .to_string()
}

fn endpoint(url: &str, key: Option<&str>) -> Endpoint {
    Endpoint {
        base_url: url.into(),
        model: "test-model".into(),
        key: key.map(Into::into),
    }
}

fn meta() -> Meta {
    Meta {
        title: Some("Daft Punk - Get Lucky (Official Audio)".into()),
        channel: Some("Daft Punk".into()),
        ..Default::default()
    }
}

#[test]
fn n9_a_good_answer_comes_back_sanitized_and_the_request_is_well_formed() {
    let fake = serve(
        200,
        chat(r#"{"filename": "Daft Punk – Get Lucky"}"#),
        Duration::ZERO,
    );
    let name = ai::ask(&endpoint(&fake.url, Some("sk-test")), &meta()).unwrap();
    assert_eq!(name, "Daft Punk – Get Lucky");
    let request = fake.request.join().unwrap();
    assert!(
        request.starts_with("POST /v1/chat/completions "),
        "{request}"
    );
    assert!(request.contains("Bearer sk-test"));
    let body: serde_json::Value =
        serde_json::from_str(&request[request.find("\r\n\r\n").unwrap() + 4..]).unwrap();
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["temperature"], 0);
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(body["messages"][0]["role"], "system");
    let data: serde_json::Value =
        serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(data["title"], "Daft Punk - Get Lucky (Official Audio)");
}

/// The runner's raw path (`ask_content`) is the same chokepoint as `ask`: it refuses a non-local
/// `http://` endpoint before connecting.
#[test]
fn n9_the_raw_answer_path_has_the_same_rules() {
    for refused in [
        "http://example.com/v1",
        "http://192.168.1.5:8080/v1",
        "ftp://x/v1",
    ] {
        let error =
            ai::ask_content(&endpoint(refused, Some("sk-x")), &meta(), ai::TIMEOUT).unwrap_err();
        assert!(
            error.contains("https://") && !error.contains("sk-x"),
            "{error}"
        );
    }
}

#[test]
fn n9_no_key_sends_no_authorization() {
    let fake = serve(200, chat(r#"{"filename": "x"}"#), Duration::ZERO);
    ai::ask(&endpoint(&fake.url, None), &meta()).unwrap();
    assert!(
        !fake
            .request
            .join()
            .unwrap()
            .to_ascii_lowercase()
            .contains("authorization")
    );
}

#[test]
fn n9_a_slow_service_times_out_within_the_budget() {
    let fake = serve(200, chat(r#"{"filename": "late"}"#), Duration::from_secs(3));
    let started = Instant::now();
    let error = ai::ask_within(
        &endpoint(&fake.url, None),
        &meta(),
        Duration::from_millis(300),
    )
    .unwrap_err();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(error, "The naming service didn't answer in time.");
    let _ = fake.request.join();
}

#[test]
fn n9_bad_answers_are_errors_never_names() {
    for (status, body, expected) in [
        (200, "not json".to_owned(), "unreadable answer"),
        (200, r#"{"choices": []}"#.to_owned(), "unreadable answer"),
        (
            200,
            chat("Daft Punk – Get Lucky"),
            "wasn't usable (NotJson)",
        ),
        (
            200,
            chat(r#"{"filename": "Untitled"}"#),
            "wasn't usable (Fallback)",
        ),
        (
            200,
            chat(&format!(r#"{{"filename": "{}"}}"#, "a".repeat(151))),
            "wasn't usable (TooLong)",
        ),
        (500, "{}".to_owned(), "HTTP 500"),
        (200, chat(&"x".repeat(70_000)), "too long"),
    ] {
        let fake = serve(status, body, Duration::ZERO);
        let error = ai::ask(&endpoint(&fake.url, None), &meta()).unwrap_err();
        assert!(
            error.contains(expected),
            "{error:?} should mention {expected:?}"
        );
        let _ = fake.request.join();
    }
}

#[test]
fn n9_a_path_in_the_answer_is_made_harmless() {
    let fake = serve(
        200,
        chat(r#"{"filename": "../../etc/passwd.mp4"}"#),
        Duration::ZERO,
    );
    let name = ai::ask(&endpoint(&fake.url, None), &meta()).unwrap();
    assert!(
        !name.contains('/') && !name.contains('\\') && !name.starts_with('.'),
        "{name}"
    );
    let _ = fake.request.join();
}

#[test]
fn n9_errors_never_contain_the_key_the_url_or_the_metadata() {
    // A 401 that echoes the key and the request back.
    let fake = serve(
        401,
        r#"{"error": "bad key sk-secret-123 for Daft Punk"}"#.into(),
        Duration::ZERO,
    );
    let error = ai::ask(
        &endpoint(&format!("{}?token=t0k3n", fake.url), Some("sk-secret-123")),
        &meta(),
    )
    .unwrap_err();
    let _ = fake.request.join();
    // Nothing listening, with credentials in the URL itself.
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let unreachable = format!("http://user:pa55word@127.0.0.1:{port}/v1?token=t0k3n");
    let error2 = ai::ask(&endpoint(&unreachable, Some("sk-secret-123")), &meta()).unwrap_err();
    // A refused (non-local http) URL.
    let error3 = ai::ask(
        &endpoint(
            "http://user:pa55word@example.com/v1?token=t0k3n",
            Some("sk-secret-123"),
        ),
        &meta(),
    )
    .unwrap_err();
    for error in [&error, &error2, &error3] {
        for secret in [
            "sk-secret-123",
            "pa55word",
            "t0k3n",
            "Daft Punk",
            "127.0.0.1",
            "example.com",
        ] {
            assert!(!error.contains(secret), "{error:?} leaks {secret:?}");
        }
    }
    assert_eq!(error, "The naming service answered with HTTP 401.");
    assert_eq!(error2, "Couldn't reach the naming service.");
    assert!(error3.contains("https://"));
}

/// #33 review (P1): with HTTP_PROXY, HTTPS_PROXY and ALL_PROXY set (and no NO_PROXY), a request
/// to the local model still goes straight to it: the proxy never sees the metadata or the key.
/// The proxy variables are set only in a child test process.
#[test]
fn n9_a_local_request_never_goes_through_a_proxy() {
    if std::env::var_os("QUEUE_TEST_PROXY_CHILD").is_some() {
        let fake = serve(200, chat(r#"{"filename": "Local"}"#), Duration::ZERO);
        let name = ai::ask(&endpoint(&fake.url, Some("sk-local")), &meta()).unwrap();
        assert_eq!(name, "Local");
        assert!(fake.request.join().unwrap().contains("Bearer sk-local"));
        return;
    }
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    proxy.set_nonblocking(true).unwrap();
    let proxy_url = format!("http://127.0.0.1:{}", proxy.local_addr().unwrap().port());
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "--exact",
            "n9_a_local_request_never_goes_through_a_proxy",
            "--nocapture",
        ])
        .env("QUEUE_TEST_PROXY_CHILD", "1")
        .env_remove("NO_PROXY")
        .env_remove("no_proxy");
    for var in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        child.env(var, &proxy_url);
    }
    let out = child.output().unwrap();
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(proxy.accept().is_err(), "the proxy was contacted");
}
