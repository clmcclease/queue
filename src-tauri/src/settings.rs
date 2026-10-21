//! User settings (D1 #10; the settings screen #36 adds more), kept as JSON in the app's config
//! folder. Plain data with no Tauri types, so loading and saving are unit-testable.
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The settings file's name inside the app config folder.
pub const FILE: &str = "settings.json";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Where finished downloads go; `None` means the OS Downloads folder.
    #[serde(deserialize_with = "or_default")]
    pub download_dir: Option<PathBuf>,
    /// What to download (D2, #11); yt-dlp decides by default.
    #[serde(deserialize_with = "or_default")]
    pub format: crate::download::Format,
    /// How proposed names are built (N3, #17); `{title}` by default.
    #[serde(deserialize_with = "or_default")]
    pub naming_template: crate::naming::Template,
}

/// Reads one setting, or its default if it can't be understood (e.g. a format from a newer
/// version). Only that setting falls back: the others in the file are kept.
fn or_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(T::deserialize(value).unwrap_or_default())
}

impl Settings {
    /// Reads the settings file. A missing or unreadable file means defaults: settings are a
    /// convenience, so a damaged file must never stop the app from starting.
    pub fn load(path: &Path) -> Self {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Writes the settings file atomically (a temp file, then a rename over the old one), so a
    /// crash mid-write leaves the previous settings intact.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        write_atomic(path, &json)
    }

    /// The folder downloads go to: the chosen one, which must still exist (an unplugged drive
    /// must not be silently recreated as an empty local folder), or `default` (the OS
    /// Downloads folder, created if needed).
    pub fn download_dir(
        &self,
        default: impl FnOnce() -> Result<PathBuf, String>,
    ) -> Result<PathBuf, String> {
        match &self.download_dir {
            Some(dir) if dir.is_dir() => Ok(dir.clone()),
            Some(dir) => Err(unavailable(dir)),
            None => default(),
        }
    }
}

/// Writes `bytes` to `path` atomically (a temp file, then a rename over the old one), so a crash
/// mid-write leaves the previous file intact. Shared by the settings (D-020) and the queue (#12).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    // A temp name of its own (process id, a counter, the time), claimed with `create_new`:
    // two Queue windows saving at once never write into each other's file.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let tmp = path.with_extension(format!(
        "json.{}-{}-{stamp}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut f| f.write_all(bytes).and_then(|()| f.sync_all()))
        .and_then(|()| fs::rename(&tmp, path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Why downloads can't go to `dir` right now: it's gone (D-020: never recreated).
pub fn unavailable(dir: &Path) -> String {
    format!(
        "The download folder {} isn't available. Choose another with Change….",
        dir.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("queue-settings-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_missing_or_damaged_file_means_defaults() {
        let dir = temp("defaults");
        let path = dir.join(FILE);
        assert_eq!(Settings::load(&path), Settings::default());
        fs::create_dir_all(&dir).unwrap();
        for junk in [
            &b"{not json"[..],
            b"[]",
            b"\xff\xfe",
            b"{\"downloadDir\": 42}",
        ] {
            fs::write(&path, junk).unwrap();
            assert_eq!(Settings::load(&path), Settings::default());
        }
        fs::write(&path, br#"{"downloadDir": "/x", "fromTheFuture": true}"#).unwrap();
        assert_eq!(
            Settings::load(&path).download_dir,
            Some(PathBuf::from("/x")),
            "unknown keys are ignored"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    /// N3 (#17): the naming template is remembered; files from before #17, or with a template
    /// from a newer version, mean `{title}`, and only that setting falls back.
    #[test]
    fn the_naming_template_is_remembered_and_unknown_values_mean_title() {
        use crate::naming::Template;
        let dir = temp("template");
        let path = dir.join(FILE);
        let dated = Settings {
            naming_template: Template::DateTitle,
            ..Default::default()
        };
        dated.save(&path).unwrap();
        assert_eq!(Settings::load(&path), dated);
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains(r#""namingTemplate": "date_title""#)
        );
        fs::write(&path, br#"{"format": "audio"}"#).unwrap();
        assert_eq!(
            Settings::load(&path).naming_template,
            Template::Title,
            "before #17"
        );
        fs::write(
            &path,
            br#"{"format": "audio", "namingTemplate": "{artist}/{title}"}"#,
        )
        .unwrap();
        let loaded = Settings::load(&path);
        assert_eq!(loaded.naming_template, Template::Title);
        assert_eq!(
            loaded.format,
            crate::download::Format::Audio,
            "the rest is kept"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_format_is_remembered_and_old_or_unknown_values_mean_default() {
        let dir = temp("format");
        let path = dir.join(FILE);
        let audio = Settings {
            format: crate::download::Format::CompatibleAudio,
            ..Default::default()
        };
        audio.save(&path).unwrap();
        assert_eq!(Settings::load(&path), audio);
        fs::write(&path, br#"{"downloadDir": "/x"}"#).unwrap();
        assert_eq!(
            Settings::load(&path).format,
            crate::download::Format::Default,
            "files from before #11"
        );
        fs::write(&path, br#"{"downloadDir": "/chosen", "format": "8k-hdr"}"#).unwrap();
        assert_eq!(
            Settings::load(&path),
            Settings {
                download_dir: Some(PathBuf::from("/chosen")),
                ..Default::default()
            },
            "an unknown format falls back alone; the chosen folder is kept"
        );
        fs::write(&path, br#"{"downloadDir": 42, "format": "audio"}"#).unwrap();
        assert_eq!(
            Settings::load(&path).format,
            crate::download::Format::Audio,
            "and the other way round"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saved_settings_round_trip_with_unicode_paths_and_leave_no_temp_file() {
        let dir = temp("roundtrip");
        let path = dir.join("nested").join(FILE);
        let settings = Settings {
            download_dir: Some(dir.join("Vidéos 日本語 🎵")),
            ..Default::default()
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), settings);
        Settings::default().save(&path).unwrap();
        assert_eq!(
            Settings::load(&path),
            Settings::default(),
            "replaced, not appended"
        );
        let names: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [FILE]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_saves_leave_one_complete_choice_and_no_temp_files() {
        let dir = temp("concurrent");
        let path = dir.join(FILE);
        let choices: Vec<_> = (0..16)
            .map(|n| Settings {
                download_dir: Some(dir.join(format!("folder {n} {}", "x".repeat(4096)))),
                ..Default::default()
            })
            .collect();
        std::thread::scope(|scope| {
            for settings in &choices {
                let path = &path;
                scope.spawn(move || {
                    for _ in 0..20 {
                        settings.save(path).unwrap();
                    }
                });
            }
        });
        let saved = Settings::load(&path);
        assert!(choices.contains(&saved), "a complete, submitted choice");
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [FILE]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_download_dir_is_the_chosen_folder_only_while_it_exists() {
        let dir = temp("dir");
        fs::create_dir_all(&dir).unwrap();
        let default = || Ok(PathBuf::from("/default"));
        assert_eq!(
            Settings::default().download_dir(default).unwrap(),
            PathBuf::from("/default")
        );
        let chosen = Settings {
            download_dir: Some(dir.clone()),
            ..Default::default()
        };
        assert_eq!(chosen.download_dir(default).unwrap(), dir);
        fs::remove_dir_all(&dir).unwrap();
        let err = chosen.download_dir(default).unwrap_err();
        assert!(err.contains("isn't available"), "{err}");
        assert!(!dir.exists(), "never recreated");
        assert_eq!(
            Settings::default()
                .download_dir(|| Err("no Downloads".into()))
                .unwrap_err(),
            "no Downloads"
        );
    }
}
