//! Independent #31 / N7 QA. Tiny loopback responses; never downloads a real model.
#[path = "support/accept.rs"]
mod accept;

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use queue_lib::models::{self, Options, Outcome, Pin, Progress, State};
use queue_lib::{proc, settings::Settings};
use sha2::{Digest, Sha256};

const DATA: &[u8] = b"GGUF a small pinned fixture";

struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "queue-n7-qa-日本語-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn pin(url: String) -> Pin {
    Pin {
        url,
        file: "fixture.gguf".into(),
        size: DATA.len() as u64,
        sha256: format!("{:x}", Sha256::digest(DATA)),
    }
}

fn opts() -> Options {
    Options {
        chunk: DATA.len() as u64,
        budget: Duration::from_secs(3),
        pause: Duration::from_millis(1),
    }
}

fn request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut head = Vec::new();
    let mut byte = [0];
    while !head.ends_with(b"\r\n\r\n") {
        assert_eq!(stream.read(&mut byte).unwrap(), 1);
        head.push(byte[0]);
    }
    String::from_utf8(head).unwrap()
}

fn response(stream: &mut TcpStream, status: u16, range: Option<&str>, body: &[u8]) {
    write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    )
    .unwrap();
    if let Some(range) = range {
        write!(stream, "Content-Range: {range}\r\n").unwrap();
    }
    stream.write_all(b"\r\n").unwrap();
    stream.write_all(body).unwrap();
}

fn server(f: impl FnOnce(TcpListener) + Send + 'static) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/model", listener.local_addr().unwrap());
    (url, thread::spawn(move || f(listener)))
}

#[test]
fn complete_partial_is_verified_offline_in_a_unicode_directory() {
    let dir = Dir::new();
    let p = pin("http://127.0.0.1:1/no-network-needed".into());
    fs::write(dir.0.join("fixture.gguf.part"), DATA).unwrap();
    let mut progress = Vec::new();
    assert_eq!(
        models::download(&p, &dir.0, &opts(), &mut |x| progress.push(x)),
        Ok(Outcome::Done)
    );
    assert_eq!(progress, [Progress::Verifying]);
    assert_eq!(models::state(&dir.0, &p), State::Ready);
    assert_eq!(fs::read(dir.0.join(&p.file)).unwrap(), DATA);
    assert_eq!(
        models::download(&p, &dir.0, &opts(), &mut |_| panic!(
            "ready must be a no-op"
        )),
        Ok(Outcome::Done)
    );
}

#[test]
fn a_corrupt_complete_partial_is_deleted_without_publishing_or_network() {
    let dir = Dir::new();
    let p = pin("http://127.0.0.1:1/no-network-needed".into());
    let mut corrupt = DATA.to_vec();
    *corrupt.last_mut().unwrap() ^= 1;
    fs::write(dir.0.join("fixture.gguf.part"), corrupt).unwrap();
    assert_eq!(
        models::download(&p, &dir.0, &opts(), &mut |_| {}),
        Err(models::MISMATCH.into())
    );
    assert_eq!(models::state(&dir.0, &p), State::Missing);
    assert!(!dir.0.join("fixture.gguf.sha256").exists());
}

#[test]
fn an_oversize_partial_restarts_at_zero_and_checks_the_new_whole_file() {
    let (url, worker) = server(|listener| {
        let mut stream = accept::accept(&listener);
        assert!(request(&mut stream).contains(&format!("bytes=0-{}", DATA.len() - 1)));
        response(
            &mut stream,
            206,
            Some(&format!("bytes 0-{}/{}", DATA.len() - 1, DATA.len())),
            DATA,
        );
    });
    let p = pin(url);
    let dir = Dir::new();
    fs::write(dir.0.join("fixture.gguf.part"), vec![0; DATA.len() + 1]).unwrap();
    assert_eq!(
        models::download(&p, &dir.0, &opts(), &mut |_| {}),
        Ok(Outcome::Done)
    );
    worker.join().unwrap();
    assert_eq!(fs::read(dir.0.join(&p.file)).unwrap(), DATA);
}

#[test]
fn range_416_discards_the_partial_and_a_retry_requests_zero() {
    let (url, worker) = server(|listener| {
        let mut stream = accept::accept(&listener);
        assert!(request(&mut stream).contains("bytes=4-"));
        response(&mut stream, 416, None, b"");
        drop(stream);
        let mut stream = accept::accept(&listener);
        assert!(request(&mut stream).contains("bytes=0-"));
        response(
            &mut stream,
            206,
            Some(&format!("bytes 0-{}/{}", DATA.len() - 1, DATA.len())),
            DATA,
        );
    });
    let dir = Dir::new();
    fs::write(dir.0.join("fixture.gguf.part"), b"GGUF").unwrap();
    assert_eq!(
        models::download(&pin(url), &dir.0, &opts(), &mut |_| {}),
        Ok(Outcome::Done)
    );
    worker.join().unwrap();
}

#[test]
fn malformed_or_wrong_start_ranges_preserve_the_existing_partial() {
    for range in [
        None,
        Some("bytes */27"),
        Some("bytes x-y/z"),
        Some("items 4-26/27"),
        Some("bytes 3-26/27"),
        Some("bytes 4-3/27"),
    ] {
        let (url, worker) = server(move |listener| {
            let mut stream = accept::accept(&listener);
            request(&mut stream);
            response(&mut stream, 206, range, &DATA[4..]);
        });
        let dir = Dir::new();
        fs::write(dir.0.join("fixture.gguf.part"), b"GGUF").unwrap();
        assert!(
            models::download(&pin(url), &dir.0, &opts(), &mut |_| {}).is_err(),
            "{range:?}"
        );
        worker.join().unwrap();
        assert_eq!(
            fs::read(dir.0.join("fixture.gguf.part")).unwrap(),
            b"GGUF",
            "{range:?}"
        );
        assert!(!dir.0.join("fixture.gguf").exists());
    }
}

#[test]
fn a_file_created_during_verification_is_never_overwritten() {
    let dir = Dir::new();
    let p = pin("http://127.0.0.1:1/offline".into());
    fs::write(dir.0.join("fixture.gguf.part"), DATA).unwrap();
    let result = models::download(&p, &dir.0, &opts(), &mut |progress| {
        if progress == Progress::Verifying {
            fs::write(dir.0.join(&p.file), b"another writer's file").unwrap();
        }
    });
    assert_eq!(result, Err(models::TAKEN.into()));
    assert_eq!(
        fs::read(dir.0.join(&p.file)).unwrap(),
        b"another writer's file"
    );
    assert_eq!(fs::read(dir.0.join("fixture.gguf.part")).unwrap(), DATA);
    assert!(!dir.0.join("fixture.gguf.sha256").exists());
}

#[test]
fn both_model_choices_survive_restart_and_unknown_values_fall_back_alone() {
    let dir = Dir::new();
    let path = dir.0.join("settings.json");
    for model in [
        models::ModelId::Qwen3_4bInstruct2507,
        models::ModelId::Qwen3_5_2b,
    ] {
        let settings = Settings {
            local_model: Some(model),
            ..Default::default()
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), settings);
    }
    for bad in [
        serde_json::json!("future-model"),
        serde_json::json!(42),
        serde_json::Value::Null,
    ] {
        fs::write(
            &path,
            serde_json::to_vec(
                &serde_json::json!({"localModel":bad,"format":"audio","apiModel":"keep me"}),
            )
            .unwrap(),
        )
        .unwrap();
        let loaded = Settings::load(&path);
        assert_eq!(loaded.local_model, None);
        assert_eq!(loaded.api_model, "keep me");
        assert_eq!(loaded.format, queue_lib::download::Format::Audio);
    }
}

#[test]
fn two_body_timeouts_stop_and_preserve_the_resumable_bytes() {
    let (url, server) = server(|listener| {
        let mut held = Vec::new();
        for _ in 0..2 {
            let mut stream = accept::accept(&listener);
            assert!(request(&mut stream).contains("bytes=4-26"));
            write!(stream, "HTTP/1.1 206 X\r\nContent-Length: 23\r\nContent-Range: bytes 4-26/27\r\nConnection: close\r\n\r\n").unwrap();
            stream.flush().unwrap();
            held.push(stream); // Headers arrive, but neither response sends a body.
        }
        thread::sleep(Duration::from_millis(250));
        drop(held);
    });
    let dir = Dir::new();
    fs::write(dir.0.join("fixture.gguf.part"), b"GGUF").unwrap();
    let options = Options {
        budget: Duration::from_millis(100),
        ..opts()
    };
    let started = Instant::now();
    assert_eq!(
        models::download(&pin(url), &dir.0, &options, &mut |_| {}),
        Err(models::STALLED.into())
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    server.join().unwrap();
    assert_eq!(fs::read(dir.0.join("fixture.gguf.part")).unwrap(), b"GGUF");
}

#[test]
fn cancel_at_verification_keeps_the_complete_partial_for_offline_resume() {
    let dir = Dir::new();
    let p = pin("http://127.0.0.1:1/offline".into());
    fs::write(dir.0.join("fixture.gguf.part"), DATA).unwrap();
    let flag = Arc::new(AtomicBool::new(false));
    let result = proc::with_cancel(flag.clone(), || {
        models::download(&p, &dir.0, &opts(), &mut |p| {
            if p == Progress::Verifying {
                flag.store(true, Ordering::SeqCst);
            }
        })
    });
    assert_eq!(result, Ok(Outcome::Stopped));
    assert!(!dir.0.join("fixture.gguf").exists());
    assert_eq!(fs::read(dir.0.join("fixture.gguf.part")).unwrap(), DATA);
    assert_eq!(
        models::download(&p, &dir.0, &opts(), &mut |_| {}),
        Ok(Outcome::Done)
    );
}

#[test]
fn cancel_during_a_stalled_body_must_not_wait_for_the_request_budget() {
    // Release the server even on failure, so the assertion cannot leave a hung test process.
    let (release_tx, release_rx) = mpsc::channel();
    let (url, server) = server(move |listener| {
        let mut stream = accept::accept(&listener);
        request(&mut stream);
        write!(stream, "HTTP/1.1 206 X\r\nContent-Length: {}\r\nContent-Range: bytes 0-{}/{}\r\nConnection: close\r\n\r\n", DATA.len(), DATA.len() - 1, DATA.len()).unwrap();
        stream.write_all(b"GGUF").unwrap();
        stream.flush().unwrap();
        let _ = release_rx.recv_timeout(Duration::from_secs(5));
    });
    let dir = Dir::new();
    let p = pin(url);
    let flag = Arc::new(AtomicBool::new(false));
    let worker_flag = flag.clone();
    let worker_dir = dir.0.clone();
    let (progress_tx, progress_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let result = proc::with_cancel(worker_flag, || {
            models::download(&p, &worker_dir, &opts(), &mut |p| {
                if p == Progress::Received(4) {
                    let _ = progress_tx.send(());
                }
            })
        });
        done_tx.send(result).unwrap();
    });
    progress_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    thread::sleep(Duration::from_millis(100)); // Let the worker enter its next blocking read.
    let asked = Instant::now();
    flag.store(true, Ordering::SeqCst);
    let prompt = done_rx.recv_timeout(Duration::from_secs(1));
    let elapsed = asked.elapsed();
    let _ = release_tx.send(());
    let result = prompt
        .clone()
        .unwrap_or_else(|_| done_rx.recv_timeout(Duration::from_secs(4)).unwrap());
    worker.join().unwrap();
    server.join().unwrap();
    assert_eq!(result, Ok(Outcome::Stopped));
    assert_eq!(fs::read(dir.0.join("fixture.gguf.part")).unwrap(), b"GGUF");
    assert!(
        prompt.is_ok(),
        "Cancel was ignored for {elapsed:?} while the response body was stalled (production budget: 180 s)"
    );
}
