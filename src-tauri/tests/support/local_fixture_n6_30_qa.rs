//! Independent N6/#30 fixture. Model text selects faults; no model downloads or shell.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let value = |flag: &str| args[args.iter().position(|a| a == flag).unwrap() + 1].clone();
    let model = std::path::PathBuf::from(value("-m"));
    let scenario = std::fs::read_to_string(&model).unwrap();
    let listener =
        TcpListener::bind(format!("127.0.0.1:{}", value("--port").to_str().unwrap())).unwrap();
    let log = |suffix: &str, value: &str| {
        let mut path = model.as_os_str().to_owned();
        path.push(suffix);
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(file, "{value}").unwrap();
    };
    log(".launches", &std::process::id().to_string());
    log(".port", &listener.local_addr().unwrap().port().to_string());
    let key = std::env::var("LLAMA_API_KEY").unwrap();
    let health = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let (scenario, model, key, health) =
            (scenario.clone(), model.clone(), key.clone(), health.clone());
        std::thread::spawn(move || {
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
            let (mut size, mut authorized) = (0, false);
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        size = value.trim().parse().unwrap();
                    }
                    if name.eq_ignore_ascii_case("authorization") {
                        authorized = value.trim() == format!("Bearer {key}");
                    }
                }
            }
            let mut body = vec![0; size];
            if reader.read_exact(&mut body).is_err() {
                return;
            }
            let mut logpath = model.as_os_str().to_owned();
            logpath.push(".requests");
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(logpath)
                .unwrap();
            writeln!(file, "{path}\t{}", String::from_utf8_lossy(&body)).unwrap();
            if (path == "/health" && scenario == "health-stall")
                || (path != "/health" && scenario == "chat-stall")
            {
                std::thread::sleep(Duration::from_secs(30));
                return;
            }
            let (status, body) = if path == "/health" {
                if scenario == "health-retry" && health.fetch_add(1, Ordering::SeqCst) < 3 {
                    ("503 Service Unavailable", "{}".to_owned())
                } else {
                    ("200 OK", "{}".to_owned())
                }
            } else if !authorized {
                ("401 Unauthorized", "{}".to_owned())
            } else {
                let content = scenario
                    .strip_prefix("answer\n")
                    .unwrap_or(r#"{"filename":"Local name"}"#);
                let escaped = content
                    .chars()
                    .map(|c| match c {
                        '"' => "\\\"".to_owned(),
                        '\\' => "\\\\".to_owned(),
                        '\n' => "\\n".to_owned(),
                        '\r' => "\\r".to_owned(),
                        '\t' => "\\t".to_owned(),
                        c => c.to_string(),
                    })
                    .collect::<String>();
                (
                    "200 OK",
                    format!(r#"{{"choices":[{{"message":{{"content":"{escaped}"}}}}]}}"#),
                )
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        });
    }
}
