//! N6 / #30(b): the local server's lifecycle, against a stand-in llama-server
//! (`support/fake_llama_30.rs`): started on demand, reused, stopped when idle, on `stop`, and
//! when the model changes; a failed or slow start leaves no process behind.
#[path = "support/native.rs"]
mod native;

use queue_lib::{ai, local, meta::Meta};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

static FAKE: OnceLock<PathBuf> = OnceLock::new();

/// The stand-in, compiled once with the pinned rustc.
fn fake() -> &'static PathBuf {
    FAKE.get_or_init(|| {
        let target = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/qa-local-30b");
        std::fs::create_dir_all(&target).unwrap();
        let output = target.join(format!(
            "llama-server-{}{}",
            std::process::id(),
            std::env::consts::EXE_SUFFIX
        ));
        let result = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args(["--edition=2024", "--crate-name", "fake_llama_30"])
            .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/support/fake_llama_30.rs"))
            .arg("-o")
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "fixture compilation: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        output
    })
}

/// A model file whose content tells the stand-in what to do, in a fresh folder.
fn model(test: &str, scenario: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("queue-30b-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{scenario} model.gguf"));
    std::fs::write(&path, scenario).unwrap();
    path
}

fn launches(model: &Path) -> Vec<String> {
    let mut log = model.as_os_str().to_owned();
    log.push(".args");
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn runtime(idle: Duration, wait: Duration) -> local::Runtime {
    local::Runtime::with_times(fake().clone(), false, idle, wait)
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

fn port_of(endpoint: &ai::Endpoint) -> u16 {
    url::Url::parse(&endpoint.base_url).unwrap().port().unwrap()
}

fn closed(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(200),
    )
    .is_err()
}

fn meta() -> Meta {
    Meta {
        title: Some("Some Video".into()),
        ..Meta::default()
    }
}

#[test]
fn starts_on_demand_asks_with_its_key_and_is_reused() {
    let rt = runtime(local::IDLE, Duration::from_secs(20));
    let m = model("reuse", "ok");
    assert_eq!(rt.pid(), None, "nothing runs before a row is named");
    let endpoint = rt.endpoint(&m, "m.gguf").unwrap();
    assert!(endpoint.base_url.starts_with("http://127.0.0.1:"));
    assert!(endpoint.base_url.ends_with("/v1"));
    assert_eq!(endpoint.model, "m.gguf");
    assert_eq!(ai::ask(&endpoint, &meta()), Ok("Fake Local Name".into()));
    let without_key = ai::Endpoint {
        key: None,
        ..endpoint.clone()
    };
    assert_eq!(
        ai::ask(&without_key, &meta()),
        Err("The naming service answered with HTTP 401.".into()),
        "only Queue, with this launch's key, can use the server"
    );
    let pid = rt.pid().unwrap();
    let again = rt.endpoint(&m, "m.gguf").unwrap();
    assert_eq!(rt.pid(), Some(pid), "a running server is reused");
    assert_eq!(again.key, endpoint.key);
    assert_eq!(launches(&m).len(), 1);
    let args = &launches(&m)[0];
    assert!(args.contains("--host\t127.0.0.1"), "{args}");
    assert!(
        !args.contains(endpoint.key.as_deref().unwrap()),
        "the key isn't an argument"
    );
    rt.stop();
    assert!(gone(pid));
    assert!(closed(port_of(&endpoint)));
}

#[test]
fn a_new_launch_gets_a_new_key() {
    let rt = runtime(local::IDLE, Duration::from_secs(20));
    let m = model("keys", "ok");
    let first = rt.endpoint(&m, "m").unwrap();
    rt.stop();
    let second = rt.endpoint(&m, "m").unwrap();
    assert_ne!(first.key, second.key);
    assert!(
        ai::ask(&first, &meta()).is_err(),
        "the old key no longer works"
    );
    rt.stop();
}

#[test]
fn stops_after_the_idle_time_and_starts_again_when_needed() {
    let rt = runtime(Duration::from_millis(300), Duration::from_secs(20));
    let m = model("idle", "ok");
    rt.endpoint(&m, "m").unwrap();
    let pid = rt.pid().unwrap();
    assert!(!rt.stop_if_idle(), "not idle yet");
    std::thread::sleep(Duration::from_millis(400));
    assert!(rt.stop_if_idle());
    assert!(gone(pid));
    assert_eq!(rt.pid(), None);
    rt.endpoint(&m, "m").unwrap();
    assert_ne!(rt.pid(), Some(pid));
    assert_eq!(launches(&m).len(), 2);
    rt.stop();
}

#[test]
fn using_it_resets_the_idle_time() {
    let rt = runtime(Duration::from_millis(500), Duration::from_secs(20));
    let m = model("touch", "ok");
    rt.endpoint(&m, "m").unwrap();
    let pid = rt.pid().unwrap();
    for _ in 0..4 {
        std::thread::sleep(Duration::from_millis(200));
        rt.endpoint(&m, "m").unwrap();
        assert!(!rt.stop_if_idle());
    }
    assert_eq!(rt.pid(), Some(pid));
    rt.stop();
}

#[test]
fn switching_models_stops_the_old_server() {
    let rt = runtime(local::IDLE, Duration::from_secs(20));
    let (a, b) = (model("switch-a", "ok"), model("switch-b", "ok"));
    let old = rt.endpoint(&a, "a").unwrap();
    let old_pid = rt.pid().unwrap();
    rt.endpoint(&b, "b").unwrap();
    assert!(gone(old_pid));
    assert!(closed(port_of(&old)));
    assert_ne!(rt.pid(), Some(old_pid));
    rt.stop();
}

#[test]
fn a_server_that_died_is_started_again() {
    let rt = runtime(local::IDLE, Duration::from_secs(20));
    let m = model("died", "ok");
    rt.endpoint(&m, "m").unwrap();
    let pid = rt.pid().unwrap();
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
    #[cfg(windows)]
    Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .output()
        .unwrap();
    // Our own unreaped child until the runtime notices; give the kill a moment to land.
    std::thread::sleep(Duration::from_millis(300));
    let endpoint = rt.endpoint(&m, "m").unwrap();
    assert!(gone(pid), "reaped once the runtime saw it had exited");
    assert_eq!(ai::ask(&endpoint, &meta()), Ok("Fake Local Name".into()));
    assert_eq!(launches(&m).len(), 2);
    rt.stop();
}

#[test]
fn a_server_that_exits_is_tried_twice_then_reported() {
    let rt = runtime(local::IDLE, Duration::from_secs(20));
    let m = model("exits", "exit");
    assert_eq!(
        rt.endpoint(&m, "m").err().as_deref(),
        Some("The local model couldn't start.")
    );
    assert_eq!(launches(&m).len(), 2, "one more try, on another port");
    assert_eq!(rt.pid(), None);
}

#[test]
fn a_server_that_never_gets_healthy_is_stopped() {
    let rt = runtime(local::IDLE, Duration::from_millis(600));
    let m = model("slow", "slow");
    let started = Instant::now();
    assert_eq!(
        rt.endpoint(&m, "m").err().as_deref(),
        Some("The local model took too long to load.")
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(rt.pid(), None);
    assert_eq!(launches(&m).len(), 1);
}

#[test]
fn stop_cuts_a_start_short() {
    let rt = std::sync::Arc::new(runtime(local::IDLE, Duration::from_secs(60)));
    let m = model("cut", "slow");
    let starting = {
        let (rt, m) = (rt.clone(), m.clone());
        std::thread::spawn(move || {
            let started = Instant::now();
            (rt.endpoint(&m, "m"), started.elapsed())
        })
    };
    while launches(&m).is_empty() {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(200));
    let stopping = Instant::now();
    rt.stop();
    assert!(
        stopping.elapsed() < Duration::from_secs(5),
        "stop doesn't wait out the load"
    );
    let (result, took) = starting.join().unwrap();
    assert_eq!(
        result.err().as_deref(),
        Some("The local model was stopped.")
    );
    assert!(took < Duration::from_secs(10));
    assert_eq!(rt.pid(), None);
}

#[test]
fn starting_namer_reports_a_failed_start_as_a_failed_ask() {
    let failed = ai::Starting(|| ai::Ready::Failed("The local model couldn't start.".into()));
    assert_eq!(
        ai::Namer::name_if(&failed, &meta(), &|| true),
        Some(Err("The local model couldn't start.".into()))
    );
    let nothing = ai::Starting(|| ai::Ready::Nothing);
    assert_eq!(ai::Namer::name_if(&nothing, &meta(), &|| true), None);
    let started = std::cell::Cell::new(false);
    let stale = ai::Starting(|| {
        started.set(true);
        ai::Ready::Nothing
    });
    assert_eq!(ai::Namer::name_if(&stale, &meta(), &|| false), None);
    assert!(!started.get(), "a stale job doesn't start the server");
}

#[test]
fn a_start_that_is_no_longer_wanted_launches_nothing() {
    let rt = runtime(local::IDLE, Duration::from_secs(20));
    let m = model("unwanted", "ok");
    assert_eq!(
        rt.endpoint_if(&m, "m", &|| false).err().as_deref(),
        Some(local::NOT_IN_USE)
    );
    assert!(launches(&m).is_empty(), "no server started");
    assert_eq!(rt.pid(), None);
}

/// #30(b) review: a job that read the settings before a switch never starts the old model. A
/// start already loading is cut short; one waiting for the change sees it.
#[test]
fn a_switch_cuts_a_loading_start_and_stops_later_ones() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let rt = std::sync::Arc::new(runtime(local::IDLE, Duration::from_secs(60)));
    let chosen = std::sync::Arc::new(AtomicBool::new(true));
    let m = model("switched", "slow");
    let loading = {
        let (rt, m, chosen) = (rt.clone(), m.clone(), chosen.clone());
        std::thread::spawn(move || rt.endpoint_if(&m, "m", &|| chosen.load(Ordering::SeqCst)))
    };
    while launches(&m).is_empty() {
        std::thread::sleep(Duration::from_millis(20));
    }
    // The change runs under the runtime's lock, so it can't call back into the runtime.
    rt.stop_while(|| chosen.store(false, Ordering::SeqCst));
    assert_eq!(
        loading.join().unwrap().err().as_deref(),
        Some("The local model was stopped.")
    );
    assert_eq!(
        rt.endpoint_if(&m, "m", &|| chosen.load(Ordering::SeqCst))
            .err()
            .as_deref(),
        Some(local::NOT_IN_USE),
        "a job that read the old settings starts nothing"
    );
    assert_eq!(launches(&m).len(), 1);
}

/// #30(b) review: Delete stops the server before removing the file, and no start can slip in
/// between: a start after it finds the model gone.
#[test]
fn delete_runs_with_no_server_and_none_starts_after() {
    let rt = runtime(local::IDLE, Duration::from_secs(20));
    let m = model("deleted", "ok");
    rt.endpoint(&m, "m").unwrap();
    let pid = rt.pid().unwrap();
    rt.stop_while(|| {
        assert!(gone(pid), "stopped before the file is removed");
        std::fs::remove_file(&m).unwrap();
    });
    assert_eq!(
        rt.endpoint_if(&m, "m", &|| m.exists()).err().as_deref(),
        Some(local::NOT_IN_USE)
    );
    assert_eq!(launches(&m).len(), 1);
    assert_eq!(rt.pid(), None);
}

/// #30(b) review round 2: a change that has begun but doesn't hold the lock yet still keeps a
/// new start from beginning, and cuts short one that's loading.
#[test]
fn a_change_under_way_blocks_new_starts_and_cuts_loading_ones() {
    let rt = std::sync::Arc::new(runtime(local::IDLE, Duration::from_secs(60)));
    let ok = model("pending", "ok");
    let held = rt.change();
    assert_eq!(
        rt.endpoint_if(&ok, "m", &|| true).err().as_deref(),
        Some(local::NOT_IN_USE)
    );
    assert!(
        launches(&ok).is_empty(),
        "nothing starts while a change is under way"
    );
    drop(held);
    rt.endpoint(&ok, "m").unwrap();
    rt.stop();

    let slow = model("pending-slow", "slow");
    let loading = {
        let (rt, slow) = (rt.clone(), slow.clone());
        std::thread::spawn(move || rt.endpoint(&slow, "m"))
    };
    while launches(&slow).is_empty() {
        std::thread::sleep(Duration::from_millis(20));
    }
    let held = rt.change();
    let started = Instant::now();
    assert_eq!(
        loading.join().unwrap().err().as_deref(),
        Some("The local model was stopped.")
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    drop(held);
    assert_eq!(rt.pid(), None);
}
