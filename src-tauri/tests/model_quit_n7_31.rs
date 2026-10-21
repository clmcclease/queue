//! #31 / N7: quitting stops a model download promptly and keeps the partial file to resume.
//! Its own test binary: `proc::shut_down` is process-wide.
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};

use queue_lib::models::{self, Options, Outcome, Pin, State};
use queue_lib::proc;

#[test]
fn n7_quitting_stops_a_download_and_keeps_the_partial_file() {
    let mut data = b"GGUF".to_vec();
    data.resize(4 * 1024 * 1024, 7);
    let size = data.len() as u64;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://127.0.0.1:{}/model",
        listener.local_addr().unwrap().port()
    );
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut head = [0; 2048];
            let _ = stream.read(&mut head);
            let header = format!(
                "HTTP/1.1 206 X\r\nContent-Length: {size}\r\nContent-Range: bytes 0-{}/{size}\r\nConnection: close\r\n\r\n",
                size - 1
            );
            let _ = stream.write_all(header.as_bytes());
            for piece in data.chunks(16 * 1024) {
                thread::sleep(Duration::from_millis(30));
                if stream.write_all(piece).is_err() {
                    break;
                }
            }
        }
    });
    let pin = Pin {
        url,
        file: "quit.gguf".into(),
        size,
        sha256: "0".repeat(64),
    };
    let dir = std::env::temp_dir().join(format!("queue-n7-quit-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let opts = Options {
        chunk: size,
        budget: Duration::from_secs(30),
        pause: Duration::from_millis(10),
    };
    let (worker_pin, worker_dir) = (pin.clone(), dir.clone());
    let worker =
        thread::spawn(move || models::download(&worker_pin, &worker_dir, &opts, &mut |_| {}));
    let part = dir.join("quit.gguf.part");
    let until = Instant::now() + Duration::from_secs(10);
    while fs::metadata(&part).map(|m| m.len()).unwrap_or(0) == 0 && Instant::now() < until {
        thread::sleep(Duration::from_millis(10));
    }
    let asked = Instant::now();
    proc::shut_down();
    assert_eq!(worker.join().unwrap(), Ok(Outcome::Stopped));
    assert!(
        asked.elapsed() < Duration::from_secs(1),
        "{:?}",
        asked.elapsed()
    );
    assert!(matches!(models::state(&dir, &pin), State::Partial { received } if received > 0));
    fs::remove_dir_all(dir).unwrap();
}
