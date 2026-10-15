pub mod download;
pub mod link;
pub mod meta;
pub mod proc;
pub mod queue;
pub mod reveal;
pub mod settings;
pub mod sidecar;

use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use queue::{AddError, Item, Queue};
use tauri::{AppHandle, Emitter, Manager};

/// Emitted with the updated [`Item`] whenever background work (metadata, download) changes it.
const ITEM_EVENT: &str = "queue://item";

/// How long one `yt-dlp -J` may take before it's killed and the row fails.
const FETCH_LIMIT: Duration = Duration::from_secs(90);

/// Rows waiting for metadata. One worker fetches them in order, so a burst of pastes doesn't
/// start a burst of yt-dlp processes.
struct Fetcher(Mutex<Sender<(u64, String)>>);

fn fetch_worker(app: AppHandle, jobs: Receiver<(u64, String)>) {
    let info_dir = app
        .path()
        .app_cache_dir()
        .ok()
        .map(|d| meta::session_dir(&d));
    for (id, url) in jobs {
        let state = app.state::<Mutex<Queue>>();
        let updated = run_fetch_job(&state, id, || {
            let (meta, raw) = sidecar::dir()
                .map_err(|e| format!("Couldn't locate yt-dlp: {e}"))
                .and_then(|dir| meta::fetch_raw(&dir, &url, FETCH_LIMIT))?;
            // Keep the -J for the download (D-017). Best effort: without it, the download just
            // extracts again.
            let saved = info_dir
                .as_ref()
                .and_then(|dir| meta::save_info(dir, id, &raw).ok());
            Ok((meta, saved))
        });
        if let Some(item) = updated {
            let _ = app.emit(ITEM_EVENT, item);
        }
    }
}

/// One metadata job: runs `fetch` and records the outcome. A panic inside `fetch` fails just
/// this row (release builds unwind, see Cargo.toml), so the worker keeps serving later rows.
/// The saved info is attached under the same lock that makes the row Queued, so a Download can
/// never start in between without it.
pub fn run_fetch_job(
    queue: &Mutex<Queue>,
    id: u64,
    fetch: impl FnOnce() -> Result<(meta::Meta, Option<std::path::PathBuf>), String>,
) -> Option<Item> {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(fetch))
        .unwrap_or_else(|_| Err("Getting the video's info failed unexpectedly.".into()));
    let mut q = lock(queue).ok()?;
    let mut saved_path = None;
    let outcome = outcome.map(|(meta, saved)| {
        if let Some(path) = saved {
            q.set_info(id, path.clone(), std::time::SystemTime::now());
            saved_path = Some(path);
        }
        meta
    });
    let updated = q.fetched(id, outcome);
    if updated.is_none()
        && let Some(path) = saved_path
    {
        let _ = std::fs::remove_file(path); // the row was removed meanwhile (Q6)
    }
    updated
}

/// Hands a new row to the metadata worker. If the worker is gone, the row fails now instead of
/// staying "Fetching info…" forever.
pub fn dispatch_fetch(
    queue: &Mutex<Queue>,
    jobs: &Mutex<Sender<(u64, String)>>,
    item: Item,
) -> Item {
    let sent = jobs
        .lock()
        .map(|jobs| jobs.send((item.id, item.url.clone())).is_ok())
        .unwrap_or(false);
    if sent {
        return item;
    }
    let reason = "Couldn't start getting the video's info. Restart Queue.".to_string();
    lock(queue)
        .ok()
        .and_then(|mut q| q.fetched(item.id, Err(reason)))
        .unwrap_or(item)
}

pub fn lock(queue: &Mutex<Queue>) -> Result<std::sync::MutexGuard<'_, Queue>, String> {
    queue.lock().map_err(|_| "queue lock poisoned".to_string())
}

#[tauri::command]
fn queue_add(
    queue: tauri::State<'_, Mutex<Queue>>,
    fetcher: tauri::State<'_, Fetcher>,
    url: String,
) -> Result<Item, AddError> {
    let item = lock(&queue)
        .map_err(|message| AddError::Invalid { message })?
        .add(&url)?;
    // The row shows "Fetching info…" until the worker reports back via ITEM_EVENT.
    Ok(dispatch_fetch(&queue, &fetcher.0, item))
}

#[tauri::command]
fn queue_list(queue: tauri::State<'_, Mutex<Queue>>) -> Result<Vec<Item>, String> {
    Ok(lock(&queue)?.items().to_vec())
}

/// Removes a row that isn't downloading (Q6), and deletes its saved info. A Done row's file stays.
#[tauri::command]
fn queue_remove(queue: tauri::State<'_, Mutex<Queue>>, id: u64) -> Result<(), String> {
    let mut removed = lock(&queue)?.remove(id)?;
    if let Some(path) = removed.take_info() {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

/// Retries a Failed row (Q6). A row whose info fetch failed goes back to the metadata worker;
/// otherwise it's Queued again for Download or "Download all".
#[tauri::command]
fn queue_retry(
    queue: tauri::State<'_, Mutex<Queue>>,
    fetcher: tauri::State<'_, Fetcher>,
    id: u64,
) -> Result<Item, String> {
    let item = lock(&queue)?.retry(id)?;
    Ok(match item.status {
        queue::Status::Fetching => dispatch_fetch(&queue, &fetcher.0, item),
        _ => item,
    })
}

/// The user's settings and the file they're saved in (D1). `file` is `None` if the OS has no
/// config folder; settings then last until the app quits.
struct Settings {
    file: Option<std::path::PathBuf>,
    current: Mutex<settings::Settings>,
}

/// Where finished downloads go (D1): the chosen folder, or the OS Downloads folder.
fn download_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let settings = app.state::<Settings>();
    let current = settings
        .current
        .lock()
        .map_err(|_| "settings lock poisoned".to_string())?
        .clone();
    current.download_dir(|| {
        let dir = app
            .path()
            .download_dir()
            .map_err(|e| format!("Couldn't find your Downloads folder: {e}"))?;
        // The default folder is the one Queue may create (D-020); a chosen one never is.
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
        Ok(dir)
    })
}

/// A copy of the current settings.
fn current_settings(app: &AppHandle) -> Result<settings::Settings, String> {
    app.state::<Settings>()
        .current
        .lock()
        .map(|s| s.clone())
        .map_err(|_| "settings lock poisoned".to_string())
}

/// The download format (D2, #11).
#[tauri::command]
fn settings_format(app: AppHandle) -> Result<download::Format, String> {
    Ok(current_settings(&app)?.format)
}

/// Saves the download format (#11). Downloads already running keep the one they started with.
#[tauri::command]
fn settings_set_format(
    app: AppHandle,
    format: download::Format,
) -> Result<download::Format, String> {
    let state = app.state::<Settings>();
    let mut current = state
        .current
        .lock()
        .map_err(|_| "settings lock poisoned".to_string())?;
    let mut next = current.clone();
    next.format = format;
    if let Some(file) = &state.file {
        next.save(file)
            .map_err(|e| format!("Couldn't save your settings: {e}"))?;
    }
    *current = next;
    Ok(format)
}

/// The download folder as the UI shows it. `error` says why it can't be used right now (a
/// chosen folder that's gone), while `path` still names it.
#[derive(Clone, Debug, serde::Serialize)]
struct DownloadFolder {
    path: String,
    error: Option<String>,
}

fn download_folder(app: &AppHandle) -> DownloadFolder {
    let chosen = app
        .state::<Settings>()
        .current
        .lock()
        .ok()
        .and_then(|s| s.download_dir.clone());
    match download_dir(app) {
        Ok(dir) => DownloadFolder {
            path: dir.to_string_lossy().into_owned(),
            error: None,
        },
        Err(error) => DownloadFolder {
            path: chosen
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_default(),
            error: Some(error),
        },
    }
}

/// The current download folder (D1).
#[tauri::command]
fn settings_download_folder(app: AppHandle) -> DownloadFolder {
    download_folder(&app)
}

/// Opens the OS folder picker at the current download folder and saves the choice (D1).
/// `None` if the picker was cancelled. Async, so the blocking picker never runs on the main
/// thread. Downloads already running keep the folder they started with.
#[tauri::command]
async fn settings_choose_download_folder(app: AppHandle) -> Result<Option<DownloadFolder>, String> {
    use tauri_plugin_dialog::DialogExt;
    let mut picker = app.dialog().file().set_title("Choose the download folder");
    if let Ok(dir) = download_dir(&app) {
        picker = picker.set_directory(dir);
    }
    let Some(picked) = picker.blocking_pick_folder() else {
        return Ok(None);
    };
    let dir = picked
        .into_path()
        .map_err(|e| format!("Couldn't use that folder: {e}"))?;
    let state = app.state::<Settings>();
    let mut current = state
        .current
        .lock()
        .map_err(|_| "settings lock poisoned".to_string())?;
    let mut next = current.clone();
    next.download_dir = Some(dir);
    if let Some(file) = &state.file {
        next.save(file)
            .map_err(|e| format!("Couldn't save your settings: {e}"))?;
    }
    *current = next;
    drop(current);
    Ok(Some(download_folder(&app)))
}

/// Runs blocking file-manager and filesystem work off the main thread, so a slow network
/// folder or an unresponsive file manager (Linux D-Bus) can't freeze the window.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| format!("Couldn't finish that: {e}"))?
}

/// Opens the download folder in the OS file manager (D7). No path comes from the UI: it's always
/// the current download folder. Queue's own launchers (`reveal`) report failures; the opener
/// plugin is never registered, so the webview gets no opener permission (D-022).
#[tauri::command]
async fn open_download_folder(app: AppHandle) -> Result<(), String> {
    blocking(move || reveal::open_folder(&download_dir(&app)?)).await
}

/// Reveals (selects) a finished row's file in the OS file manager (D7). Takes a row id, not a
/// path, so only files Queue saved can be shown. Returns a notice when only the file's folder
/// could be opened (Linux without a file manager that can select files).
#[tauri::command]
async fn reveal_download(app: AppHandle, id: u64) -> Result<Option<String>, String> {
    let file = lock(&app.state::<Mutex<Queue>>())?.saved_file(id)?; // lock released here
    blocking(move || {
        if !file.is_file() {
            return Err(format!(
                "{} isn't there anymore. It may have been moved or deleted.",
                file.file_name().unwrap_or_default().to_string_lossy()
            ));
        }
        Ok(match reveal::reveal(&file)? {
            reveal::Shown::Selected => None,
            reveal::Shown::FolderOnly => Some(format!(
                "Opened the folder, but your file manager couldn't select {}.",
                file.file_name().unwrap_or_default().to_string_lossy()
            )),
        })
    })
    .await
}

/// Everything a started download needs, captured while the queue was locked.
struct DownloadJob {
    id: u64,
    url: String,
    info: Option<std::path::PathBuf>,
    sidecar_dir: std::path::PathBuf,
    work_dir: std::path::PathBuf,
    out_dir: std::path::PathBuf,
    /// The format chosen in Settings when the download started (#11).
    format: download::Format,
}

/// Marks a queued item Downloading (one at a time, see `Queue::start`) and prepares its paths:
/// the download folder (D1) and a fresh staging dir under the app cache.
fn begin_download(
    app: &AppHandle,
    queue: &Mutex<Queue>,
    id: u64,
) -> Result<(Item, DownloadJob), String> {
    let out_dir = download_dir(app)?;
    let format = current_settings(app)?.format;
    let sidecar_dir = sidecar::dir().map_err(|e| format!("Couldn't locate yt-dlp: {e}"))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let work_dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("Couldn't find the app cache folder: {e}"))?
        .join("downloads")
        .join(format!("item-{id}-{stamp}"));
    let mut q = lock(queue)?;
    let item = q.start(id)?;
    let info = q.fresh_info(id, std::time::SystemTime::now());
    let job = DownloadJob {
        id,
        url: item.url.clone(),
        info,
        sidecar_dir,
        work_dir,
        out_dir,
        format,
    };
    Ok((item, job))
}

/// Runs a started download to the end (blocking): 403 retries with a pause (D-017), then the
/// outcome is recorded, the saved info deleted, and the row emitted.
fn complete_download(app: &AppHandle, job: DownloadJob) {
    let state = app.state::<Mutex<Queue>>();
    let id = job.id;
    let emit = |updated: Option<Item>| {
        if let Some(item) = updated {
            let _ = app.emit(ITEM_EVENT, item);
        }
    };
    // Progress arrives already throttled (download::PROGRESS_EVERY); state changes always do.
    let outcome = download::run_with_retries_saved(
        &job.sidecar_dir,
        &job.work_dir,
        &job.out_dir,
        &job.url,
        job.info.as_deref(),
        job.format,
        std::thread::sleep,
        |attempt| emit(lock(&state).ok().and_then(|mut q| q.retrying(id, attempt))),
        |event| match event {
            download::Event::Progress(p) => {
                emit(lock(&state).ok().and_then(|mut q| q.progressed(id, p)));
            }
            download::Event::Processing(_) => {
                emit(lock(&state).ok().and_then(|mut q| q.processing(id)));
            }
        },
    );
    // A "most compatible" format the site didn't have gets an honest notice (D-015), decided
    // from the codecs yt-dlp chose.
    let notice = outcome.as_ref().ok().and_then(|saved| {
        job.format
            .notice_for(std::path::Path::new(&saved.path), &saved.codecs)
    });
    let outcome = outcome.map(|saved| saved.path);
    let (updated, info) = match lock(&state) {
        Ok(mut q) => {
            let finished = q.finish(id, outcome);
            let updated = match notice {
                Some(notice) => q.noticed(id, notice).or(finished),
                None => finished,
            };
            (updated, q.take_info(id))
        }
        Err(_) => (None, None),
    };
    if let Some(path) = info {
        let _ = std::fs::remove_file(path);
    }
    if let Some(item) = updated {
        let _ = app.emit(ITEM_EVENT, item);
    }
}

/// Starts downloading one queued item on a background thread. Returns the item as
/// `Downloading`; the outcome arrives as [`ITEM_EVENT`].
#[tauri::command]
fn queue_download(
    app: AppHandle,
    queue: tauri::State<'_, Mutex<Queue>>,
    id: u64,
) -> Result<Item, String> {
    let (item, job) = begin_download(&app, &queue, id)?;
    std::thread::spawn(move || complete_download(&app, job));
    Ok(item)
}

/// Emitted with `{ running }` when "Download all" starts or finishes.
const RUNNER_EVENT: &str = "queue://runner";

/// Whether "Download all" is running.
struct Runner(Mutex<bool>);

#[derive(Clone, serde::Serialize)]
struct RunnerState {
    running: bool,
}

/// Sets the runner flag and emits the change under one lock, so `queue://runner` events always
/// reach the UI in the order the state changed (a stale "finished" can't follow a newer "started").
fn set_running(app: &AppHandle, runner: &Runner, running: bool) -> bool {
    let Ok(mut flag) = runner.0.lock() else {
        return false;
    };
    if running && *flag {
        return false; // already running
    }
    *flag = running;
    let _ = app.emit(RUNNER_EVENT, RunnerState { running });
    true
}

/// "Download all" (Q4): downloads every Queued row in order, one at a time, until none are left.
/// Returns false if a run is already in progress, or an error if downloads can't work at all
/// (no download or cache folder, no yt-dlp): reported once instead of failing every row.
#[tauri::command]
fn queue_run_all(app: AppHandle, runner: tauri::State<'_, Runner>) -> Result<bool, String> {
    download_dir(&app)?;
    app.path()
        .app_cache_dir()
        .map_err(|e| format!("Couldn't find the app cache folder: {e}"))?;
    sidecar::dir().map_err(|e| format!("Couldn't locate yt-dlp: {e}"))?;
    if !set_running(&app, &runner, true) {
        return Ok(false);
    }
    std::thread::spawn(move || {
        let state = app.state::<Mutex<Queue>>();
        // Whatever happens in the run, the flag is reset (release builds unwind, see Cargo.toml).
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            queue::run_all(
                &state,
                |id| {
                    if lock(&state).map(|q| q.is_busy()).unwrap_or(true) {
                        return false; // a manual Download holds the slot: wait
                    }
                    match begin_download(&app, &state, id) {
                        Ok((item, job)) => {
                            let _ = app.emit(ITEM_EVENT, item);
                            complete_download(&app, job);
                        }
                        Err(_) if lock(&state).map(|q| q.is_busy()).unwrap_or(true) => {
                            return false; // lost a race for the slot: wait
                        }
                        Err(reason) => {
                            // A lasting reason: fail this row and move on.
                            let failed = lock(&state)
                                .ok()
                                .and_then(|mut q| q.fail_queued(id, reason));
                            if let Some(item) = failed {
                                let _ = app.emit(ITEM_EVENT, item);
                            }
                        }
                    }
                    true
                },
                || std::thread::sleep(Duration::from_millis(250)),
            );
        }));
        set_running(&app, &app.state::<Runner>(), false);
    });
    Ok(true)
}

/// Whether "Download all" is running (for the UI's initial state).
#[tauri::command]
fn queue_runner_state(runner: tauri::State<'_, Runner>) -> bool {
    runner.0.lock().map(|flag| *flag).unwrap_or(false)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(Mutex::new(Queue::default()))
        .manage(Runner(Mutex::new(false)))
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let file = app
                .path()
                .app_config_dir()
                .ok()
                .map(|d| d.join(settings::FILE));
            let current = file
                .as_deref()
                .map(settings::Settings::load)
                .unwrap_or_default();
            app.manage(Settings {
                file,
                current: Mutex::new(current),
            });
            let (jobs, queued) = mpsc::channel();
            app.manage(Fetcher(Mutex::new(jobs)));
            let handle = app.handle().clone();
            std::thread::spawn(move || fetch_worker(handle, queued));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            queue_add,
            queue_list,
            queue_download,
            queue_remove,
            queue_retry,
            queue_run_all,
            queue_runner_state,
            settings_download_folder,
            settings_choose_download_folder,
            settings_format,
            settings_set_format,
            open_download_folder,
            reveal_download
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use meta::Meta;
    use queue::Status;

    #[test]
    fn a_fetch_finishing_after_its_row_was_removed_deletes_the_saved_info() {
        let queue = Mutex::new(Queue::default());
        let id = queue.lock().unwrap().add("https://e.test/gone").unwrap().id;
        let dir = std::env::temp_dir().join(format!("queue-q6-removed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let saved = dir.join("item.json");
        std::fs::write(&saved, b"{}").unwrap();
        queue.lock().unwrap().remove(id).unwrap();
        let updated = run_fetch_job(&queue, id, || Ok((Meta::default(), Some(saved.clone()))));
        assert_eq!(updated, None);
        assert!(!saved.exists(), "no orphaned info file");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_panicking_job_fails_its_row_and_the_next_job_still_works() {
        let queue = Mutex::new(Queue::default());
        let a = queue
            .lock()
            .unwrap()
            .add("https://example.com/a")
            .unwrap()
            .id;
        let b = queue
            .lock()
            .unwrap()
            .add("https://example.com/b")
            .unwrap()
            .id;
        let failed = run_fetch_job(&queue, a, || panic!("boom")).unwrap();
        assert!(matches!(failed.status, Status::Failed { .. }));
        let ok = run_fetch_job(&queue, b, || Ok((Meta::default(), None))).unwrap();
        assert_eq!(ok.status, Status::Queued);
    }

    #[test]
    fn a_dispatch_with_no_worker_fails_the_row_instead_of_leaving_it_fetching() {
        let queue = Mutex::new(Queue::default());
        let (jobs, worker) = mpsc::channel();
        drop(worker); // the worker thread is gone
        let item = queue.lock().unwrap().add("https://example.com/a").unwrap();
        let item = dispatch_fetch(&queue, &Mutex::new(jobs), item);
        assert!(matches!(item.status, Status::Failed { ref reason } if reason.contains("Restart")));
        let (jobs, worker) = mpsc::channel();
        let next = queue.lock().unwrap().add("https://example.com/b").unwrap();
        assert_eq!(
            dispatch_fetch(&queue, &Mutex::new(jobs), next).status,
            Status::Fetching
        );
        assert_eq!(worker.recv().unwrap().1, "https://example.com/b");
    }

    #[test]
    fn saved_info_is_attached_before_the_row_becomes_queued() {
        let queue = Mutex::new(Queue::default());
        let id = queue
            .lock()
            .unwrap()
            .add("https://example.com/a")
            .unwrap()
            .id;
        let file = std::env::temp_dir().join(format!("queue-attach-{}.json", std::process::id()));
        std::fs::write(&file, "{}").unwrap();
        let item = run_fetch_job(&queue, id, || Ok((Meta::default(), Some(file.clone())))).unwrap();
        assert_eq!(item.status, Status::Queued);
        let fresh = queue
            .lock()
            .unwrap()
            .fresh_info(id, std::time::SystemTime::now());
        assert_eq!(
            fresh,
            Some(file.clone()),
            "Queued and info visible together"
        );
        std::fs::remove_file(file).unwrap();
    }
}
