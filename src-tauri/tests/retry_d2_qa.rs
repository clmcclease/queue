//! Independent QA for #48 / D2 / D-017. Fake subprocesses do not contact external sites.
//! The two failing regression tests express issue #48's expected behavior; do not fix the
//! production code here. Queue/cache tests also run on Windows; shell fixtures are Unix-only.
use queue_lib::{meta::Meta, queue::Queue, queue::Status};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "queue-48-qa-日本語 & spaces-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn ready(queue: &mut Queue) -> u64 {
    let id = queue
        .add("https://www.youtube.com/watch?v=jNQXAC9IVRw")
        .unwrap()
        .id;
    queue.fetched(id, Ok(Meta::default())).unwrap();
    id
}

#[test]
fn cache_freshness_boundaries_and_non_file_fallback() {
    let temp = Temp::new();
    let path = temp.0.join("info.json");
    fs::write(&path, b"{}").unwrap();
    let mut queue = Queue::default();
    let id = ready(&mut queue);
    let fetched_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    queue.set_info(id, path.clone(), fetched_at);
    let cutoff = queue_lib::queue::INFO_FRESH_FOR;
    // The smallest step every OS's clock can represent: Windows' SystemTime counts 100 ns
    // units, so "1 ns before the cutoff" would round to the cutoff itself.
    let tick = Duration::from_micros(1);
    assert_eq!(queue.fresh_info(id, fetched_at), Some(path.clone()));
    assert_eq!(
        queue.fresh_info(id, fetched_at + cutoff - tick),
        Some(path.clone())
    );
    assert_eq!(queue.fresh_info(id, fetched_at + cutoff), None);
    assert_eq!(queue.fresh_info(id, fetched_at - tick), None);
    assert_eq!(queue.fresh_info(999, fetched_at), None);
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert_eq!(
        queue.fresh_info(id, fetched_at),
        None,
        "a directory is not info JSON"
    );
    assert_eq!(queue.take_info(id), Some(path));
    assert_eq!(queue.take_info(id), None);
}

#[test]
fn saved_media_links_and_cache_paths_never_appear_in_serialized_items() {
    let temp = Temp::new();
    let path = temp.0.join("private-info.json");
    fs::write(&path, br#"{"url":"https://cdn.test/?token=secret"}"#).unwrap();
    let mut queue = Queue::default();
    let id = ready(&mut queue);
    queue.set_info(id, path.clone(), SystemTime::now());
    let item = queue.start(id).unwrap();
    let value = serde_json::to_value(item).unwrap();
    assert_eq!(value["attempt"], 1);
    assert!(value.get("info").is_none());
    assert!(value.get("key").is_none());
    let json = value.to_string();
    assert!(!json.contains("private-info") && !json.contains("token=secret"));
    assert_eq!(queue.take_info(id), Some(path));
}

#[test]
fn retry_revisions_increase_and_final_failure_unlocks_the_next_item() {
    let mut queue = Queue::default();
    let id = ready(&mut queue);
    let next = queue.add("https://example.test/next").unwrap().id;
    queue.fetched(next, Ok(Meta::default())).unwrap();
    let started = queue.start(id).unwrap();
    for attempt in [2, 3] {
        let retry = queue.retrying(id, attempt).unwrap();
        assert_eq!(retry.status, Status::Downloading);
        assert_eq!(retry.attempt, attempt);
        assert_eq!(retry.rev, started.rev + u64::from(attempt - 1));
        assert!(
            queue.start(next).is_err(),
            "backoff still holds concurrency slot"
        );
    }
    let failed = queue
        .finish(id, Err("HTTP Error 403: Forbidden".into()))
        .unwrap();
    assert_eq!(failed.attempt, 0);
    assert_eq!(failed.rev, started.rev + 3);
    assert_eq!(
        queue.retrying(id, 2),
        None,
        "late retry cannot resurrect failure"
    );
    assert_eq!(queue.retrying(999, 2), None);
    assert_eq!(queue.start(next).unwrap().attempt, 1);
}

#[cfg(unix)]
mod subprocess {
    use super::*;
    use queue_lib::{download, meta, sidecar::Sidecar};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::Instant;

    const FORBIDDEN: &str =
        "echo 'ERROR: unable to download video data: HTTP Error 403: Forbidden' >&2; exit 1";

    struct Fake {
        temp: Temp,
        out: PathBuf,
        work: PathBuf,
    }

    impl Fake {
        fn new(body: &str) -> Self {
            let temp = Temp::new();
            let out = temp.0.join("out");
            let work = temp.0.join("work");
            fs::create_dir(&out).unwrap();
            let path = Sidecar::YtDlp.path_in(&temp.0);
            let script = format!(
                "#!/bin/sh\nLOG='{}'\nP=; prev=\nfor a in \"$@\"; do [ \"$prev\" = -P ] && P=$a; prev=$a; done\nN=$(( $(cat \"$LOG/count\" 2>/dev/null || echo 0) + 1 ))\necho $N > \"$LOG/count\"\nprintf '%s\\n' \"$@\" > \"$LOG/args-$N\"\n{body}\n",
                temp.0.display()
            );
            fs::write(&path, script).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self { temp, out, work }
        }

        fn run(&self, info: Option<&Path>) -> (Result<String, String>, Vec<String>) {
            let events = std::cell::RefCell::new(Vec::new());
            let result = download::run_with_retries(
                &self.temp.0,
                &self.work,
                &self.out,
                "https://www.youtube.com/watch?v=jNQXAC9IVRw",
                info,
                |duration| {
                    events
                        .borrow_mut()
                        .push(format!("wait {}", duration.as_secs()))
                },
                |attempt| events.borrow_mut().push(format!("retry {attempt}")),
            );
            assert!(
                !self.work.exists(),
                "staging removed after success or failure"
            );
            (result, events.into_inner())
        }

        fn args(&self, attempt: u32) -> Vec<String> {
            fs::read_to_string(self.temp.0.join(format!("args-{attempt}")))
                .unwrap()
                .lines()
                .map(str::to_owned)
                .collect()
        }
    }

    #[test]
    fn two_403s_then_success_reports_backoff_in_order_and_uses_urls_for_both_retries() {
        let fake = Fake::new(&format!(
            "if [ $N -lt 3 ]; then printf partial > \"$P/Title.part\"; {FORBIDDEN}; fi\nprintf media > \"$P/日本語 é.webm\""
        ));
        let info = fake.temp.0.join("info $() ; 雪.json");
        fs::write(&info, b"{}").unwrap();
        let (result, events) = fake.run(Some(&info));
        assert_eq!(fs::read(result.unwrap()).unwrap(), b"media");
        assert_eq!(events, ["retry 2", "wait 10", "retry 3", "wait 30"]);
        let first = fake.args(1);
        let at = first.iter().position(|a| a == "--load-info-json").unwrap();
        assert_eq!(
            first[at + 1],
            info.to_str().unwrap(),
            "path is one literal argument"
        );
        for attempt in [2, 3] {
            let args = fake.args(attempt);
            assert!(!args.iter().any(|a| a == "--load-info-json"));
            assert_eq!(
                &args[args.len() - 2..],
                ["--", "https://www.youtube.com/watch?v=jNQXAC9IVRw"]
            );
        }
        assert!(!fake.temp.0.join("args-4").exists());
    }

    #[test]
    fn a_non_403_after_the_first_retry_stops_and_returns_the_latest_reason() {
        let fake = Fake::new(&format!(
            "printf partial > \"$P/Title.part\"\nif [ $N -eq 1 ]; then {FORBIDDEN}; fi\necho 'ERROR: HTTP Error 429: Too Many Requests' >&2; exit 1"
        ));
        let (result, events) = fake.run(None);
        assert_eq!(result.unwrap_err(), "HTTP Error 429: Too Many Requests");
        assert_eq!(events, ["retry 2", "wait 10"]);
        assert!(!fake.temp.0.join("args-3").exists());
        assert_eq!(fs::read_dir(&fake.out).unwrap().count(), 0);
    }

    #[test]
    fn successful_exit_ignores_403_warnings_and_malformed_progress_output() {
        let fake = Fake::new(
            "echo 'WARNING: HTTP Error 403 on an optional request' >&2\nprintf '[download] NaN%% of Unknown ETA ???\\n{bad json\\n'\nprintf media > \"$P/Title.webm\"",
        );
        let (result, events) = fake.run(None);
        assert!(result.is_ok());
        assert!(
            events.is_empty(),
            "success must not be retried because of stderr noise"
        );
    }

    #[test]
    fn success_without_a_finished_file_is_not_retried() {
        let fake = Fake::new("exit 0");
        let (result, events) = fake.run(None);
        assert!(result.unwrap_err().contains("without saving a file"));
        assert!(events.is_empty());
    }

    #[test]
    fn collision_on_a_retry_keeps_existing_bytes_and_stops() {
        let fake = Fake::new(&format!(
            "if [ $N -eq 1 ]; then {FORBIDDEN}; fi\nprintf new > \"$P/Café.webm\""
        ));
        fs::write(fake.out.join("Café.webm"), b"original").unwrap();
        let (result, events) = fake.run(None);
        assert!(result.unwrap().ends_with("Café (2).webm"), "N2: keep both");
        assert_eq!(events, ["retry 2", "wait 10"]);
        assert_eq!(fs::read(fake.out.join("Café.webm")).unwrap(), b"original");
        assert!(!fake.temp.0.join("args-3").exists());
    }

    #[test]
    fn local_filename_collision_containing_http_error_403_is_not_a_network_retry() {
        let fake = Fake::new("printf new > \"$P/HTTP Error 403.webm\"");
        fs::write(fake.out.join("HTTP Error 403.webm"), b"original").unwrap();
        let (result, events) = fake.run(None);
        assert!(
            result.unwrap().ends_with("HTTP Error 403 (2).webm"),
            "N2: keep both"
        );
        assert_eq!(
            fs::read(fake.out.join("HTTP Error 403.webm")).unwrap(),
            b"original"
        );
        assert!(
            events.is_empty(),
            "local collision is not HTTP 403: {events:?}"
        );
        assert!(!fake.temp.0.join("args-2").exists());
    }

    #[test]
    fn completed_media_parts_are_reused_after_a_403_as_required_by_issue_48() {
        let fake = Fake::new(&format!(
            "if [ $N -eq 1 ]; then printf completed-video > \"$P/Title.f137.mp4\"; {FORBIDDEN}; fi\nif [ ! -f \"$P/Title.f137.mp4\" ]; then echo 'ERROR: completed video part was discarded' >&2; exit 1; fi\nmv \"$P/Title.f137.mp4\" \"$P/Title.mp4\""
        ));
        let (result, events) = fake.run(None);
        assert_eq!(events, ["retry 2", "wait 10"]);
        assert!(
            result.is_ok(),
            "completed stream must survive retry: {result:?}"
        );
        assert_eq!(fs::read(result.unwrap()).unwrap(), b"completed-video");
    }

    #[test]
    fn missing_sidecar_is_not_retried() {
        let fake = Fake::new("exit 0");
        fs::remove_file(Sidecar::YtDlp.path_in(&fake.temp.0)).unwrap();
        let (result, events) = fake.run(None);
        assert!(result.unwrap_err().contains("Couldn't start yt-dlp"));
        assert!(events.is_empty());
    }

    #[test]
    fn stale_or_missing_info_falls_back_to_url_through_the_subprocess_path() {
        let fake = Fake::new("printf media > \"$P/Title.webm\"");
        let file = fake.temp.0.join("info.json");
        fs::write(&file, b"{}").unwrap();
        let mut queue = Queue::default();
        let id = ready(&mut queue);
        let fetched_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        queue.set_info(id, file.clone(), fetched_at);
        let stale = queue.fresh_info(id, fetched_at + Duration::from_secs(6 * 3600));
        assert!(stale.is_none());
        assert!(fake.run(stale.as_deref()).0.is_ok());
        fs::remove_file(fake.out.join("Title.webm")).unwrap();
        fs::remove_file(&file).unwrap();
        let missing = queue.fresh_info(id, fetched_at);
        assert!(missing.is_none());
        assert!(fake.run(missing.as_deref()).0.is_ok());
        for attempt in [1, 2] {
            let args = fake.args(attempt);
            assert!(!args.iter().any(|a| a == "--load-info-json"));
            assert_eq!(args[args.len() - 2], "--");
        }
    }

    #[test]
    fn raw_info_keeps_unicode_and_format_fields_byte_exact_for_later_downloads() {
        let raw = br#" {"title":"  CON : \u65e5\u672c\u8a9e \ud83c\udfb5 (OFFICIAL VIDEO) ","formats":[{"url":"https://cdn.test/?expire=42&sig=%FF","format_id":"137"}],"extra":{"private_token":"secret"}} "#;
        let fake = Fake::new("cat \"$LOG/raw.json\"");
        fs::write(fake.temp.0.join("raw.json"), raw).unwrap();
        let (parsed, returned) = meta::fetch_raw(
            &fake.temp.0,
            "https://example.test/v",
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(returned, raw);
        assert_eq!(
            parsed.title.as_deref(),
            Some("CON : 日本語 🎵 (OFFICIAL VIDEO)")
        );
        let shown = serde_json::to_string(&parsed).unwrap();
        assert!(!shown.contains("private_token") && !shown.contains("sig=%FF"));
    }

    #[test]
    fn raw_info_timeout_returns_an_error_instead_of_partial_json() {
        let fake = Fake::new("printf '{\"title\":\"partial\"}'; sleep 2");
        let started = Instant::now();
        let error = meta::fetch_raw(
            &fake.temp.0,
            "https://example.test/v",
            Duration::from_millis(75),
        )
        .unwrap_err();
        assert!(error.contains("took longer"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
