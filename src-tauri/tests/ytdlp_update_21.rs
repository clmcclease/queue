//! #21 (D-042): the yt-dlp updater end to end against a loopback server, with throwaway Ed25519
//! keys generated here (RSA-4096 is too slow in debug builds). Nothing touches the network
//! except the one `#[ignore]` live test, run by hand.
#[path = "support/accept.rs"]
mod accept;

use pgp::composed::{
    ArmorOptions, DetachedSignature, KeyType, SecretKeyParamsBuilder, SignedPublicKey,
    SignedSecretKey,
};
use pgp::crypto::hash::HashAlgorithm;
use pgp::ser::Serialize;
use pgp::types::{KeyDetails, Password};
use queue_lib::ytdlp_update::{self as up, Resolver, Trust, Updater, Version, tick, verify_sums};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

const ASSET: &str = "yt-dlp_test";

struct Key {
    secret: SignedSecretKey,
    armored: String,
    fingerprint: String,
}

fn key() -> Key {
    let params = SecretKeyParamsBuilder::default()
        .key_type(KeyType::Ed25519Legacy)
        .can_certify(true)
        .can_sign(true)
        .primary_user_id("Queue test <test@example.invalid>".into())
        .build()
        .unwrap();
    let secret = params.generate(rand::thread_rng()).unwrap();
    let public = SignedPublicKey::from(secret.clone());
    Key {
        armored: public.to_armored_string(ArmorOptions::default()).unwrap(),
        fingerprint: format!("{:X}", secret.fingerprint()),
        secret,
    }
}

fn sign(key: &Key, data: &[u8]) -> Vec<u8> {
    sign_with(key, data, HashAlgorithm::Sha256, false)
}

fn sign_with(key: &Key, data: &[u8], hash: HashAlgorithm, text: bool) -> Vec<u8> {
    let rng = rand::thread_rng();
    let pw = Password::empty();
    let sig = if text {
        DetachedSignature::sign_text_data(rng, &key.secret.primary_key, &pw, hash, data)
    } else {
        DetachedSignature::sign_binary_data(rng, &key.secret.primary_key, &pw, hash, data)
    };
    sig.unwrap().to_bytes().unwrap()
}

fn trust(key: &Key) -> Trust<'_> {
    Trust {
        key: &key.armored,
        fingerprint: &key.fingerprint,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sums_for(asset: &[u8]) -> Vec<u8> {
    format!(
        "{}  {ASSET}\n{}  yt-dlp_other\n",
        hex(&Sha256::digest(asset)),
        "c".repeat(64)
    )
    .into_bytes()
}

/// A canned answer: status, extra headers, body.
type Answer = (u16, Vec<(String, String)>, Vec<u8>);

/// A loopback server: `path → answer`, recording every path asked for.
struct Server {
    base: String,
    routes: Arc<Mutex<HashMap<String, Answer>>>,
    seen: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let routes: Arc<Mutex<HashMap<_, _>>> = Arc::default();
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (r, s, st) = (routes.clone(), seen.clone(), stop.clone());
        let thread = thread::spawn(move || {
            while let Some(stream) = accept::accept_until(&listener, || st.load(Ordering::SeqCst)) {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).is_err() || h == "\r\n" || h.is_empty() {
                        break;
                    }
                }
                s.lock().unwrap().push(path.clone());
                let (status, headers, body) = r.lock().unwrap().get(&path).cloned().unwrap_or((
                    404,
                    vec![],
                    b"not found".to_vec(),
                ));
                let mut out = stream;
                let mut head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                );
                for (k, v) in headers {
                    head.push_str(&format!("{k}: {v}\r\n"));
                }
                head.push_str("\r\n");
                let _ = out.write_all(head.as_bytes());
                let _ = out.write_all(&body);
            }
        });
        Self {
            base,
            routes,
            seen,
            stop,
            thread: Some(thread),
        }
    }

    fn route(&self, path: &str, status: u16, headers: &[(&str, &str)], body: &[u8]) {
        self.routes.lock().unwrap().insert(
            path.to_owned(),
            (
                status,
                headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                body.to_vec(),
            ),
        );
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }

    /// Serves `tag` as the newest release, signed by `key`, with `asset` as the binary.
    fn release(&self, tag: &str, key: &Key, asset: &[u8]) {
        let sums = sums_for(asset);
        self.route(
            "/latest/download/SHA2-256SUMS",
            302,
            &[(
                "Location",
                &format!("{}/download/{tag}/SHA2-256SUMS", self.base),
            )],
            b"",
        );
        self.route(&format!("/download/{tag}/SHA2-256SUMS"), 200, &[], &sums);
        self.route(
            &format!("/download/{tag}/SHA2-256SUMS.sig"),
            200,
            &[],
            &sign(key, &sums),
        );
        self.route(&format!("/download/{tag}/{ASSET}"), 200, &[], asset);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// One wake with the setting `enabled` (the setting command sets the resolver; #21 review).
fn tick_with(u: &Updater, enabled: bool, launched: &mut bool, r: &Resolver) {
    r.set_enabled(enabled);
    tick(u, launched, r);
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("queue-ytdlp-21-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

/// An updater on `root` against `server`, whose binaries "report" `reports` (by file contents
/// when `None`: the asset bytes are the version text).
fn updater(root: &Path, server: &Server, key: &Key, reports: Option<&str>) -> Updater {
    let reports = reports.map(str::to_owned);
    Updater {
        root: root.to_owned(),
        bundled: v("2026.08.19"),
        asset: ASSET.into(),
        key: key.armored.clone(),
        fingerprint: key.fingerprint.clone(),
        latest: format!("{}/latest/download/SHA2-256SUMS", server.base),
        download: format!("{}/download/", server.base),
        run_version: Box::new(move |path: &Path| match &reports {
            Some(r) => Ok(r.clone().into_bytes()),
            None => fs::read(path).map_err(|e| e.to_string()),
        }),
        asset_cap: 1024 * 1024,
    }
}

#[test]
fn the_signature_policy_accepts_only_binary_sha2_signatures_by_the_pinned_key() {
    let k = key();
    let other = key();
    let data = b"abc  file\n";
    verify_sums(trust(&k), data, &sign(&k, data)).unwrap();
    for hash in [HashAlgorithm::Sha384, HashAlgorithm::Sha512] {
        verify_sums(trust(&k), data, &sign_with(&k, data, hash, false)).unwrap();
    }
    assert!(
        verify_sums(trust(&k), data, &sign(&other, data)).is_err(),
        "another key"
    );
    assert!(
        verify_sums(
            trust(&k),
            data,
            &sign_with(&k, data, HashAlgorithm::Sha256, true)
        )
        .is_err(),
        "a text signature"
    );
    // Only SHA-256, SHA-384 and SHA-512 are accepted: SHA3 verifies cryptographically but isn't
    // on D-042's list. (rPGP refuses to make SHA-1 or SHA-224 Ed25519 signatures at all.)
    for other_hash in [HashAlgorithm::Sha3_256, HashAlgorithm::Sha3_512] {
        let sig = sign_with(&k, data, other_hash, false);
        assert!(
            verify_sums(trust(&k), data, &sig).is_err(),
            "{other_hash:?}"
        );
    }
    let pinned_elsewhere = Trust {
        key: &k.armored,
        fingerprint: &other.fingerprint,
    };
    assert!(
        verify_sums(pinned_elsewhere, data, &sign(&k, data)).is_err(),
        "the pin"
    );
    // The real key with a throwaway signature.
    assert!(verify_sums(up::pinned(), data, &sign(&k, data)).is_err());
}

#[test]
fn a_newer_signed_release_that_reports_its_version_is_adopted() {
    let server = Server::start();
    let k = key();
    server.release("2026.10.26", &k, b"2026.10.26\n");
    let root = temp("adopt");
    let u = updater(&root, &server, &k, None);
    let adopted = u.check(None).unwrap().expect("adopted");
    assert_eq!(adopted.version, v("2026.10.26"));
    let dir = root.join("2026.10.26");
    assert_eq!(adopted.path, dir.join(ASSET));
    for f in ["SHA2-256SUMS", "SHA2-256SUMS.sig", "verified.json", ASSET] {
        assert!(dir.join(f).is_file(), "{f}");
    }
    assert!(!dir.join(format!("{ASSET}.part")).exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&adopted.path).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
    let state = u.state();
    assert!(state.last_check.is_some());
    assert_eq!(state.last_result.as_deref(), Some("Updated to 2026.10.26."));
    // A restart finds it again by re-verifying the chain.
    assert_eq!(u.launch().unwrap().version, v("2026.10.26"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_older_signed_release_under_a_newer_tag_is_rejected_and_remembered() {
    let server = Server::start();
    let k = key();
    // A genuinely signed old binary served as the newest tag (D-042's rollback case).
    server.release("2026.10.26", &k, b"2026.09.01\n");
    let root = temp("rollback");
    let u = updater(&root, &server, &k, None);
    let err = u.check(None).unwrap_err();
    assert!(err.contains("didn't report version 2026.10.26"), "{err}");
    assert!(!root.join("2026.10.26").exists(), "the copy is deleted");
    assert_eq!(u.state().rejected.as_deref(), Some("2026.10.26"));
    let before = server.seen().len();
    assert_eq!(u.check(None).unwrap(), None, "skipped until a newer tag");
    assert_eq!(
        server.seen().len() - before,
        1,
        "only the redirect is asked"
    );
    server.release("2026.10.27", &k, b"2026.10.27\n");
    assert_eq!(u.check(None).unwrap().unwrap().version, v("2026.10.27"));
    assert_eq!(u.state().rejected, None, "a newer tag clears it");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn nothing_is_downloaded_unless_the_tag_is_newer_than_bundled_and_current() {
    let server = Server::start();
    let k = key();
    let root = temp("not-newer");
    let u = updater(&root, &server, &k, None);
    for (tag, current) in [
        ("2026.08.19", None),
        ("2026.07.01", None),
        ("2026.10.26", Some("2026.10.26")),
    ] {
        server.release(tag, &k, format!("{tag}\n").as_bytes());
        let before = server.seen().len();
        assert_eq!(u.check(current.map(v)).unwrap(), None, "{tag}");
        assert_eq!(server.seen().len() - before, 1, "{tag}: only the redirect");
        assert_eq!(u.state().last_result.as_deref(), Some("Up to date."));
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_bad_signature_hash_or_redirect_leaves_nothing_and_is_not_remembered() {
    let k = key();
    let other = key();
    let root = temp("bad");
    // A signature by another key.
    let server = Server::start();
    server.release("2026.10.26", &other, b"2026.10.26\n");
    let u = updater(&root, &server, &k, None);
    assert!(u.check(None).unwrap_err().contains("signature"));
    assert!(
        !server.seen().iter().any(|p| p.ends_with(ASSET)),
        "no binary downloaded"
    );
    // A binary that doesn't match its signed sha256.
    let server = Server::start();
    server.release("2026.10.26", &k, b"2026.10.26\n");
    server.route(
        &format!("/download/2026.10.26/{ASSET}"),
        200,
        &[],
        b"something else",
    );
    let u = updater(&root, &server, &k, None);
    assert!(u.check(None).unwrap_err().contains("doesn't match"));
    assert!(!root.join("2026.10.26").exists());
    assert_eq!(u.state().rejected, None);
    // A binary over the size cap.
    let server = Server::start();
    let big = vec![b'x'; 1024 * 1024 + 1];
    server.release("2026.10.26", &k, &big);
    let u = updater(&root, &server, &k, None);
    assert!(u.check(None).unwrap_err().contains("larger"));
    assert!(!root.join("2026.10.26").exists());
    // A redirect to plain HTTP that isn't this machine.
    let server = Server::start();
    server.release("2026.10.26", &k, b"2026.10.26\n");
    server.route(
        "/download/2026.10.26/SHA2-256SUMS",
        302,
        &[("Location", "http://example.com/SHA2-256SUMS")],
        b"",
    );
    let u = updater(&root, &server, &k, None);
    assert!(u.check(None).unwrap_err().contains("https://"));
    // A first hop that names the wrong place or isn't a tag.
    for location in [
        format!("{}/download/nightly/SHA2-256SUMS", server.base),
        format!("{}/elsewhere/2026.10.26/SHA2-256SUMS", server.base),
        format!("{}/download/2026.10.26/SHA2-256SUMS.sig", server.base),
    ] {
        server.route(
            "/latest/download/SHA2-256SUMS",
            302,
            &[("Location", &location)],
            b"",
        );
        let before = server.seen().len();
        assert!(
            u.check(None).unwrap_err().contains("wasn't understood"),
            "{location}"
        );
        assert_eq!(
            server.seen().len() - before,
            1,
            "{location}: nothing more asked"
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_failed_state_write_sends_no_request() {
    let server = Server::start();
    let k = key();
    server.release("2026.10.26", &k, b"2026.10.26\n");
    let root = temp("state-fail");
    fs::create_dir_all(root.parent().unwrap()).unwrap();
    fs::write(&root, b"a file where the folder should be").unwrap();
    let u = updater(&root, &server, &k, None);
    assert!(u.check(None).unwrap_err().contains("update state"));
    assert!(server.seen().is_empty());
    fs::remove_file(root).unwrap();
}

#[test]
fn launch_keeps_only_the_newest_copy_that_verifies_and_beats_bundled() {
    let server = Server::start();
    let k = key();
    let root = temp("launch");
    let u = updater(&root, &server, &k, None);
    for tag in ["2026.09.01", "2026.10.26"] {
        server.release(tag, &k, format!("{tag}\n").as_bytes());
        u.check(None).unwrap().unwrap();
    }
    // Junk the launch must clear: a folder without a marker, a version not newer than bundled,
    // a stray file, a `.part`, a name that isn't a version.
    fs::create_dir_all(root.join("2026.11.01")).unwrap();
    fs::create_dir_all(root.join("2026.08.19")).unwrap();
    fs::write(root.join("stray"), b"x").unwrap();
    fs::create_dir_all(root.join("not-a-version")).unwrap();
    fs::write(root.join("2026.10.26").join(format!("{ASSET}.part")), b"x").unwrap();
    let adopted = u.launch().unwrap();
    assert_eq!(adopted.version, v("2026.10.26"));
    let mut left: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left, ["2026.10.26", "state.json"]);
    assert!(
        !root
            .join("2026.10.26")
            .join(format!("{ASSET}.part"))
            .exists()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn launch_rejects_a_swapped_binary_even_with_a_rewritten_marker() {
    let server = Server::start();
    let k = key();
    let root = temp("swap");
    let u = updater(&root, &server, &k, None);
    server.release("2026.10.26", &k, b"2026.10.26\n");
    u.check(None).unwrap().unwrap();
    let dir = root.join("2026.10.26");
    fs::write(dir.join(ASSET), b"#!/bin/sh\necho planted\n").unwrap();
    fs::write(
        dir.join("verified.json"),
        br#"{"version":"2026.10.26","asset":"yt-dlp_test"}"#,
    )
    .unwrap();
    assert_eq!(u.launch(), None);
    assert!(!dir.exists(), "removed");
    // Sums rewritten to match a planted binary don't carry the signature.
    u.check(None).unwrap().unwrap();
    fs::write(dir.join(ASSET), b"planted").unwrap();
    fs::write(dir.join("SHA2-256SUMS"), sums_for(b"planted")).unwrap();
    assert_eq!(u.launch(), None);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn off_requests_nothing_and_uses_bundled_and_on_verifies_then_checks_daily() {
    let server = Server::start();
    let k = key();
    let root = temp("tick");
    let u = updater(&root, &server, &k, None);
    server.release("2026.10.26", &k, b"2026.10.26\n");
    let r = Resolver::default();
    let bundled = PathBuf::from("/bundled/yt-dlp");
    let mut launched = false;
    tick_with(&u, false, &mut launched, &r);
    assert!(server.seen().is_empty(), "off asks nothing");
    assert!(!launched && !root.exists(), "off touches nothing");
    assert_eq!(r.resolve(bundled.clone()), bundled);
    tick_with(&u, true, &mut launched, &r);
    assert_eq!(r.in_use(), Some(v("2026.10.26")));
    assert_eq!(
        r.resolve(bundled.clone()),
        root.join("2026.10.26").join(ASSET)
    );
    let asked = server.seen().len();
    tick_with(&u, true, &mut launched, &r);
    assert_eq!(server.seen().len(), asked, "not again within a day");
    tick_with(&u, false, &mut launched, &r);
    assert_eq!(r.resolve(bundled.clone()), bundled, "off means bundled");
    assert!(root.join("2026.10.26").exists(), "and the copy is kept");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_stored_copy_not_newer_than_bundled_is_ignored_and_removed() {
    let server = Server::start();
    let k = key();
    let root = temp("not-newer-stored");
    let mut u = updater(&root, &server, &k, None);
    server.release("2026.10.26", &k, b"2026.10.26\n");
    u.check(None).unwrap().unwrap();
    u.bundled = v("2026.10.26"); // a Queue upgrade bundled the same or newer
    assert_eq!(u.launch(), None);
    assert!(!root.join("2026.10.26").exists());
    fs::remove_dir_all(root).unwrap();
}

/// The real thing, by hand on macOS: `cargo test --test ytdlp_update_21 -- --ignored --nocapture`.
#[test]
#[ignore = "reaches GitHub and runs the downloaded yt-dlp"]
fn live_update_from_github() {
    let root = temp("live");
    let u = Updater::real(root.clone()).expect("this target has an updater");
    let adopted = u.check(None);
    println!("check: {adopted:?}\nstate: {:?}", u.state());
    let adopted = adopted
        .unwrap()
        .expect("a release newer than the bundled one");
    let out = up::run_version(&adopted.path).unwrap();
    println!("--version: {}", String::from_utf8_lossy(&out).trim());
    assert_eq!(u.launch().unwrap().version, adopted.version);
    fs::remove_dir_all(root).unwrap();
}

// Independent QA regressions for #21, PRD §§6.4/7, D-036/D-042.
#[test]
fn qa21_a_failed_attempt_consumes_the_day_across_restarts() {
    let server = Server::start(); // No route: the first request returns HTTP 404.
    let k = key();
    let root = temp("qa-failed-day");
    let u = updater(&root, &server, &k, None);
    let r = Resolver::default();
    tick_with(&u, true, &mut false, &r);
    assert_eq!(server.seen().len(), 1);
    assert!(u.state().last_result.unwrap().contains("HTTP 404"));
    let restarted = updater(&root, &server, &k, None);
    let r = Resolver::default();
    tick_with(&restarted, true, &mut false, &r);
    assert_eq!(server.seen().len(), 1, "a failed check is still a check");
    let bundled = root.join("bundled");
    assert_eq!(r.resolve(bundled.clone()), bundled);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn qa21_a_bad_new_release_preserves_the_current_verified_copy() {
    let server = Server::start();
    let k = key();
    let root = temp("qa-preserve");
    let u = updater(&root, &server, &k, None);
    server.release("2026.10.26", &k, b"2026.10.26\n");
    let old = u.check(None).unwrap().unwrap();
    let r = Resolver::default();
    r.set_enabled(true);
    r.adopt(old.clone());
    server.release("2026.10.27", &k, b"2026.10.27\n");
    server.route(
        &format!("/download/2026.10.27/{ASSET}"),
        200,
        &[],
        b"corrupt",
    );
    fs::write(root.join("state.json"), br#"{"lastCheck":0}"#).unwrap();
    tick_with(&u, true, &mut true, &r);
    assert_eq!(r.resolve(root.join("bundled")), old.path);
    assert_eq!(fs::read(&old.path).unwrap(), b"2026.10.26\n");
    assert!(!root.join("2026.10.27").exists());
    assert_eq!(u.launch().unwrap().version, old.version);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn qa21_off_leaves_existing_state_and_incomplete_downloads_untouched() {
    let server = Server::start();
    let k = key();
    let root = temp("qa-off-existing");
    fs::create_dir_all(root.join("unfinished")).unwrap();
    let state = br#"{"lastCheck":0,"lastResult":"previous"}"#;
    fs::write(root.join("state.json"), state).unwrap();
    fs::write(root.join("unfinished/file.part"), b"partial").unwrap();
    let u = updater(&root, &server, &k, None);
    let mut launched = false;
    tick_with(&u, false, &mut launched, &Resolver::default());
    assert!(!launched);
    assert!(server.seen().is_empty());
    assert_eq!(fs::read(root.join("state.json")).unwrap(), state);
    assert_eq!(
        fs::read(root.join("unfinished/file.part")).unwrap(),
        b"partial"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn qa21_signed_but_ambiguous_or_missing_asset_checksums_never_execute() {
    let server = Server::start();
    let k = key();
    let root = temp("qa-bad-sums");
    let mut u = updater(&root, &server, &k, None);
    u.run_version = Box::new(|_| panic!("unverified asset must not execute"));
    for sums in [
        format!("{}  other_asset\n", "a".repeat(64)).into_bytes(),
        format!("{}  {ASSET}\n", "g".repeat(64)).into_bytes(),
        [sums_for(b"2026.10.26\n"), sums_for(b"2026.10.26\n")].concat(),
        vec![0xff, 0xfe],
    ] {
        server.release("2026.10.26", &k, b"2026.10.26\n");
        server.route("/download/2026.10.26/SHA2-256SUMS", 200, &[], &sums);
        server.route(
            "/download/2026.10.26/SHA2-256SUMS.sig",
            200,
            &[],
            &sign(&k, &sums),
        );
        let before = server.seen().len();
        assert!(u.check(None).is_err());
        assert!(!server.seen()[before..].iter().any(|p| p.ends_with(ASSET)));
        assert!(!root.join("2026.10.26").exists());
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn qa21_redirect_cycles_stop_without_executing_an_asset() {
    let server = Server::start();
    let k = key();
    let root = temp("qa-redirect-cycle");
    server.release("2026.10.26", &k, b"2026.10.26\n");
    server.route(
        "/download/2026.10.26/SHA2-256SUMS",
        302,
        &[("Location", "/cycle")],
        b"",
    );
    server.route("/cycle", 302, &[("Location", "/cycle")], b"");
    let u = updater(&root, &server, &k, None);
    assert!(u.check(None).unwrap_err().contains("too many times"));
    assert!(server.seen().len() <= 8, "redirects must be bounded");
    assert!(!root.join("2026.10.26").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn qa21_launch_falls_back_to_older_verified_copy_after_newest_corruption() {
    let server = Server::start();
    let k = key();
    let root = temp("qa-launch-fallback");
    let u = updater(&root, &server, &k, None);
    for tag in ["2026.10.26", "2026.10.27"] {
        server.release(tag, &k, format!("{tag}\n").as_bytes());
        u.check(None).unwrap().unwrap();
    }
    fs::write(root.join("2026.10.27").join(ASSET), b"tampered").unwrap();
    let before = server.seen().len();
    assert_eq!(u.launch().unwrap().version, v("2026.10.26"));
    assert_eq!(
        server.seen().len(),
        before,
        "launch verification is offline"
    );
    assert!(!root.join("2026.10.27").exists());
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
fn qa21_script(root: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(root).unwrap();
    let path = root.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn qa21_real_version_runner_keeps_unicode_and_shell_metacharacters_literal() {
    let root = temp("qa-argv");
    let path = qa21_script(
        &root,
        "雪 ; $(echo unsafe) yt-dlp",
        r#"
test "$#" = 3 || exit 1
test "$1" = --ignore-config || exit 2
test "$2" = --no-update || exit 3
test "$3" = --version || exit 4
printf '2026.10.26\n'
"#,
    );
    assert_eq!(up::run_version(&path).unwrap(), b"2026.10.26\n");
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn qa21_real_version_runner_rejects_nonzero_exit_and_output_floods() {
    let root = temp("qa-run-fail");
    let bad = qa21_script(&root, "nonzero", "printf '2026.10.26\\n'; exit 7");
    assert!(
        up::run_version(&bad).is_err(),
        "printing a version isn't enough"
    );
    let flood = qa21_script(
        &root,
        "flood",
        "while :; do printf '0123456789012345678901234567890123456789'; done",
    );
    let started = std::time::Instant::now();
    assert!(up::run_version(&flood).is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn qa21_real_version_timeout_terminates_a_descendant_holding_stdout() {
    let root = temp("qa-timeout");
    let path = qa21_script(
        &root,
        "stalled",
        r#"
sleep 120 &
printf '%s' "$!" > "$(dirname "$0")/child.pid"
exit 0
"#,
    );
    let started = std::time::Instant::now();
    let result = up::run_version(&path);
    let elapsed = started.elapsed();
    let pid: i32 = fs::read_to_string(root.join("child.pid"))
        .unwrap()
        .parse()
        .unwrap();
    let alive = unsafe { libc::kill(pid, 0) == 0 };
    if alive {
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
    fs::remove_dir_all(root).unwrap();
    assert!(result.is_err());
    assert!(elapsed >= std::time::Duration::from_secs(59), "{elapsed:?}");
    assert!(elapsed < std::time::Duration::from_secs(68), "{elapsed:?}");
    assert!(
        !alive,
        "timing out must terminate descendants as well as the leader"
    );
}

#[cfg(unix)]
#[test]
fn qa21_losing_execute_permission_falls_back_to_the_bundled_copy() {
    use std::os::unix::fs::PermissionsExt;
    let root = temp("qa-permission-fallback");
    let path = qa21_script(&root, "yt-dlp", "printf '2026.10.26\\n'");
    let r = Resolver::default();
    r.set_enabled(true);
    r.adopt(up::Adopted {
        version: v("2026.10.26"),
        path: path.clone(),
        stamp: up::Stamp::of(&path).unwrap(),
    });
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        up::run_version(&path).is_err(),
        "the copy is no longer runnable"
    );
    let bundled = root.join("bundled");
    let resolved = r.resolve(bundled.clone());
    fs::remove_dir_all(root).unwrap();
    assert_eq!(resolved, bundled, "an unusable update must fall back");
}

// #21 review round 1.

#[test]
fn switching_off_mid_check_stops_before_the_next_request_and_leaves_nothing() {
    let k = key();
    // Off after each step in turn: the redirect, the sums, the signature, partway into the
    // binary, and after the binary is placed.
    for stop_after in 1..=4 {
        let server = Server::start();
        server.release("2026.10.26", &k, &vec![b'x'; 300_000]);
        let root = temp(&format!("off-mid-{stop_after}"));
        let u = updater(&root, &server, &k, Some("2026.10.26"));
        let r = Resolver::default();
        r.set_enabled(true);
        let go = || {
            if server.seen().len() >= stop_after {
                r.set_enabled(false);
            }
            r.enabled()
        };
        let err = u.check_while(None, &go).unwrap_err();
        assert!(err.contains("turned off"), "{stop_after}: {err}");
        assert_eq!(
            server.seen().len(),
            stop_after,
            "{stop_after}: no request after off"
        );
        assert!(
            !root.join("2026.10.26").exists(),
            "{stop_after}: nothing left"
        );
        assert_eq!(r.in_use(), None);
        fs::remove_dir_all(root).unwrap();
    }
    // Off before anything: not even the state is touched.
    let server = Server::start();
    let root = temp("off-before");
    let u = updater(&root, &server, &k, None);
    assert!(u.check_while(None, &|| false).is_err());
    assert!(server.seen().is_empty() && !root.exists());
}

#[test]
fn a_stale_on_cant_switch_updates_back_on() {
    let server = Server::start();
    let k = key();
    server.release("2026.10.26", &k, b"2026.10.26\n");
    let root = temp("stale-on");
    let u = updater(&root, &server, &k, None);
    let r = Resolver::default();
    r.set_enabled(true);
    r.set_enabled(false); // the setting command, after the worker last looked
    tick(&u, &mut false, &r);
    assert!(server.seen().is_empty());
    assert!(!r.enabled());
}

#[test]
fn a_binary_that_couldnt_run_is_retried_on_the_next_check_not_rejected() {
    let server = Server::start();
    let k = key();
    server.release("2026.10.26", &k, b"2026.10.26\n");
    let root = temp("transient");
    let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut u = updater(&root, &server, &k, None);
    let counted = runs.clone();
    u.run_version = Box::new(move |path: &Path| {
        if counted.fetch_add(1, Ordering::SeqCst) == 0 {
            Err("timed out".into())
        } else {
            fs::read(path).map_err(|e| e.to_string())
        }
    });
    let err = u.check(None).unwrap_err();
    assert!(err.contains("couldn't be run"), "{err}");
    assert_eq!(u.state().rejected, None, "not remembered");
    assert!(!root.join("2026.10.26").exists());
    assert_eq!(
        u.check(None).unwrap().unwrap().version,
        v("2026.10.26"),
        "the next check"
    );
    fs::remove_dir_all(root).unwrap();
}

// #21 review round 2 (architect, D-028): off is read before every request, redirect hops
// included. Written red first (STATUS records each failing on the round-2 code).

/// `file`'s request answers with a redirect to `/hop/<file>`, and updates are switched off as
/// soon as that first request arrives: the hop must never be asked for.
fn off_during_a_redirect_of(file: &str) {
    let k = key();
    let server = Server::start();
    let asset = b"2026.10.26\n";
    server.release("2026.10.26", &k, asset);
    let first = format!("/download/2026.10.26/{file}");
    let hop = format!("/hop/{file}");
    let body = match file {
        "SHA2-256SUMS" => sums_for(asset),
        "SHA2-256SUMS.sig" => sign(&k, &sums_for(asset)),
        _ => asset.to_vec(),
    };
    server.route(
        &first,
        302,
        &[("Location", &format!("{}{hop}", server.base))],
        b"",
    );
    server.route(&hop, 200, &[], &body);
    let root = temp(&format!("off-hop-{}", file.replace('.', "-")));
    let u = updater(&root, &server, &k, Some("2026.10.26"));
    let r = Resolver::default();
    r.set_enabled(true);
    let go = || {
        if server.seen().contains(&first) {
            r.set_enabled(false);
        }
        r.enabled()
    };
    let err = u.check_while(None, &go).unwrap_err();
    assert!(err.contains("turned off"), "{file}: {err}");
    assert!(
        !server.seen().contains(&hop),
        "{file}: the hop was requested after off"
    );
    assert!(!root.join("2026.10.26").exists(), "{file}: nothing left");
    assert_eq!(r.in_use(), None);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn off_during_the_sums_redirect_sends_no_hop() {
    off_during_a_redirect_of("SHA2-256SUMS");
}

#[test]
fn off_during_the_signature_redirect_sends_no_hop() {
    off_during_a_redirect_of("SHA2-256SUMS.sig");
}

#[test]
fn off_during_the_binary_redirect_sends_no_hop() {
    off_during_a_redirect_of(ASSET);
}
