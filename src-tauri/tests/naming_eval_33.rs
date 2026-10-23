//! #33 / N9: the naming eval set (`naming-eval/titles.json`) and its runner.
//!
//! The set's shape is checked on every run. Scoring a model runs only when asked:
//!
//! ```sh
//! QUEUE_EVAL_ENDPOINT=http://127.0.0.1:8080/v1 QUEUE_EVAL_MODEL=qwen3-4b \
//!   cargo test --test naming_eval_33 -- --ignored --nocapture
//! ```
//!
//! `QUEUE_EVAL_KEY` adds a key for an API provider. `QUEUE_EVAL_SET` scores another set of the
//! same shape instead, such as the owner's labelled set, which stays outside the repo (D-031).
//! The run fails below 65% accepted, the target restated on the owner's set (PRD §9, D-033); the
//! author-written seed scores lower and is a regression set, not the target. Latency is printed,
//! not asserted (it depends on the hardware).
//!
//! #33(b)'s collector fetches the real metadata of a list of links, for the owner to label. It
//! runs the pinned yt-dlp exactly as the app does (`meta::fetch`), one link at a time, and writes
//! entries shaped like the set's, with `lang`, `expected` and `accept` left empty, to a new file
//! (never `titles.json`, and never over an existing file):
//!
//! ```sh
//! QUEUE_QA_REAL_SIDECARS=$PWD/binaries QUEUE_EVAL_COLLECT=links.txt QUEUE_EVAL_OUT=to-label.json \
//!   cargo test --test naming_eval_33 n9_collect -- --ignored --nocapture
//! ```
//!
//! `links.txt` has one link per line; blank lines and `#` comments are skipped.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use queue_lib::ai::{self, Endpoint};
use queue_lib::meta::{self, Meta};
use queue_lib::naming;
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    lang: String,
    meta: Meta,
    expected: String,
    accept: Vec<String>,
}

fn cases() -> Vec<Case> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/naming-eval/titles.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn n9_the_eval_set_is_well_formed() {
    let cases = cases();
    assert!(cases.len() >= 50, "{} cases", cases.len());
    let other = cases.iter().filter(|c| c.lang != "en").count();
    assert!(
        other * 10 >= cases.len() * 3,
        "at least 30% non-English: {other}/{}",
        cases.len()
    );
    check_labels(&cases);
}

/// Every case has a unique title, and every listed name is one Queue could produce. Run on the
/// seed and on a `QUEUE_EVAL_SET` before any request is made.
fn check_labels(cases: &[Case]) {
    assert!(!cases.is_empty(), "the set is empty");
    let mut titles = std::collections::HashSet::new();
    for case in cases {
        let title = case.meta.title.as_deref().expect("every case has a title");
        assert!(titles.insert(title), "duplicate: {title}");
        for name in std::iter::once(&case.expected).chain(&case.accept) {
            // Every listed name is one Queue could produce: already sanitized, and not rejected
            // by the answer checks (e.g. for dropping the title's script).
            assert_eq!(&naming::sanitize(name), name, "not sanitized: {name}");
            let answer = serde_json::json!({ "filename": name }).to_string();
            assert_eq!(
                ai::parse(&answer, Some(title)).as_ref(),
                Ok(name),
                "{title} → {name}"
            );
        }
    }
}

#[test]
#[ignore = "scores a model; set QUEUE_EVAL_ENDPOINT (and QUEUE_EVAL_MODEL, QUEUE_EVAL_KEY)"]
fn n9_eval_scores_a_model() {
    let Ok(base_url) = std::env::var("QUEUE_EVAL_ENDPOINT") else {
        eprintln!("skipped: QUEUE_EVAL_ENDPOINT isn't set");
        return;
    };
    let endpoint = Endpoint {
        kind: queue_lib::ai::ApiKind::OpenaiCompatible,
        base_url,
        model: std::env::var("QUEUE_EVAL_MODEL").unwrap_or_default(),
        key: std::env::var("QUEUE_EVAL_KEY").ok(),
    };
    let cases: Vec<Case> = match std::env::var_os("QUEUE_EVAL_SET") {
        Some(path) => serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap(),
        None => cases(),
    };
    check_labels(&cases);
    let mut accepted = 0;
    // D-016's measures: how answers fail, and cold (first) vs warm latency.
    let (mut invalid_json, mut fallback, mut errors, mut romanized, mut extension) =
        (0, 0, 0, 0, 0);
    let (mut dropped, mut injection, mut drift) = (0, 0, 0);
    let mut times = Vec::new();
    for case in &cases {
        let title = case.meta.title.as_deref().unwrap_or("");
        let started = Instant::now();
        let raw = ai::ask_content(&endpoint, &case.meta, ai::TIMEOUT);
        times.push(started.elapsed());
        let answer = match &raw {
            Err(e) => {
                errors += 1;
                Err(e.clone())
            }
            Ok(content) => {
                if serde_json::from_str::<serde_json::Value>(content.trim())
                    .ok()
                    .and_then(|v| v["filename"].as_str().map(ai::has_media_extension))
                    .unwrap_or(false)
                {
                    extension += 1;
                }
                ai::parse(content, Some(title)).map_err(|reject| {
                    match reject {
                        ai::Reject::NotJson => invalid_json += 1,
                        ai::Reject::LostScript => romanized += 1,
                        _ => {}
                    }
                    format!("{reject:?}")
                })
            }
        };
        if answer.is_err() {
            fallback += 1; // Queue would keep the Tier-1 name
        }
        let ok = answer
            .as_ref()
            .is_ok_and(|name| *name == case.expected || case.accept.contains(name));
        accepted += usize::from(ok);
        if let (Ok(name), false) = (&answer, ok) {
            let marker = |s: &str| s.contains("feat.") || episode_marker(s);
            if title.contains("Ignore all previous instructions") {
                injection += 1;
            } else if marker(&case.expected) && !marker(name) {
                dropped += 1;
            } else {
                drift += 1;
            }
        }
        let shown = answer.unwrap_or_else(|e| format!("<{e}>"));
        println!(
            "{} [{}] {:?} → {:?} (want {:?})",
            if ok { "✓" } else { "✗" },
            case.lang,
            title,
            shown,
            case.expected
        );
    }
    let cold = times[0];
    let mut warm = times[1.min(times.len() - 1)..].to_vec();
    warm.sort();
    let at = |q: f64| warm[((warm.len() - 1) as f64 * q).round() as usize];
    let n = cases.len();
    let percent = accepted * 100 / n;
    let rate = |k: usize| k * 100 / n;
    println!(
        "accepted {accepted}/{n} ({percent}%), p50 {:?}, p95 {:?} (warm), cold {cold:?}",
        at(0.5),
        at(0.95)
    );
    println!(
        "valid JSON {}%, fallback to Tier 1 {}% ({errors} service errors)",
        100 - rate(invalid_json + errors),
        rate(fallback)
    );
    println!(
        "failures: translated or romanized {romanized} (rejected by design, D-016), added extension {extension}, \
         dropped episode or feat. {dropped}, prompt injection {injection}, style drift {drift}"
    );
    println!("made-up details: manual review of the ✗ lines above (not detected automatically)");
    assert!(accepted * 100 >= n * 65, "below 65% accepted: {percent}%");
    assert!(
        at(0.5) < Duration::from_secs(60),
        "sanity: the median answer took over a minute"
    );
}

/// An episode or show number: `S01E02`, or `#` followed by a digit.
fn episode_marker(name: &str) -> bool {
    let b = name.as_bytes();
    let digit = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
    (0..b.len()).any(|i| {
        (b[i] == b'#' && digit(i + 1))
            || (b[i] == b'S'
                && digit(i + 1)
                && digit(i + 2)
                && b.get(i + 3) == Some(&b'E')
                && digit(i + 4))
    })
}

/// How long one link's metadata may take, as in the app's fetch worker.
const COLLECT_LIMIT: Duration = Duration::from_secs(90);

/// The links in `list`: one per line, without blank lines or `#` comments.
fn links(list: &str) -> Vec<&str> {
    list.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

/// Fetches each link's metadata through the app's own yt-dlp call, one at a time. Returns the
/// entries to label, and the links that failed with yt-dlp's reason.
fn collect(sidecars: &Path, links: &[&str]) -> (Vec<serde_json::Value>, Vec<(String, String)>) {
    let (mut entries, mut failed) = (Vec::new(), Vec::new());
    for link in links {
        match meta::fetch(sidecars, link, COLLECT_LIMIT) {
            Ok(meta) => entries.push(serde_json::json!({
                "url": link,
                "lang": "",
                "meta": meta,
                "expected": "",
                "accept": [],
            })),
            Err(reason) => failed.push(((*link).to_owned(), reason)),
        }
    }
    (entries, failed)
}

/// The pinned sidecars in `binaries/` (named with their target triple) copied into a temporary
/// folder under the names the installed app uses, so `meta::fetch` runs them exactly as the app
/// does (as `download_real_d2.rs` does).
fn app_layout(source: &Path) -> PathBuf {
    use queue_lib::sidecar::Sidecar;
    let triple = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(target_os = "macos") {
        "x86_64-apple-darwin"
    } else if cfg!(target_os = "windows") {
        "x86_64-pc-windows-msvc"
    } else {
        "x86_64-unknown-linux-gnu"
    };
    let bin = std::env::temp_dir().join(format!("queue-33b-tools-{}", std::process::id()));
    std::fs::create_dir_all(&bin).unwrap();
    let exe = std::env::consts::EXE_SUFFIX;
    let mut names: Vec<String> = [Sidecar::YtDlp, Sidecar::Ffmpeg, Sidecar::Deno]
        .iter()
        .map(|s| s.file_name().to_owned())
        .collect();
    // yt-dlp finds ffprobe next to ffmpeg, as in the app (sidecar.rs).
    names.push(format!("queue-ffprobe{exe}"));
    for name in names {
        let stem = name.trim_end_matches(exe);
        std::fs::copy(
            source.join(format!("{stem}-{triple}{exe}")),
            bin.join(&name),
        )
        .unwrap();
    }
    bin
}

/// Whether `out` can take the collected entries: not `titles.json`, nothing there yet (not even
/// a dangling link), and its folder exists. Checked before any link is fetched, so a long
/// collection never ends in a refused write (#33(b) review); `write_new` still creates it with
/// `create_new`, so a file that appears meanwhile is never replaced.
fn destination_ok(out: &Path) -> std::io::Result<()> {
    if out.file_name().is_some_and(|n| n == "titles.json") {
        return Err(std::io::Error::other(
            "the seed set isn't written by the collector",
        ));
    }
    if out.symlink_metadata().is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "the output file already exists",
        ));
    }
    let folder = match out.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    if !folder.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "the output file's folder doesn't exist",
        ));
    }
    Ok(())
}

/// Writes `entries` to `out`, refusing `titles.json` and any file that already exists.
fn write_new(out: &Path, entries: &[serde_json::Value]) -> std::io::Result<()> {
    destination_ok(out)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out)?;
    serde_json::to_writer_pretty(&mut file, entries)?;
    std::io::Write::write_all(&mut file, b"\n")
}

#[test]
#[ignore = "collects real metadata; set QUEUE_EVAL_COLLECT, QUEUE_EVAL_OUT and QUEUE_QA_REAL_SIDECARS"]
fn n9_collect_metadata_for_labelling() {
    // Skipped unless asked for, like the model eval: `--ignored` runs both (#33(b) review).
    let Some(list) = std::env::var_os("QUEUE_EVAL_COLLECT") else {
        eprintln!("skipped: QUEUE_EVAL_COLLECT isn't set");
        return;
    };
    let var = |name: &str| std::env::var_os(name).unwrap_or_else(|| panic!("set {name}"));
    let out = PathBuf::from(var("QUEUE_EVAL_OUT"));
    destination_ok(&out).unwrap();
    let list = std::fs::read_to_string(list).unwrap();
    let sidecars = app_layout(&PathBuf::from(var("QUEUE_QA_REAL_SIDECARS")));
    let links = links(&list);
    let (entries, failed) = collect(&sidecars, &links);
    let _ = std::fs::remove_dir_all(&sidecars);
    write_new(&out, &entries).unwrap();
    println!(
        "collected {} of {} links into {}",
        entries.len(),
        links.len(),
        out.display()
    );
    for (link, reason) in &failed {
        println!("failed: {link}: {reason}");
    }
}

/// The collector against a fake yt-dlp (a shell script, as in `meta_q3.rs`): entries keep the
/// app's metadata shape with empty labels, a failed link is reported and skipped, and the output
/// never replaces a file.
#[cfg(unix)]
#[test]
fn n9_the_collector_writes_entries_to_label() {
    use queue_lib::sidecar::Sidecar;
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!("queue-33b-collect-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let fake = Sidecar::YtDlp.path_in(&root);
    // The link is the last argument, after `--`.
    std::fs::write(
        &fake,
        "#!/bin/sh\nfor a; do last=$a; done\ncase $last in *bad*) echo 'ERROR: [generic] Unsupported URL' >&2; exit 1;; esac\nprintf '{\"title\":\"Title of %s\",\"uploader\":\"Chan\",\"upload_date\":\"20240501\"}' \"$last\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();

    let list = "# links to label\nhttps://example.test/a\n\n  https://example.test/bad  \nhttps://example.test/b\n";
    let links = links(list);
    assert_eq!(
        links,
        [
            "https://example.test/a",
            "https://example.test/bad",
            "https://example.test/b"
        ]
    );
    let (entries, failed) = collect(&root, &links);
    assert_eq!(
        failed,
        [(
            "https://example.test/bad".to_owned(),
            "Unsupported URL".to_owned()
        )]
    );
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["url"], "https://example.test/a");
    assert_eq!(
        entries[0]["meta"]["title"],
        "Title of https://example.test/a"
    );
    assert_eq!(entries[0]["meta"]["channel"], "Chan");
    assert_eq!(entries[0]["meta"]["upload_date"], "20240501");
    assert_eq!(
        (
            &entries[1]["lang"],
            &entries[1]["expected"],
            &entries[1]["accept"]
        ),
        (
            &serde_json::json!(""),
            &serde_json::json!(""),
            &serde_json::json!([])
        )
    );

    let out = root.join("to-label.json");
    write_new(&out, &entries).unwrap();
    let written: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(written, entries);
    assert!(write_new(&out, &entries).is_err(), "never replaces a file");
    assert!(
        write_new(&root.join("titles.json"), &entries).is_err(),
        "never writes the seed set"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[path = "support/naming_collector_33b_qa.rs"]
mod collector_qa;

/// #33(b) review: the destination is checked before any link is fetched.
#[test]
fn n9_the_collector_checks_its_destination_first() {
    let root = std::env::temp_dir().join(format!("queue-33b-dest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    assert!(destination_ok(&root.join("to-label.json")).is_ok());
    assert!(
        destination_ok(&root.join("titles.json")).is_err(),
        "never the seed set"
    );
    std::fs::write(root.join("taken.json"), b"labels").unwrap();
    assert_eq!(
        destination_ok(&root.join("taken.json")).unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(
        destination_ok(&root.join("missing/out.json"))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
    );
    std::fs::remove_dir_all(&root).unwrap();
}
