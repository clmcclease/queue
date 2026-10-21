//! #91 / N8: the Anthropic Messages API provider against a fake `/v1/messages` on 127.0.0.1.
//! No network and no real key. The request shape follows D-029's #91 ruling: structured
//! outputs, no forced tool, no sampling, thinking or effort fields.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use queue_lib::ai::{self, ApiKind, Endpoint, Provider};
use queue_lib::meta::Meta;
use queue_lib::secrets::Memory;
use queue_lib::settings::Settings;

const KEY: &str = "sk-ant-test-0123456789";

struct Fake {
    base: String,
    request: thread::JoinHandle<String>,
}

/// One request, answered with `status` and `body`. A client that never connects fails the
/// test after a few seconds instead of hanging it (#91 review).
fn serve(status: u16, body: String) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let request = thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the client never connected"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("{e}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
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
    Fake { base, request }
}

/// A Messages API response: an empty thinking block (as current models send by default), then
/// the structured output as text.
fn answer(text: &str, stop_reason: &str) -> String {
    serde_json::json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "model": "claude-haiku-5-5",
        "content": [
            {"type": "thinking", "thinking": "", "signature": "c2ln"},
            {"type": "text", "text": text}
        ],
        "stop_reason": stop_reason,
        "usage": {"input_tokens": 300, "output_tokens": 40}
    })
    .to_string()
}

fn endpoint(base: &str) -> Endpoint {
    Endpoint {
        kind: ApiKind::Anthropic,
        base_url: base.to_owned(),
        model: "claude-haiku-5-5".into(),
        key: Some(KEY.into()),
    }
}

fn meta() -> Meta {
    Meta {
        title: Some("Rick Astley - Never Gonna Give You Up (Official Video)".into()),
        channel: Some("Rick Astley".into()),
        description: Some("SECRET-DESCRIPTION".into()),
        ..Meta::default()
    }
}

/// The request line, the headers (lowercased names) and the JSON body.
fn split(raw: &str) -> (String, Vec<(String, String)>, serde_json::Value) {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let mut lines = head.lines();
    let line = lines.next().unwrap().to_owned();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    (line, headers, serde_json::from_str(body).unwrap())
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

#[test]
fn n8_a_good_answer_comes_back_and_the_request_is_well_formed() {
    let fake = serve(
        200,
        answer(
            r#"{"filename": "Rick Astley – Never Gonna Give You Up"}"#,
            "end_turn",
        ),
    );
    let name = ai::ask(&endpoint(&fake.base), &meta());
    let raw = fake.request.join().unwrap();
    assert_eq!(name, Ok("Rick Astley – Never Gonna Give You Up".into()));
    let (line, headers, body) = split(&raw);
    assert!(line.starts_with("POST /v1/messages "), "{line}");
    assert_eq!(header(&headers, "x-api-key"), Some(KEY));
    assert_eq!(header(&headers, "anthropic-version"), Some("2023-06-01"));
    assert!(
        header(&headers, "content-type").is_some_and(|v| v.starts_with("application/json")),
        "{headers:?}"
    );
    assert_eq!(
        header(&headers, "authorization"),
        None,
        "the key goes in x-api-key only"
    );
    assert!(
        !headers.iter().any(|(k, _)| k == "anthropic-beta"),
        "no beta headers"
    );

    let keys: Vec<&str> = body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        ["max_tokens", "messages", "model", "output_config", "system"],
        "nothing else: no temperature, tools, tool_choice, thinking or stream"
    );
    assert_eq!(body["model"], "claude-haiku-5-5");
    assert_eq!(body["max_tokens"], 1024);
    assert_eq!(body["system"], ai::SYSTEM);
    let openai = ai::prompt(&meta());
    assert_eq!(body["messages"].as_array().unwrap().len(), 1);
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(
        body["messages"][0]["content"].as_str().unwrap(),
        openai[1].content,
        "the same data, byte for byte, as the OpenAI-compatible request"
    );
    assert_eq!(body["output_config"]["format"]["type"], "json_schema");
    assert_eq!(
        body["output_config"]["format"]["schema"],
        ai::anthropic_schema()
    );
}

#[test]
fn n8_the_schema_is_response_formats_without_length_limits() {
    let schema = ai::anthropic_schema();
    let filename = &schema["properties"]["filename"];
    assert_eq!(filename["type"], "string");
    assert!(filename.get("minLength").is_none() && filename.get("maxLength").is_none());
    assert_eq!(schema["required"], serde_json::json!(["filename"]));
    assert_eq!(schema["additionalProperties"], false);
    let mut original = ai::response_format()["json_schema"]["schema"].clone();
    let props = original["properties"]["filename"].as_object_mut().unwrap();
    props.remove("minLength");
    props.remove("maxLength");
    assert_eq!(
        schema, original,
        "derived from response_format, not a second copy"
    );
    assert_eq!(
        ai::parse(&format!(r#"{{"filename": "{}"}}"#, "x".repeat(151)), None),
        Err(ai::Reject::TooLong),
        "parse still enforces the length"
    );
}

#[test]
fn n8_a_base_with_v1_or_the_full_path_is_used_once() {
    for suffix in ["", "/", "/v1", "/v1/", "/v1/messages"] {
        let fake = serve(200, answer(r#"{"filename": "X"}"#, "end_turn"));
        let base = format!("{}{suffix}", fake.base);
        assert_eq!(
            ai::ask(&endpoint(&base), &meta()),
            Ok("X".into()),
            "{suffix}"
        );
        let raw = fake.request.join().unwrap();
        assert!(raw.starts_with("POST /v1/messages "), "{suffix}: {raw}");
    }
}

#[test]
fn n8_only_a_finished_turn_is_an_answer() {
    for stop in [
        "max_tokens",
        "refusal",
        "pause_turn",
        "tool_use",
        "stop_sequence",
    ] {
        let fake = serve(200, answer(r#"{"filename": "Looks Fine"}"#, stop));
        assert_eq!(
            ai::ask(&endpoint(&fake.base), &meta()),
            Err("The naming service sent an unreadable answer.".into()),
            "{stop}"
        );
        fake.request.join().unwrap();
    }
    let only_thinking = serde_json::json!({
        "content": [{"type": "thinking", "thinking": "", "signature": "c2ln"}],
        "stop_reason": "end_turn"
    })
    .to_string();
    let fake = serve(200, only_thinking);
    assert!(
        ai::ask(&endpoint(&fake.base), &meta()).is_err(),
        "no text block"
    );
    fake.request.join().unwrap();
}

#[test]
fn n8_errors_never_contain_the_key_the_url_the_metadata_or_the_body() {
    let echo = format!(
        r#"{{"type":"error","error":{{"type":"authentication_error","message":"bad key {KEY} for SECRET-DESCRIPTION"}}}}"#
    );
    for (status, body) in [
        (401, echo.clone()),
        (400, echo.clone()),
        (529, echo.clone()),
        (200, format!("not json {KEY}")),
    ] {
        let fake = serve(status, body);
        let error = ai::ask(&endpoint(&fake.base), &meta()).unwrap_err();
        fake.request.join().unwrap();
        for secret in [
            KEY,
            "SECRET-DESCRIPTION",
            "Rick Astley",
            "127.0.0.1",
            "authentication_error",
        ] {
            assert!(!error.contains(secret), "{status}: {error}");
        }
    }
    let fake = serve(401, echo);
    assert_eq!(
        ai::ask(&endpoint(&fake.base), &meta()),
        Err("The naming service answered with HTTP 401.".into())
    );
    fake.request.join().unwrap();
}

fn anthropic_settings(base: &str) -> Settings {
    Settings {
        naming_provider: Provider::Api,
        api_kind: ApiKind::Anthropic,
        api_base_url: base.to_owned(),
        api_model: "claude-haiku-5-5".into(),
        ..Settings::default()
    }
}

/// A listener that only records whether anything connected.
fn recorder() -> (String, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    (
        format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port()),
        listener,
    )
}

#[test]
fn n8_without_a_key_nothing_is_sent() {
    let (base, listener) = recorder();
    let settings = anthropic_settings(&base);
    let none = Memory::default();
    assert!(ai::askable(&settings), "decided without reading the key");
    assert_eq!(
        ai::endpoint(&settings, &none, ai::LocalModel::Missing).err(),
        Some(ai::Unavailable::NotSetUp)
    );
    let resolve = ai::Resolving(|| ai::endpoint(&settings, &none, ai::LocalModel::Missing));
    assert_eq!(
        ai::Namer::name_if(&resolve, &meta(), &|| true),
        None,
        "nothing asked, so no rules tag"
    );
    let tested = ai::test_connection(&settings, &none);
    assert_eq!(
        (tested.ok, tested.message.as_str()),
        (false, "Anthropic needs an API key.")
    );
    thread::sleep(Duration::from_millis(100));
    assert!(listener.accept().is_err(), "no connection was made");
}

#[test]
fn n8_test_connection_names_the_sample_and_hints_at_a_bad_model() {
    let fake = serve(200, answer(r#"{"filename": "Me at the zoo"}"#, "end_turn"));
    let tested = ai::test_connection(&anthropic_settings(&fake.base), &Memory::with(KEY));
    fake.request.join().unwrap();
    assert!(tested.ok, "{}", tested.message);
    assert!(tested.message.contains("Me at the zoo"));

    let fake = serve(400, r#"{"type":"error"}"#.into());
    let tested = ai::test_connection(&anthropic_settings(&fake.base), &Memory::with(KEY));
    fake.request.join().unwrap();
    assert_eq!(
        (tested.ok, tested.message.as_str()),
        (
            false,
            "The naming service answered with HTTP 400. Check the model name."
        )
    );
}

#[test]
fn n8_the_kind_is_saved_as_anthropic() {
    assert_eq!(
        serde_json::to_string(&ApiKind::Anthropic).unwrap(),
        r#""anthropic""#
    );
    let loaded: ApiKind = serde_json::from_str(r#""anthropic""#).unwrap();
    assert_eq!(loaded, ApiKind::Anthropic);
}

#[test]
fn n8_plain_http_to_another_machine_is_refused_before_sending() {
    let endpoint = endpoint("http://api.anthropic.com");
    assert_eq!(
        ai::ask(&endpoint, &meta()),
        Err("The naming service must use https:// (or http:// on this computer).".into())
    );
}
