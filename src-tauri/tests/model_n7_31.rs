//! #31 / N7: the model download against a fake HTTP server on 127.0.0.1, serving a small
//! in-memory "model" with Range support. The real pins are never fetched.
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use queue_lib::models::{self, Options, Outcome, Pin, Progress, State};
use queue_lib::proc;
use sha2::{Digest, Sha256};

/// How the fake server misbehaves.
#[derive(Default)]
struct Knobs {
    /// Statuses to answer instead of the body, one per model request.
    statuses: VecDeque<u16>,
    /// Answer 200 with the whole file, ignoring Range.
    ignore_range: bool,
    /// Claim this total in Content-Range.
    total: Option<u64>,
    /// Close the next response after this many body bytes.
    cut: Option<usize>,
    /// Sleep this long per 16 KiB written.
    pace: Option<Duration>,
    /// path → (Location, extra headers)
    redirects: HashMap<String, (String, Vec<(String, String)>)>,
}

struct Fake {
    base: String,
    data: Arc<Vec<u8>>,
    knobs: Arc<Mutex<Knobs>>,
    /// Each request as "<path> <range>".
    log: Arc<Mutex<Vec<String>>>,
}

fn model(len: usize) -> Vec<u8> {
    let mut data = b"GGUF".to_vec();
    let mut x: u32 = 2463534242;
    while data.len() < len {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        data.push(x as u8);
    }
    data
}

fn hex(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

impl Fake {
    fn start(data: Vec<u8>) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let fake = Fake {
            base,
            data: Arc::new(data),
            knobs: Arc::new(Mutex::new(Knobs::default())),
            log: Arc::new(Mutex::new(Vec::new())),
        };
        let (data, knobs, log) = (fake.data.clone(), fake.knobs.clone(), fake.log.clone());
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (data, knobs, log) = (data.clone(), knobs.clone(), log.clone());
                thread::spawn(move || serve(stream, &data, &knobs, &log));
            }
        });
        fake
    }

    fn pin(&self, path: &str) -> Pin {
        Pin {
            url: format!("{}{path}", self.base),
            file: "fake.gguf".into(),
            size: self.data.len() as u64,
            sha256: hex(&self.data),
        }
    }

    fn requests(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

fn serve(mut stream: TcpStream, data: &[u8], knobs: &Mutex<Knobs>, log: &Mutex<Vec<String>>) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut head = Vec::new();
    let mut byte = [0];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).unwrap_or(0) == 0 {
            return;
        }
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let range = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("range: ")
                .map(str::to_owned)
        })
        .unwrap_or_default();
    log.lock().unwrap().push(format!("{path} {range}"));
    let mut k = knobs.lock().unwrap();
    if let Some((location, extra)) = k.redirects.get(&path) {
        let mut reply = format!(
            "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n"
        );
        for (name, value) in extra {
            reply.push_str(&format!("{name}: {value}\r\n"));
        }
        reply.push_str("\r\n");
        let _ = stream.write_all(reply.as_bytes());
        return;
    }
    if let Some(status) = k.statuses.pop_front() {
        let _ = stream.write_all(
            format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        );
        return;
    }
    let total = k.total.unwrap_or(data.len() as u64);
    let (status, from, to) = match (k.ignore_range, parse_range(&range)) {
        (false, Some((from, to))) => (206, from, to.min(data.len() as u64 - 1)),
        _ => (200, 0, data.len() as u64 - 1),
    };
    let body = &data[from as usize..=to as usize];
    let cut = k.cut.take();
    let pace = k.pace;
    drop(k);
    let mut reply = format!(
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if status == 206 {
        reply.push_str(&format!("Content-Range: bytes {from}-{to}/{total}\r\n"));
    }
    reply.push_str("\r\n");
    if stream.write_all(reply.as_bytes()).is_err() {
        return;
    }
    let send = &body[..cut.unwrap_or(body.len()).min(body.len())];
    for piece in send.chunks(16 * 1024) {
        if let Some(pace) = pace {
            thread::sleep(pace);
        }
        if stream.write_all(piece).is_err() {
            return;
        }
    }
}

fn parse_range(range: &str) -> Option<(u64, u64)> {
    let (from, to) = range.strip_prefix("bytes=")?.split_once('-')?;
    Some((from.parse().ok()?, to.parse().ok()?))
}

fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("queue-n7-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn opts() -> Options {
    Options {
        chunk: 256 * 1024,
        budget: Duration::from_secs(10),
        pause: Duration::from_millis(10),
    }
}

fn run(pin: &Pin, dir: &Path, opts: &Options) -> (Result<Outcome, String>, Vec<Progress>) {
    let mut seen = Vec::new();
    let result = models::download(pin, dir, opts, &mut |p| seen.push(p));
    (result, seen)
}

#[test]
fn n7_a_full_download_is_chunked_verified_and_named() {
    let fake = Fake::start(model(3 * 1024 * 1024 + 123));
    let pin = fake.pin("/model");
    let dir = dir("full");
    let (result, seen) = run(&pin, &dir, &opts());
    assert_eq!(result, Ok(Outcome::Done));
    let saved = fs::read(dir.join("fake.gguf")).unwrap();
    assert_eq!(hex(&saved), pin.sha256);
    assert_eq!(
        fs::read_to_string(dir.join("fake.gguf.sha256")).unwrap(),
        format!("{}  fake.gguf\n", pin.sha256)
    );
    assert!(!dir.join("fake.gguf.part").exists());
    assert_eq!(models::state(&dir, &pin), State::Ready);
    assert!(fake.requests().len() >= 13, "one request per 256 KiB chunk");
    assert!(
        fake.requests().iter().all(|r| r.contains("bytes=")),
        "always ranged"
    );
    let received: Vec<u64> = seen
        .iter()
        .filter_map(|p| match p {
            Progress::Received(n) => Some(*n),
            Progress::Verifying => None,
        })
        .collect();
    assert!(received.windows(2).all(|w| w[0] <= w[1]), "monotonic");
    assert_eq!(*received.last().unwrap(), pin.size);
    assert_eq!(*seen.last().unwrap(), Progress::Verifying);
    assert_eq!(
        run(&pin, &dir, &opts()).0,
        Ok(Outcome::Done),
        "already there"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn n7_a_cut_off_response_resumes_from_the_partial_file() {
    let fake = Fake::start(model(1024 * 1024));
    fake.knobs.lock().unwrap().cut = Some(100_000);
    let pin = fake.pin("/model");
    let dir = dir("cut");
    assert_eq!(run(&pin, &dir, &opts()).0, Ok(Outcome::Done));
    assert_eq!(hex(&fs::read(dir.join("fake.gguf")).unwrap()), pin.sha256);
    let requests = fake.requests();
    assert!(requests[0].ends_with("bytes=0-262143"), "{requests:?}");
    assert!(requests[1].ends_with("bytes=100000-362143"), "{requests:?}");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn n7_a_resumed_download_continues_where_the_last_one_stopped() {
    let data = model(600 * 1024);
    let fake = Fake::start(data.clone());
    let pin = fake.pin("/model");
    let dir = dir("resume");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("fake.gguf.part"), &data[..300_000]).unwrap();
    assert_eq!(
        models::state(&dir, &pin),
        State::Partial { received: 300_000 }
    );
    assert_eq!(run(&pin, &dir, &opts()).0, Ok(Outcome::Done));
    assert!(fake.requests()[0].ends_with("bytes=300000-562143"));
    assert_eq!(hex(&fs::read(dir.join("fake.gguf")).unwrap()), pin.sha256);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn n7_a_checksum_mismatch_deletes_the_download_and_says_so() {
    let fake = Fake::start(model(512 * 1024));
    let mut pin = fake.pin("/model");
    pin.sha256 = hex(b"something else");
    let dir = dir("mismatch");
    assert_eq!(run(&pin, &dir, &opts()).0, Err(models::MISMATCH.to_owned()));
    assert!(!dir.join("fake.gguf").exists());
    assert!(!dir.join("fake.gguf.part").exists());
    assert!(!dir.join("fake.gguf.sha256").exists());
    assert!(models::MISMATCH.contains("deleted"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn n7_cancel_stops_promptly_and_keeps_the_partial_file() {
    let fake = Fake::start(model(2 * 1024 * 1024));
    fake.knobs.lock().unwrap().pace = Some(Duration::from_millis(30));
    let pin = fake.pin("/model");
    let dir = dir("cancel");
    let flag = Arc::new(AtomicBool::new(false));
    let (thread_flag, thread_pin, thread_dir) = (flag.clone(), pin.clone(), dir.clone());
    let worker = thread::spawn(move || {
        proc::with_cancel(thread_flag, || {
            models::download(&thread_pin, &thread_dir, &opts(), &mut |_| {})
        })
    });
    let part = dir.join("fake.gguf.part");
    let until = Instant::now() + Duration::from_secs(10);
    while fs::metadata(&part).map(|m| m.len()).unwrap_or(0) == 0 && Instant::now() < until {
        thread::sleep(Duration::from_millis(10));
    }
    let asked = Instant::now();
    flag.store(true, Ordering::SeqCst);
    assert_eq!(worker.join().unwrap(), Ok(Outcome::Stopped));
    assert!(
        asked.elapsed() < Duration::from_secs(1),
        "{:?}",
        asked.elapsed()
    );
    assert!(matches!(models::state(&dir, &pin), State::Partial { received } if received > 0));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn n7_a_source_that_ignores_range_is_refused_without_writing() {
    let data = model(300 * 1024);
    let fake = Fake::start(data.clone());
    fake.knobs.lock().unwrap().ignore_range = true;
    let pin = fake.pin("/model");
    let dir = dir("norange");
    assert_eq!(
        run(&pin, &dir, &opts()).0,
        Err(models::NO_RANGES.to_owned())
    );
    assert!(!dir.join("fake.gguf.part").exists(), "nothing written");
    fs::write(dir.join("fake.gguf.part"), &data[..1000]).unwrap();
    assert_eq!(
        run(&pin, &dir, &opts()).0,
        Err(models::NO_RANGES.to_owned())
    );
    assert_eq!(
        fs::read(dir.join("fake.gguf.part")).unwrap(),
        &data[..1000],
        "kept"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn n7_a_different_file_at_the_source_is_refused_before_its_body() {
    let fake = Fake::start(model(300 * 1024));
    fake.knobs.lock().unwrap().total = Some(999);
    let pin = fake.pin("/model");
    let dir = dir("total");
    assert_eq!(
        run(&pin, &dir, &opts()).0,
        Err(models::NOT_PINNED.to_owned())
    );
    assert!(!dir.join("fake.gguf.part").exists());

    // Hugging Face's redirect names the file: a mismatch stops before the CDN is asked.
    let fake = Fake::start(model(300 * 1024));
    for header in [
        ("x-linked-size", "12345".to_owned()),
        ("x-linked-etag", format!("\"{}\"", "0".repeat(64))),
    ] {
        fake.knobs.lock().unwrap().redirects.insert(
            "/resolve".into(),
            ("/model".into(), vec![(header.0.into(), header.1)]),
        );
        let pin = fake.pin("/resolve");
        assert_eq!(
            run(&pin, &dir, &opts()).0,
            Err(models::NOT_PINNED.to_owned())
        );
    }
    assert!(
        fake.requests().iter().all(|r| r.starts_with("/resolve")),
        "{:?}",
        fake.requests()
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn n7_redirects_are_followed_only_to_https_or_this_computer_and_at_most_five() {
    let fake = Fake::start(model(300 * 1024));
    let pin = fake.pin("/resolve");
    {
        let mut k = fake.knobs.lock().unwrap();
        k.redirects.insert(
            "/resolve".into(),
            (
                "hop".into(),
                vec![
                    ("x-linked-size".into(), pin.size.to_string()),
                    ("x-linked-etag".into(), format!("\"{}\"", pin.sha256)),
                ],
            ),
        );
        k.redirects.insert("/hop".into(), ("/model".into(), vec![]));
    }
    let dir = dir("redirects");
    assert_eq!(
        run(&pin, &dir, &opts()).0,
        Ok(Outcome::Done),
        "relative Location"
    );
    let _ = fs::remove_dir_all(&dir);

    fake.knobs
        .lock()
        .unwrap()
        .redirects
        .insert("/away".into(), ("http://example.com/m".into(), vec![]));
    let error = run(&fake.pin("/away"), &dir, &opts()).0.unwrap_err();
    assert!(error.contains("https://"), "{error}");

    {
        let mut k = fake.knobs.lock().unwrap();
        for i in 0..6 {
            k.redirects
                .insert(format!("/loop{i}"), (format!("/loop{}", i + 1), vec![]));
        }
    }
    let error = run(&fake.pin("/loop0"), &dir, &opts()).0.unwrap_err();
    assert!(error.contains("too many"), "{error}");
    assert!(
        !fake.requests().iter().any(|r| r.starts_with("/loop6")),
        "a sixth hop isn't taken"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn n7_two_empty_attempts_stall_but_a_slow_one_that_moves_bytes_continues() {
    let data = model(300 * 1024);
    let fake = Fake::start(data.clone());
    fake.knobs.lock().unwrap().statuses = VecDeque::from([503, 503]);
    let pin = fake.pin("/model");
    let dir = dir("stall");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("fake.gguf.part"), &data[..1000]).unwrap();
    assert_eq!(run(&pin, &dir, &opts()).0, Err(models::STALLED.to_owned()));
    assert_eq!(
        fs::metadata(dir.join("fake.gguf.part")).unwrap().len(),
        1000,
        "kept"
    );
    fs::remove_dir_all(&dir).unwrap();

    // Each request times out part way through its chunk, but bytes arrive, so it goes on.
    let fake = Fake::start(model(400 * 1024));
    fake.knobs.lock().unwrap().pace = Some(Duration::from_millis(40));
    let pin = fake.pin("/model");
    let slow = Options {
        budget: Duration::from_millis(250),
        ..opts()
    };
    assert_eq!(run(&pin, &dir, &slow).0, Ok(Outcome::Done));
    assert!(fake.requests().len() > 2, "several partial requests");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn n7_an_existing_file_is_never_replaced() {
    let fake = Fake::start(model(300 * 1024));
    let pin = fake.pin("/model");
    let dir = dir("taken");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("fake.gguf"), b"the user's own file").unwrap();
    assert_eq!(models::state(&dir, &pin), State::Unverified);
    assert_eq!(run(&pin, &dir, &opts()).0, Err(models::TAKEN.to_owned()));
    assert_eq!(
        fs::read(dir.join("fake.gguf")).unwrap(),
        b"the user's own file"
    );
    assert!(fake.requests().is_empty(), "nothing downloaded");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn n7_a_file_that_isnt_a_gguf_model_is_refused_at_its_first_bytes() {
    let mut data = model(300 * 1024);
    data[..4].copy_from_slice(b"<htm");
    let fake = Fake::start(data);
    let pin = fake.pin("/model");
    let dir = dir("magic");
    assert_eq!(
        run(&pin, &dir, &opts()).0,
        Err(models::NOT_PINNED.to_owned())
    );
    assert!(!dir.join("fake.gguf.part").exists());
    let _ = fs::remove_dir_all(dir);
}
