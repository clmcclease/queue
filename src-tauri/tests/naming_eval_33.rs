//! #33 / N9: the naming eval set (`naming-eval/titles.json`) and its runner.
//!
//! The set's shape is checked on every run. Scoring a model runs only when asked:
//!
//! ```sh
//! QUEUE_EVAL_ENDPOINT=http://127.0.0.1:8080/v1 QUEUE_EVAL_MODEL=qwen3-4b \
//!   cargo test --test naming_eval_33 -- --ignored --nocapture
//! ```
//!
//! `QUEUE_EVAL_KEY` adds a key for an API provider. The run fails below 90% accepted (PRD §9);
//! latency is printed, not asserted (it depends on the hardware).
use std::time::{Duration, Instant};

use queue_lib::ai::{self, Endpoint};
use queue_lib::meta::Meta;
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
    let mut titles = std::collections::HashSet::new();
    for case in &cases {
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
        base_url,
        model: std::env::var("QUEUE_EVAL_MODEL").unwrap_or_default(),
        key: std::env::var("QUEUE_EVAL_KEY").ok(),
    };
    let cases = cases();
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
    assert!(accepted * 10 >= n * 9, "below 90% accepted: {percent}%");
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
