//! #21 review: yt-dlp runs go through the adopted copy (D-042). `meta::fetch` and
//! `download::run_in` ask `ytdlp_update::global()` for the binary: the adopted copy while
//! updates are on and its file is unchanged, else the bundled one. One test in its own binary,
//! since the resolver is process-wide (never in download_d2.rs, whose tests rely on it being
//! off). Unix: fake yt-dlp scripts.
#![cfg(unix)]
use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use queue_lib::{download, meta, sidecar::Sidecar, ytdlp_update as up};

fn script(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, Permissions::from_mode(0o755)).unwrap();
}

fn title(dir: &Path, url: &str) -> Result<String, String> {
    meta::fetch(dir, url, Duration::from_secs(10)).map(|m| m.title.unwrap_or_default())
}

#[test]
fn metadata_uses_the_adopted_copy_only_while_on_and_unchanged() {
    let root: PathBuf =
        std::env::temp_dir().join(format!("queue-ytdlp-resolve-21-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let bundled_dir = root.join("app");
    script(
        &Sidecar::YtDlp.path_in(&bundled_dir),
        r#"printf '%s' '{"title":"Bundled"}'"#,
    );
    let adopted_path = root.join("data/yt-dlp/2026.10.26/yt-dlp_test");
    // The updated copy fails one item and names the others: a failed row mustn't stop the rest.
    script(
        &adopted_path,
        r#"for a in "$@"; do last="$a"; done
case "$last" in *bad*) echo 'ERROR: [generic] Unsupported URL' >&2; exit 1;; esac
printf '%s' '{"title":"Updated"}'"#,
    );
    let r = up::global();
    downloads_use_the_adopted_copy(&root, r);
    r.adopt(up::Adopted {
        version: up::Version::parse("2026.10.26").unwrap(),
        path: adopted_path.clone(),
        stamp: up::Stamp::of(&adopted_path).unwrap(),
    });
    assert_eq!(
        title(&bundled_dir, "https://e.test/a").unwrap(),
        "Bundled",
        "off by default"
    );
    r.set_enabled(true);
    assert_eq!(title(&bundled_dir, "https://e.test/a").unwrap(), "Updated");
    assert_eq!(
        title(&bundled_dir, "https://e.test/bad").unwrap_err(),
        "Unsupported URL"
    );
    assert_eq!(
        title(&bundled_dir, "https://e.test/b").unwrap(),
        "Updated",
        "the next item"
    );
    r.set_enabled(false);
    assert_eq!(
        title(&bundled_dir, "https://e.test/a").unwrap(),
        "Bundled",
        "off"
    );
    r.set_enabled(true);
    fs::set_permissions(&adopted_path, Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        title(&bundled_dir, "https://e.test/a").unwrap(),
        "Bundled",
        "a changed file"
    );
    fs::set_permissions(&adopted_path, Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        title(&bundled_dir, "https://e.test/a").unwrap(),
        "Bundled",
        "for the session"
    );
    fs::remove_dir_all(root).unwrap();
}

/// `-P` is the working folder, as with the real yt-dlp (download_d2.rs's convention).
const FIND_P: &str = "P=; prev=; for a in \"$@\"; do [ \"$prev\" = -P ] && P=$a; prev=$a; done";

fn download(bin: &Path, root: &Path, url: &str) -> (Result<String, String>, usize) {
    let work = root.join("cache/work");
    let out = root.join("out");
    fs::create_dir_all(&out).unwrap();
    let mut progress = 0;
    let result = download::run_with_retries_events(
        bin,
        &work,
        &out,
        url,
        None,
        |_| {},
        |_| {},
        |e| {
            if matches!(e, download::Event::Progress(_)) {
                progress += 1;
            }
        },
    );
    (result, progress)
}

/// The architect's (b) (D-028, 2026-11-10): with updates on, a failed item and then a finished
/// one, both through the adopted copy; with updates off, the bundled one.
fn downloads_use_the_adopted_copy(root: &Path, r: &up::Resolver) {
    let bin = root.join("dl/app");
    script(
        &Sidecar::YtDlp.path_in(&bin),
        &format!("{FIND_P}\nprintf bundled > \"$P\"/from-bundled.mp4"),
    );
    let adopted = root.join("dl/data/yt-dlp/2026.10.26/yt-dlp_test");
    script(
        &adopted,
        &format!(
            r#"{FIND_P}
for a in "$@"; do last="$a"; done
case "$last" in *bad*) echo 'ERROR: [generic] Unsupported URL' >&2; exit 1;; esac
echo 'QUEUE-PROGRESS {{"status":"downloading","downloaded":5,"total":10}}'
printf updated > "$P"/from-adopted.mp4"#
        ),
    );
    r.adopt(up::Adopted {
        version: up::Version::parse("2026.10.26").unwrap(),
        path: adopted.clone(),
        stamp: up::Stamp::of(&adopted).unwrap(),
    });
    r.set_enabled(true);
    let (failed, _) = download(&bin, &root.join("dl"), "https://e.test/bad");
    assert!(failed.unwrap_err().contains("Unsupported URL"));
    let (saved, progress) = download(&bin, &root.join("dl"), "https://e.test/good");
    let saved = saved.unwrap();
    assert!(saved.ends_with("from-adopted.mp4"), "{saved}");
    assert!(progress >= 1, "progress reported");
    r.set_enabled(false);
    let (saved, _) = download(&bin, &root.join("dl"), "https://e.test/good");
    assert!(saved.unwrap().ends_with("from-bundled.mp4"));
}
