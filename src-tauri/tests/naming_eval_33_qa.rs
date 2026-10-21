//! Independent #33(a) / N9 eval-runner QA using deterministic responses, never a real model.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

// Exercise the actual runner in a child test process, including its exit status and report.
#[path = "naming_eval_33.rs"]
mod eval_runner;

fn cases() -> Vec<serde_json::Value> {
    serde_json::from_str(include_str!("../naming-eval/titles.json")).unwrap()
}

// The 200–300 real, owner-labelled items #33's 2026-10-11 note asks for are #33(b)'s; the
// architect dropped this QA's size check for the 54-case seed (D-029). The runner takes any size.

/// `good` responses use the expected name (or a listed alternative); the rest are invalid.
fn run_fixture(good: usize, use_alternatives: bool) -> std::process::Output {
    let cases = cases();
    let total = cases.len();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut received = 0;
        while received < total {
            assert!(
                Instant::now() < deadline,
                "runner stopped after {received}/{total} requests"
            );
            let mut stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(e) => panic!("{e}"),
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 4096];
            let end = loop {
                let n = stream.read(&mut buf).unwrap();
                assert_ne!(n, 0, "incomplete request");
                request.extend_from_slice(&buf[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap();
                    if request.len() >= end + 4 + length {
                        break end + 4;
                    }
                }
            };
            let headers = String::from_utf8_lossy(&request[..end]);
            assert!(headers.starts_with("POST /v1/chat/completions "));
            assert!(!headers.to_ascii_lowercase().contains("authorization:"));
            let body: serde_json::Value = serde_json::from_slice(&request[end..]).unwrap();
            assert_eq!(body["model"], "qa-eval-model");
            assert_eq!(body["response_format"], queue_lib::ai::response_format());
            let metadata: serde_json::Value =
                serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
            assert_eq!(metadata["title"], cases[received]["meta"]["title"]);
            let case = &cases[received];
            let content = if received < good {
                let name = if use_alternatives {
                    case["accept"][0].as_str()
                } else {
                    None
                }
                .unwrap_or_else(|| case["expected"].as_str().unwrap());
                serde_json::json!({"filename":name}).to_string()
            } else {
                "not structured JSON".into()
            };
            let reply =
                serde_json::json!({"choices":[{"message":{"content":content}}]}).to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            )
            .unwrap();
            received += 1;
        }
        received
    });
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "eval_runner::n9_eval_scores_a_model",
            "--ignored",
            "--nocapture",
        ])
        .env("QUEUE_EVAL_ENDPOINT", url)
        .env("QUEUE_EVAL_MODEL", "qa-eval-model")
        .env_remove("QUEUE_EVAL_KEY")
        .output()
        .unwrap();
    assert_eq!(server.join().unwrap(), total);
    output
}

#[test]
fn n9_runner_accepts_listed_alternatives_and_reports_latency_quantiles() {
    let total = cases().len();
    let output = run_fixture(total, true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains(&format!("accepted {total}/{total} (100%)")),
        "{stdout}"
    );
    assert!(
        stdout.contains("p50 ") && stdout.contains("p95 "),
        "{stdout}"
    );
}

#[test]
fn n9_runner_passes_at_the_first_integer_score_above_90_percent() {
    let total = cases().len();
    let minimum = (total * 9).div_ceil(10);
    let output = run_fixture(minimum, false);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains(&format!("accepted {minimum}/{total} ")),
        "{stdout}"
    );
}

#[test]
fn n9_runner_fails_below_90_percent_instead_of_dropping_invalid_responses() {
    let total = cases().len();
    let below = (total * 9).div_ceil(10) - 1;
    let output = run_fixture(below, false);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(
        stdout.contains(&format!("accepted {below}/{total} ")),
        "{stdout}"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("below 90% accepted"));
}
