//! A stand-in for llama-server (#30(b)), compiled by `local_n6_30b.rs`. The `-m` file says what
//! it does: `ok` answers `/health` and one chat completion; `exit` exits at once; `slow` never
//! becomes healthy. It appends its arguments, one launch per line, to `<model>.args`, and only
//! answers a completion carrying the key from `LLAMA_API_KEY`.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        let i = args.iter().position(|a| a == flag).expect(flag);
        args[i + 1].clone()
    };
    let model = value("-m");
    let port = value("--port");
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("{model}.args"))
        .unwrap();
    writeln!(log, "{}", args.join("\t")).unwrap();
    let scenario = std::fs::read_to_string(&model).unwrap();
    if scenario.trim() == "exit" {
        std::process::exit(1);
    }
    let key = std::env::var("LLAMA_API_KEY").unwrap_or_default();
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).unwrap();
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            continue;
        }
        let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
        let (mut length, mut auth) = (0usize, String::new());
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                break;
            }
            let lower = header.to_ascii_lowercase();
            if let Some(v) = lower.strip_prefix("content-length:") {
                length = v.trim().parse().unwrap_or(0);
            }
            if lower.starts_with("authorization:") {
                auth = header["authorization:".len()..].trim().to_owned();
            }
        }
        let mut body = vec![0; length];
        let _ = reader.read_exact(&mut body);
        let (status, text) = match path.as_str() {
            "/health" if scenario.trim() == "slow" => ("503 Service Unavailable", "{}".to_owned()),
            "/health" => ("200 OK", r#"{"status":"ok"}"#.to_owned()),
            "/v1/chat/completions" if auth != format!("Bearer {key}") || key.is_empty() => {
                ("401 Unauthorized", "{}".to_owned())
            }
            "/v1/chat/completions" => (
                "200 OK",
                r#"{"choices":[{"message":{"content":"{\"filename\":\"Fake Local Name\"}"}}]}"#
                    .to_owned(),
            ),
            _ => ("404 Not Found", "{}".to_owned()),
        };
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
            text.len()
        );
    }
}
