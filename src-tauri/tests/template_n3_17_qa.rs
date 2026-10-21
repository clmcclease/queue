//! Independent QA for #17 / N3: template settings, queue lifecycle, and safe output names.
use queue_lib::{
    download::{self, Format, Source},
    meta::Meta,
    naming::{self, Template},
    queue::{NameSource, Queue, Status},
    settings::Settings,
};
use std::{ffi::OsStr, fs, path::PathBuf};

const TEMPLATES: [Template; 3] = [Template::Title, Template::ChannelTitle, Template::DateTitle];

fn metadata() -> Meta {
    Meta {
        title: Some("  🔥 Café 日本語 [4K] (Official Video)  ".into()),
        channel: Some(" Créateur [HD] ".into()),
        upload_date: Some("20240229".into()),
        ..Default::default()
    }
}

fn add_ready(q: &mut Queue, suffix: &str, meta: Meta) -> u64 {
    let id = q.add(&format!("https://example.test/{suffix}")).unwrap().id;
    q.fetched(id, Ok(meta)).unwrap();
    id
}

struct Temp(PathBuf);
impl Temp {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("queue-n3-17-{label}-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn every_template_round_trips_without_changing_other_settings() {
    let temp = Temp::new("settings");
    let file = temp.0.join("settings.json");
    for template in TEMPLATES {
        let settings = Settings {
            download_dir: Some(temp.0.join("Téléchargements 日本語")),
            format: Format::CompatibleAudio,
            naming_template: template,
            naming_provider: queue_lib::ai::Provider::Off, // #25
        };
        settings.save(&file).unwrap();
        assert_eq!(Settings::load(&file), settings);
    }
    for bad in [
        serde_json::Value::Null,
        serde_json::json!(17),
        serde_json::json!(false),
        serde_json::json!([]),
        serde_json::json!({"template":"title"}),
        serde_json::json!("../../%(title)s"),
    ] {
        let input = serde_json::json!({
            "downloadDir":temp.0, "format":"audio", "namingTemplate":bad,
        });
        fs::write(&file, serde_json::to_vec(&input).unwrap()).unwrap();
        let loaded = Settings::load(&file);
        assert_eq!(loaded.naming_template, Template::Title, "{bad}");
        assert_eq!(loaded.format, Format::Audio);
        assert_eq!(loaded.download_dir, Some(temp.0.clone()));
    }
}

#[test]
fn queued_names_change_with_one_revision_each_while_every_other_state_is_preserved() {
    for state in [
        "fetching",
        "downloading",
        "processing",
        "done",
        "failed",
        "edited",
    ] {
        let mut q = Queue::default();
        let queued = add_ready(&mut q, "queued", metadata());
        let protected = if state == "fetching" {
            q.add("https://example.test/fetching").unwrap().id
        } else {
            add_ready(&mut q, "protected", metadata())
        };
        match state {
            "edited" => {
                q.rename(protected, "My Café 日本語").unwrap();
            }
            "fetching" => {}
            _ => {
                q.start(protected).unwrap();
                match state {
                    "processing" => {
                        q.processing(protected).unwrap();
                    }
                    "done" => {
                        q.finish(protected, Ok("saved.webm".into())).unwrap();
                    }
                    "failed" => {
                        q.finish(protected, Err("timeout".into())).unwrap();
                    }
                    _ => {}
                }
            }
        }
        let before = q.items().to_vec();
        let changes = q.changes();
        let changed = q.set_template(Template::ChannelTitle);
        assert_eq!(changed.len(), 1, "{state}");
        assert_eq!(changed[0].id, queued);
        assert_eq!(changed[0].name.as_deref(), Some("Créateur - Café 日本語"));
        assert_eq!(changed[0].name_source, Some(NameSource::Rules));
        assert_eq!(changed[0].rev, before[0].rev + 1);
        assert_eq!(changed[0].meta, before[0].meta);
        assert_eq!(q.items()[1], before[1], "{state}");
        assert_eq!(q.changes(), changes + 1);
        let stable = q.items().to_vec();
        assert!(q.set_template(Template::ChannelTitle).is_empty());
        assert_eq!(q.items(), stable);
        assert_eq!(q.changes(), changes + 1);
    }
}

#[test]
fn selection_on_an_empty_queue_and_pending_fetches_uses_the_latest_template() {
    let mut q = Queue::default();
    assert!(q.set_template(Template::ChannelTitle).is_empty());
    let pending = q.add("https://example.test/pending").unwrap().id;
    assert!(q.set_template(Template::DateTitle).is_empty());
    assert!(q.items()[0].name.is_none());
    let row = q.fetched(pending, Ok(metadata())).unwrap();
    assert_eq!(row.name.as_deref(), Some("2024-02-29 Café 日本語"));
    assert_eq!(row.name_source, Some(NameSource::Rules));
    let next = add_ready(&mut q, "new", metadata());
    assert_eq!(q.start(next).unwrap().name, row.name);
}

#[test]
fn missing_or_unusable_template_fields_do_not_create_spurious_events() {
    let mut q = Queue::default();
    add_ready(
        &mut q,
        "missing",
        Meta {
            title: Some("Café".into()),
            ..Default::default()
        },
    );
    add_ready(
        &mut q,
        "noise",
        Meta {
            title: Some("Café".into()),
            channel: Some("🔥 [HD]".into()),
            upload_date: Some("not a date".into()),
            ..Default::default()
        },
    );
    let before = (q.items().to_vec(), q.changes());
    for template in TEMPLATES {
        assert!(q.set_template(template).is_empty());
        assert_eq!((q.items().to_vec(), q.changes()), before);
    }
}

#[test]
fn restart_applies_saved_template_to_rules_and_interrupted_rows_but_keeps_edits_and_done() {
    let temp = Temp::new("restart");
    let file = temp.0.join("settings.json");
    Settings {
        naming_template: Template::DateTitle,
        ..Default::default()
    }
    .save(&file)
    .unwrap();
    let mut q = Queue::default();
    let plain = add_ready(&mut q, "plain", metadata());
    let edited = add_ready(&mut q, "edited", metadata());
    let done = add_ready(&mut q, "done", metadata());
    let interrupted = add_ready(&mut q, "interrupted", metadata());
    q.rename(edited, "NUL").unwrap();
    q.start(done).unwrap();
    q.finish(done, Ok("saved.webm".into())).unwrap();
    q.start(interrupted).unwrap();
    q.processing(interrupted).unwrap();
    let mut restored = Queue::from_json(&q.to_json()).unwrap();
    let changed = restored.set_template(Settings::load(&file).naming_template);
    assert_eq!(
        changed.iter().map(|i| i.id).collect::<Vec<_>>(),
        [plain, interrupted]
    );
    assert_eq!(
        restored.items()[0].name.as_deref(),
        Some("2024-02-29 Café 日本語")
    );
    assert_eq!(restored.items()[1].name.as_deref(), Some("NUL_"));
    assert_eq!(restored.items()[1].name_source, Some(NameSource::Edited));
    assert_eq!(restored.items()[2].name, q.items()[2].name);
    assert_eq!(restored.items()[2].status, q.items()[2].status);
    assert_eq!(restored.items()[3].status, Status::Queued);
    assert_eq!(
        restored.items()[3].notice.as_deref(),
        Some(queue_lib::queue::INTERRUPTED)
    );
}

#[test]
fn selected_templates_keep_unicode_safe_names_and_literal_output_arguments() {
    for title in [
        "CON",
        "NuL.txt",
        "COM¹",
        "🔥 (Official Video)",
        "../a\\b:*?",
        "e\u{301} Café 日本語",
        "100% %(title)s $(touch nope)",
    ] {
        for template in TEMPLATES {
            let mut q = Queue::default();
            q.set_template(template);
            let id = add_ready(
                &mut q,
                "video",
                Meta {
                    title: Some(title.into()),
                    ..Default::default()
                },
            );
            let name = q.start(id).unwrap().name.unwrap();
            assert_eq!(
                name,
                naming::name(
                    &Meta {
                        title: Some(title.into()),
                        ..Default::default()
                    },
                    template
                )
            );
            assert_eq!(naming::sanitize(&name), name);
            assert!(
                !name
                    .chars()
                    .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
            );
            let args = download::args_named(
                &PathBuf::from("bin"),
                &PathBuf::from("work"),
                Source::Url("https://example.test/video"),
                Format::Default,
                Some(&name),
            );
            let outputs: Vec<_> = args.windows(2).filter(|pair| pair[0] == "-o").collect();
            assert_eq!(outputs.len(), 1);
            assert_eq!(
                outputs[0][1],
                format!("{}.%(ext)s", name.replace('%', "%%")).as_str()
            );
        }
    }
    for template in TEMPLATES {
        let mut q = Queue::default();
        add_ready(
            &mut q,
            "long",
            Meta {
                title: Some("日".repeat(151)),
                channel: Some("é".repeat(151)),
                ..metadata()
            },
        );
        q.set_template(template);
        let name = q.items()[0].name.as_ref().unwrap();
        assert!(name.len() <= naming::MAX_BYTES && name.chars().count() <= naming::MAX_CHARS);
        assert_eq!(&naming::sanitize(name), name);
    }
}

#[test]
fn switching_templates_can_merge_stems_without_overwriting_existing_files() {
    let temp = Temp::new("collisions");
    let out = temp.0.join("out");
    fs::create_dir(&out).unwrap();
    let mut q = Queue::default();
    q.set_template(Template::ChannelTitle);
    for (i, channel) in ["A", "B", "C"].into_iter().enumerate() {
        add_ready(
            &mut q,
            &i.to_string(),
            Meta {
                channel: Some(channel.into()),
                ..metadata()
            },
        );
    }
    assert_eq!(q.set_template(Template::Title).len(), 3);
    for (i, row) in q.items().iter().enumerate() {
        let source = temp.0.join(format!("source-{i}"));
        fs::write(&source, format!("payload-{i}")).unwrap();
        let filename = format!("{}.webm", row.name.as_ref().unwrap());
        download::place_unique(&source, &out, OsStr::new(&filename)).unwrap();
    }
    for (i, file) in [
        "Café 日本語.webm",
        "Café 日本語 (2).webm",
        "Café 日本語 (3).webm",
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            fs::read_to_string(out.join(file)).unwrap(),
            format!("payload-{i}")
        );
    }
    assert_eq!(fs::read_dir(&out).unwrap().count(), 3);
}
