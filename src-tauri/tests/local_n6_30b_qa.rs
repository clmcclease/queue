//! Independent N6/#30(b): real subprocess lifecycle plus the production naming-job path.
#[path = "support/native.rs"]
mod native;

use queue_lib::{
    ai, download, local,
    meta::Meta,
    queue::{NameSource, Queue},
    run_naming_job,
};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Barrier, Mutex, OnceLock};
use std::time::{Duration, Instant};

static FIXTURE: OnceLock<PathBuf> = OnceLock::new();
static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn fixture() -> &'static PathBuf {
    FIXTURE.get_or_init(|| {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/qa-local-30b-independent");
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(format!(
            "fixture-{}{}",
            std::process::id(),
            std::env::consts::EXE_SUFFIX
        ));
        let result = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args(["--edition=2024", "--crate-name", "local_fixture_n6_30_qa"])
            .arg(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/support/local_fixture_n6_30_qa.rs"),
            )
            .arg("-o")
            .arg(&exe)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        exe
    })
}
struct Case {
    dir: PathBuf,
    model: PathBuf,
    rt: Arc<local::Runtime>,
}
impl Case {
    fn new(scenario: &str, wait: Duration) -> Self {
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("queue-N6-30-qa-{}-{serial}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Legal on Windows too, with shell punctuation that must remain literal in argv.
        let model = dir.join("日本語 Café & $(literal) ' model.gguf");
        std::fs::write(&model, scenario).unwrap();
        let exe = dir.join(format!(
            "server 日本語 & spaces{}",
            std::env::consts::EXE_SUFFIX
        ));
        std::fs::copy(fixture(), &exe).unwrap();
        let rt = Arc::new(local::Runtime::with_times(exe, false, local::IDLE, wait));
        Self { dir, model, rt }
    }
    fn log(&self, suffix: &str) -> String {
        let mut path = self.model.as_os_str().to_owned();
        path.push(suffix);
        std::fs::read_to_string(path).unwrap_or_default()
    }
}
impl Drop for Case {
    fn drop(&mut self) {
        self.rt.stop();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn until(mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(8);
    while !f() {
        assert!(Instant::now() < end, "fixture did not reach expected state");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn closed(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(200))
        .is_err()
}
fn queued(title: &str) -> (Mutex<Queue>, u64) {
    let mut q = Queue::default();
    let id = q.add("https://example.test/video").unwrap().id;
    q.fetched(
        id,
        Ok(Meta {
            title: Some(title.into()),
            ..Meta::default()
        }),
    )
    .unwrap();
    (Mutex::new(q), id)
}
fn starting(case: &Case) -> ai::Starting<impl Fn() -> ai::Ready + '_> {
    ai::Starting(|| match case.rt.endpoint(&case.model, "model") {
        Ok(ep) => ai::Ready::Ask(ep),
        Err(error) => ai::Ready::Failed(error),
    })
}

#[test]
fn unicode_model_and_executable_paths_are_spawned_literally_and_receive_unicode_metadata() {
    let case = Case::new(
        "answer\n{\"filename\":\"Café 日本語\"}",
        Duration::from_secs(8),
    );
    let (q, id) = queued("🔥 Café 日本語 [4K]");
    let item = run_naming_job(&q, id, &starting(&case)).unwrap();
    assert_eq!(item.name.as_deref(), Some("Café 日本語"));
    assert_eq!(item.name_source, Some(NameSource::Ai));
    assert!(case.log(".requests").contains("日本語"));
    assert_eq!(case.log(".launches").lines().count(), 1);
}

#[test]
fn simultaneous_first_requests_share_one_process_and_key() {
    let case = Case::new("health-retry", Duration::from_secs(8));
    let barrier = Arc::new(Barrier::new(5));
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let (rt, model, barrier) = (case.rt.clone(), case.model.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                rt.endpoint(&model, "m").unwrap()
            })
        })
        .collect();
    barrier.wait();
    let endpoints: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for ep in &endpoints {
        assert_eq!(ep.base_url, endpoints[0].base_url);
        assert_eq!(ep.key, endpoints[0].key);
    }
    assert_eq!(case.log(".launches").lines().count(), 1);
    assert!(
        case.log(".requests")
            .lines()
            .filter(|s| s.starts_with("/health"))
            .count()
            >= 4
    );
}

#[test]
fn stalled_health_headers_time_out_and_leave_neither_process_nor_port() {
    let case = Case::new("health-stall", Duration::from_millis(300));
    let started = Instant::now();
    let error = case.rt.endpoint(&case.model, "m").err().unwrap();
    assert_eq!(error, "The local model took too long to load.");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(case.rt.pid(), None);
    let pid = case.log(".launches").trim().parse().unwrap();
    until(|| native::process_has_exited(pid));
    assert!(closed(case.log(".port").trim().parse().unwrap()));
}

#[test]
fn stop_interrupts_a_health_request_that_never_sends_headers() {
    let case = Case::new("health-stall", Duration::from_secs(60));
    let (rt, model) = (case.rt.clone(), case.model.clone());
    let start = std::thread::spawn(move || rt.endpoint(&model, "m"));
    until(|| case.log(".requests").contains("/health"));
    let when = Instant::now();
    case.rt.stop();
    assert!(when.elapsed() < Duration::from_secs(5));
    assert_eq!(
        start.join().unwrap().err().as_deref(),
        Some("The local model was stopped.")
    );
    assert!(closed(case.log(".port").trim().parse().unwrap()));
}

#[test]
fn dropping_the_runtime_stops_its_process_even_without_an_explicit_stop() {
    let case = Case::new("ok", Duration::from_secs(8));
    let rt = local::Runtime::new(fixture().clone(), false);
    let ep = rt.endpoint(&case.model, "m").unwrap();
    let pid = rt.pid().unwrap();
    let port = url::Url::parse(&ep.base_url).unwrap().port().unwrap();
    drop(rt);
    until(|| native::process_has_exited(pid));
    assert!(closed(port));
}

#[test]
fn invalid_local_answers_keep_the_tier_one_name_and_fallback_tag() {
    for content in [
        "null".to_owned(),
        "{bad JSON".into(),
        r#"{"filename":""}"#.into(),
        r#"{"filename":".mp4"}"#.into(),
        r#"{"filename":"Translated title"}"#.into(),
        serde_json::json!({"filename":"日".repeat(151)}).to_string(),
    ] {
        let case = Case::new(&format!("answer\n{content}"), Duration::from_secs(8));
        let (q, id) = queued("🔥 日本語 [4K]");
        let item = run_naming_job(&q, id, &starting(&case)).unwrap();
        assert_eq!(item.name.as_deref(), Some("日本語"), "{content}");
        assert_eq!(item.name_source, Some(NameSource::Fallback), "{content}");
    }
}

#[test]
fn reserved_and_messy_local_answers_are_sanitized_and_collisions_preserve_both_files() {
    for (raw, expected) in [
        ("NUL.mp4", "NUL_"),
        ("CON.txt", "CON_.txt"),
        ("LPT³ .wav", "LPT³_"),
        (
            " .. Café\t日本語/α\\β:*?\u{202e} .MP4 ",
            "Café 日本語⧸α⧹β：＊？",
        ),
    ] {
        let content = serde_json::json!({"filename":raw});
        let case = Case::new(&format!("answer\n{content}"), Duration::from_secs(8));
        let (q, id) = queued("Title");
        let item = run_naming_job(&q, id, &starting(&case)).unwrap();
        assert_eq!(item.name.as_deref(), Some(expected));
        assert_eq!(item.name_source, Some(NameSource::Ai));
        let out = case.dir.join("out");
        std::fs::create_dir(&out).unwrap();
        let mut paths = Vec::new();
        for i in 0..2 {
            let source = case.dir.join(format!("source{i}"));
            std::fs::write(&source, format!("payload{i}")).unwrap();
            paths.push(
                download::place_unique(&source, &out, OsStr::new(&format!("{expected}.mp4")))
                    .unwrap(),
            );
        }
        assert_eq!(
            paths[1].file_name().unwrap(),
            OsStr::new(&format!("{expected} (2).mp4"))
        );
        assert_eq!(std::fs::read_to_string(&paths[0]).unwrap(), "payload0");
        assert_eq!(std::fs::read_to_string(&paths[1]).unwrap(), "payload1");
    }
}

#[test]
fn a_failed_local_start_tags_only_that_row_and_the_next_job_can_succeed() {
    let case = Case::new("health-stall", Duration::from_millis(200));
    let (q, id) = queued("🔥 original [4K]");
    let item = run_naming_job(&q, id, &starting(&case)).unwrap();
    assert_eq!(item.name.as_deref(), Some("original"));
    assert_eq!(item.name_source, Some(NameSource::Fallback));
    std::fs::write(&case.model, "ok").unwrap();
    let item = run_naming_job(&q, id, &starting(&case)).unwrap();
    assert_eq!(item.name.as_deref(), Some("Local name"));
    assert_eq!(item.name_source, Some(NameSource::Ai));
}

#[test]
fn a_stalled_completion_uses_the_ten_second_budget_and_keeps_a_safe_rules_name() {
    let case = Case::new("chat-stall", Duration::from_secs(8));
    let (q, id) = queued("CON");
    let start = Instant::now();
    let item = run_naming_job(&q, id, &starting(&case)).unwrap();
    assert!(start.elapsed() >= ai::TIMEOUT);
    assert!(start.elapsed() < Duration::from_secs(15));
    assert_eq!(item.name.as_deref(), Some("CON_"));
    assert_eq!(item.name_source, Some(NameSource::Fallback));
}

#[test]
fn a_download_can_start_while_the_model_loads_and_no_late_name_is_sent() {
    let case = Arc::new(Case::new("health-retry", Duration::from_secs(8)));
    let (queue, id) = queued("Original");
    let queue = Arc::new(queue);
    let job = {
        let (case, queue) = (case.clone(), queue.clone());
        std::thread::spawn(move || run_naming_job(&queue, id, &starting(&case)))
    };
    until(|| case.log(".requests").contains("/health"));
    queue.lock().unwrap().start(id).unwrap();
    assert!(job.join().unwrap().is_none());
    assert!(!case.log(".requests").contains("/v1/chat/completions"));
    let q = queue.lock().unwrap();
    let row = q.items().iter().find(|i| i.id == id).unwrap();
    assert_eq!(row.name.as_deref(), Some("Original"));
}

#[cfg(unix)]
#[test]
fn non_utf8_model_paths_remain_native_arguments() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let path = PathBuf::from(std::ffi::OsString::from_vec(b"/models/\xff.gguf".to_vec()));
    let args = local::args(Path::new(&path), 4321, false);
    assert_eq!(args[1].as_bytes(), b"/models/\xff.gguf");
}

#[test]
fn a_provider_generation_change_during_a_failed_start_does_not_retag_the_row() {
    let case = Arc::new(Case::new("health-stall", Duration::from_secs(60)));
    let (queue, id) = queued("Original");
    let queue = Arc::new(queue);
    let job = {
        let (case, queue) = (case.clone(), queue.clone());
        std::thread::spawn(move || run_naming_job(&queue, id, &starting(&case)))
    };
    until(|| case.log(".requests").contains("/health"));
    assert_eq!(queue.lock().unwrap().renaming(), [id]);
    case.rt.stop();
    assert!(job.join().unwrap().is_none());
    let q = queue.lock().unwrap();
    let row = q.items().iter().find(|i| i.id == id).unwrap();
    assert_eq!(row.name.as_deref(), Some("Original"));
    assert_eq!(row.name_source, Some(NameSource::Rules));
}

#[test]
fn quit_stops_both_a_loaded_server_and_one_waiting_for_health() {
    // proc::shut_down is irreversible: isolate it from the rest of this test binary.
    const CHILD: &str = "QUEUE_QA_N6_30_QUIT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let out = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "quit_stops_both_a_loaded_server_and_one_waiting_for_health",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        return;
    }
    let loaded = Case::new("ok", Duration::from_secs(8));
    let ep = loaded.rt.endpoint(&loaded.model, "m").unwrap();
    let pid = loaded.rt.pid().unwrap();
    let loading = Case::new("health-stall", Duration::from_secs(60));
    let (rt, model) = (loading.rt.clone(), loading.model.clone());
    let job = std::thread::spawn(move || rt.endpoint(&model, "m"));
    until(|| loading.log(".requests").contains("/health"));
    let since = Instant::now();
    queue_lib::proc::shut_down();
    loaded.rt.stop();
    loading.rt.stop();
    assert_eq!(
        job.join().unwrap().err().as_deref(),
        Some("The local model was stopped.")
    );
    assert!(queue_lib::proc::wait_idle(Duration::from_secs(5)));
    assert!(since.elapsed() < Duration::from_secs(5));
    until(|| native::process_has_exited(pid));
    assert!(closed(
        url::Url::parse(&ep.base_url).unwrap().port().unwrap()
    ));
    assert!(closed(loading.log(".port").trim().parse().unwrap()));
    assert!(
        loaded.rt.endpoint(&loaded.model, "m").is_err(),
        "no launch after quit"
    );
}
