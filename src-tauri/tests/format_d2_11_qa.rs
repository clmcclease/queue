//! Independent QA: #11 / D2 / D-015. Reuse #41's portable executable fixture;
//! these checks exercise the subprocess path without requiring external media or a webview.
use queue_lib::{
    download::{self, Format},
    settings::Settings,
    sidecar::Sidecar,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
static FIXTURE: OnceLock<PathBuf> = OnceLock::new();
const FORMATS: [Format; 4] = [
    Format::Default,
    Format::CompatibleVideo,
    Format::Audio,
    Format::CompatibleAudio,
];

struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "queue-format-11-{}-{} 日本語",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn fake(&self, scenario: &str, name: &str) {
        let executable = FIXTURE.get_or_init(|| {
            let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/qa-format-11");
            fs::create_dir_all(&dir).unwrap();
            let exe = dir.join(format!(
                "fixture-{}{}",
                std::process::id(),
                std::env::consts::EXE_SUFFIX
            ));
            let result = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
                .args(["--edition=2024", "--crate-name", "queue_format_11_fixture"])
                .arg(
                    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/support/native_sidecar_41.rs"),
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
        });
        fs::copy(executable, Sidecar::YtDlp.path_in(&self.0)).unwrap();
        fs::write(self.0.join("scenario.txt"), scenario).unwrap();
        fs::write(self.0.join("name.txt"), name).unwrap();
        fs::create_dir(self.0.join("out")).unwrap();
    }
    fn args(&self, n: u32) -> Vec<String> {
        fs::read_to_string(self.0.join(format!("args-{n}.txt")))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// An independent specification of D-015, also checked on retry and saved-info paths.
fn expected_flags(format: Format) -> &'static [&'static str] {
    match format {
        Format::Default => &[],
        Format::CompatibleVideo => &[
            "-S",
            "vcodec:h264,vext:mp4,lang,quality,res,fps,hdr:12,acodec:aac,aext:m4a",
            "--merge-output-format",
            "mp4/webm/mkv",
        ],
        Format::Audio => &["-x"],
        Format::CompatibleAudio => &["-f", "ba/b", "-S", "acodec:aac,aext:m4a,vext:mp4", "-x"],
    }
}

fn assert_format_args(args: &[String], format: Format) {
    let flags = expected_flags(format);
    let start = args.iter().position(|a| a == "--no-playlist").unwrap();
    assert_eq!(&args[start - flags.len()..start], flags);
    for option in ["-f", "-S", "-x", "--merge-output-format"] {
        assert_eq!(
            args.iter().filter(|a| a.as_str() == option).count(),
            usize::from(flags.contains(&option))
        );
    }
    assert!(!args.iter().any(|a| {
        ["-t", "--remux-video", "--recode-video", "--audio-format"].contains(&a.as_str())
    }));
}

#[test]
fn every_format_survives_saved_info_then_403_reextraction_and_collision() {
    for format in FORMATS {
        let f = Sandbox::new();
        let name = "日本語 🎵 é & $() ;.opus";
        f.fake("retry", name);
        let info = f.0.join("saved info 日本語 & ;.json");
        fs::write(&info, b"{}").unwrap();
        fs::write(f.0.join("out").join(name), b"keep original").unwrap();
        let url = "https://example.test/日本語?x=$(oops)&y=;|\"";
        let mut waits = Vec::new();
        let mut retries = Vec::new();
        let work = f.0.join("work 日本語");
        let saved = download::run_with_retries_format(
            &f.0,
            &work,
            &f.0.join("out"),
            url,
            Some(&info),
            format,
            |d| waits.push(d),
            |n| retries.push(n),
            |_| {},
        )
        .unwrap();
        assert_eq!(
            PathBuf::from(&saved),
            f.0.join("out/日本語 🎵 é & $() ; (2).opus")
        );
        assert_eq!(fs::read(saved).unwrap(), b"fixture media");
        assert_eq!(
            fs::read(f.0.join("out").join(name)).unwrap(),
            b"keep original"
        );
        assert_eq!(waits, [Duration::from_secs(10)]);
        assert_eq!(retries, [2]);
        assert!(!work.exists());
        let first = f.args(1);
        let second = f.args(2);
        assert_format_args(&first, format);
        assert_format_args(&second, format);
        assert_eq!(
            &first[first.len() - 2..],
            ["--load-info-json", info.to_str().unwrap()]
        );
        assert_eq!(&second[second.len() - 2..], ["--", url]);
    }
}

#[test]
fn audio_postprocessing_preserves_valid_progress_after_malformed_output_and_cleans_failure() {
    for format in [Format::Audio, Format::CompatibleAudio] {
        let f = Sandbox::new();
        f.fake("malformed", "unused.opus");
        let work = f.0.join("work");
        let mut events = Vec::new();
        let error = download::run_with_retries_format(
            &f.0,
            &work,
            &f.0.join("out"),
            "https://example.test/v",
            None,
            format,
            |_| panic!("non-403 must not wait"),
            |_| panic!("non-403 must not retry"),
            |e| events.push(e),
        )
        .unwrap_err();
        assert_eq!(error, "extraction failed 日本語");
        assert_eq!(events.len(), 2);
        assert!(
            matches!(&events[0], download::Event::Progress(p) if p.percent == Some(75.0) && p.part == 1)
        );
        assert_eq!(
            events[1],
            download::Event::Processing("ExtractAudio".into())
        );
        assert!(!work.exists());
        assert_eq!(fs::read_dir(f.0.join("out")).unwrap().count(), 0);
        assert_format_args(&f.args(1), format);
    }
}

#[test]
fn every_format_roundtrips_without_losing_the_unicode_download_folder() {
    let f = Sandbox::new();
    let file = f.0.join("settings.json");
    let folder = f.0.join("Vidéos 日本語 é 🎵");
    for format in FORMATS {
        let settings = Settings {
            download_dir: Some(folder.clone()),
            format,
        };
        settings.save(&file).unwrap();
        assert_eq!(Settings::load(&file), settings);
        assert_eq!(
            fs::read_dir(&f.0).unwrap().count(),
            1,
            "no leftover temporary files"
        );
    }
}

#[test]
fn legacy_settings_keep_the_download_folder_and_select_default_format() {
    let f = Sandbox::new();
    let file = f.0.join("settings.json");
    let folder = f.0.join("Downloads 日本語");
    fs::write(
        &file,
        serde_json::to_vec(&serde_json::json!({"downloadDir": folder})).unwrap(),
    )
    .unwrap();
    let loaded = Settings::load(&file);
    assert_eq!(loaded.format, Format::Default);
    assert_eq!(loaded.download_dir, Some(folder));
}

#[test]
fn unknown_format_defaults_only_that_setting_and_preserves_the_download_folder() {
    let f = Sandbox::new();
    let file = f.0.join("settings.json");
    let folder = f.0.join("Downloads 日本語");
    // D-020: missing or damaged values use defaults. An unknown preset must not
    // silently redirect subsequent downloads to the OS Downloads folder.
    fs::write(
        &file,
        serde_json::to_vec(&serde_json::json!({"downloadDir": folder, "format": "future-format"}))
            .unwrap(),
    )
    .unwrap();
    let loaded = Settings::load(&file);
    assert_eq!(loaded.format, Format::Default);
    assert_eq!(loaded.download_dir, Some(folder));
}

#[test]
fn compatible_notices_handle_uppercase_numbered_unicode_and_missing_extensions() {
    for name in ["日本語 (2).OPUS", "name.WEBM", "name.MKV", "no-extension"] {
        assert!(Format::CompatibleVideo.notice(Path::new(name)).is_some());
        assert!(Format::CompatibleAudio.notice(Path::new(name)).is_some());
        assert_eq!(Format::Default.notice(Path::new(name)), None);
        assert_eq!(Format::Audio.notice(Path::new(name)), None);
    }
    for name in ["name (2).M4A", "name.AAC", "name.MP3"] {
        assert_eq!(Format::CompatibleAudio.notice(Path::new(name)), None);
    }
}
