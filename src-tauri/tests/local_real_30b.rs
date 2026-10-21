//! N6 / #30(b) with the real llama-server. Both tests are ignored; they need files a plain
//! `cargo test` doesn't have.
//! - `QUEUE_QA_TEST_GGUF=<file>`: the pinned 1.2 MB test model (`scripts/sidecars.json`,
//!   `testModel`). CI runs this on all three OSes: it proves the server and its CPU backend load
//!   a model, answer one schema'd request, and stop leaving nothing behind (D-029).
//! - `QUEUE_QA_REAL_LLAMA=1`: the downloaded 4B model on the owner's Mac. It prints load time,
//!   first-ask latency and memory; it reads the model only, and refuses unless it's Ready.
//!
//! The server is `QUEUE_QA_LLAMA_DIR/llama-server`, by default `binaries/llama`.
#[path = "support/native.rs"]
mod native;

use queue_lib::{ai, local, models};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn server() -> PathBuf {
    let dir = std::env::var_os("QUEUE_QA_LLAMA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries/llama"));
    let exe = dir.join(format!("llama-server{}", std::env::consts::EXE_SUFFIX));
    assert!(exe.is_file(), "no llama-server at {}", exe.display());
    exe
}

fn gone(pid: u32) -> bool {
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        if native::process_has_exited(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn port_closed(endpoint: &ai::Endpoint) -> bool {
    let port = url::Url::parse(&endpoint.base_url).unwrap().port().unwrap();
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(200),
    )
    .is_err()
}

#[test]
#[ignore = "needs QUEUE_QA_TEST_GGUF (CI fetches it)"]
fn the_real_server_loads_a_model_answers_and_stops() {
    let model = PathBuf::from(std::env::var_os("QUEUE_QA_TEST_GGUF").expect("QUEUE_QA_TEST_GGUF"));
    let mut magic = [0u8; 4];
    std::io::Read::read_exact(&mut std::fs::File::open(&model).unwrap(), &mut magic).unwrap();
    assert_eq!(&magic, b"GGUF");
    let runtime = local::Runtime::new(server(), models::GPU);
    let started = Instant::now();
    let endpoint = runtime.endpoint(&model, "test").expect("the server starts");
    eprintln!("healthy after {:?}", started.elapsed());
    let pid = runtime.pid().unwrap();
    // A 260K-parameter model can't name anything, and its 128-token training context caps the
    // server's slot, so Queue's full naming prompt gets HTTP 400 here (the 4B check below sends
    // it). A short message with the same schema, key and endpoint checks the server answers.
    let chat = |key: Option<&str>| {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(60)))
            .http_status_as_error(false)
            .proxy(None)
            .build()
            .into();
        let mut request = agent.post(format!("{}/chat/completions", endpoint.base_url));
        if let Some(key) = key {
            request = request.header("Authorization", &format!("Bearer {key}"));
        }
        let mut response = request
            .send_json(serde_json::json!({
                "model": endpoint.model,
                "messages": [{"role": "user", "content": "Name: Me at the zoo"}],
                "response_format": ai::response_format(),
                "temperature": 0,
                "max_tokens": 16,
            }))
            .unwrap();
        let status = response.status().as_u16();
        (
            status,
            response.body_mut().read_to_string().unwrap_or_default(),
        )
    };
    let (status, body) = chat(endpoint.key.as_deref());
    assert_eq!(status, 200, "{body}");
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(
        body["choices"][0]["message"]["content"].is_string(),
        "{body}"
    );
    assert_eq!(chat(None).0, 401, "the per-launch key is required");
    runtime.stop();
    assert!(gone(pid), "no server left");
    assert!(port_closed(&endpoint), "its port is closed");
    assert_eq!(runtime.pid(), None);
}

/// Resident memory of `pid` in MiB, from `ps` (macOS and Linux).
#[cfg(unix)]
fn rss_mib(pid: u32) -> Option<u64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let kib: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    Some(kib / 1024)
}

#[cfg(unix)]
#[test]
#[ignore = "needs QUEUE_QA_REAL_LLAMA=1 and the downloaded 4B model"]
fn the_real_4b_model_loads_and_names_within_the_budget() {
    assert!(
        std::env::var_os("QUEUE_QA_REAL_LLAMA").is_some(),
        "QUEUE_QA_REAL_LLAMA=1"
    );
    let pinned = models::pinned(models::ModelId::Qwen3_4bInstruct2507);
    let dir = std::env::var_os("QUEUE_QA_MODELS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap())
                .join("Library/Application Support/dev.mcclease.queue/models")
        });
    assert_eq!(
        models::state(&dir, &pinned.pin()),
        models::State::Ready,
        "the 4B must be downloaded and checked"
    );
    let model = dir.join(pinned.file);
    let idle = Duration::from_secs(3);
    let runtime = local::Runtime::with_times(server(), models::GPU, idle, local::HEALTH_WAIT);
    let started = Instant::now();
    let endpoint = runtime
        .endpoint(&model, pinned.file)
        .expect("the server starts");
    let load = started.elapsed();
    let pid = runtime.pid().unwrap();
    let loaded = rss_mib(pid);
    let asked = Instant::now();
    let first = ai::ask(&endpoint, &queue_lib::naming::sample());
    let first_latency = asked.elapsed();
    let asked = Instant::now();
    let second = ai::ask(&endpoint, &queue_lib::naming::sample());
    let second_latency = asked.elapsed();
    let after = rss_mib(pid);
    std::thread::sleep(idle + Duration::from_millis(500));
    assert!(runtime.stop_if_idle(), "stopped by the idle timer");
    let stopped = gone(pid);
    println!(
        "4B: load {load:.2?}; first ask {first_latency:.2?} -> {first:?}; second ask \
         {second_latency:.2?} -> {second:?}; RSS loaded {loaded:?} MiB, after asks {after:?} \
         MiB; process gone after idle stop: {stopped}; GPU layers: {}",
        models::GPU
    );
    assert!(first.is_ok() && second.is_ok());
    assert!(first_latency < ai::TIMEOUT, "within the 10 s budget");
    assert!(stopped);
}
