pub mod ai;
pub mod download;
pub mod link;
pub mod meta;
pub mod naming;
pub mod proc;
pub mod queue;
pub mod reveal;
pub mod settings;
pub mod sidecar;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
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
        // Counted until recorded, so quitting saves after it (#67); none start once quitting.
        let Some(_running) = proc::job() else {
            return;
        };
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
            // N4 (#25): a row with its info has its Tier-1 name already; an AI name may follow.
            if item.status == queue::Status::Queued {
                ask_for_name(&app, item.id);
            }
            let _ = app.emit(ITEM_EVENT, item);
        }
    }
}

/// Rows waiting for an AI name (#25). One worker asks for them in order.
struct Naming(Mutex<Sender<u64>>);

/// Hands a row to the naming worker, if the chosen provider can be asked. Nothing is sent for
/// an unavailable one (D-029): the row keeps its Tier-1 name with no tag.
fn ask_for_name(app: &AppHandle, id: u64) {
    let usable = current_settings(app).is_ok_and(|s| ai::endpoint(s.naming_provider).is_ok());
    if usable && let Ok(jobs) = app.state::<Naming>().0.lock() {
        let _ = jobs.send(id);
    }
}

/// Asks for AI names one row at a time. It doesn't hold a quit up (#67): an answer lost at quit
/// just leaves the Tier-1 name. The provider is read for each row, so a change applies at once.
fn naming_worker(app: AppHandle, jobs: Receiver<u64>) {
    for id in jobs {
        if proc::stopping() {
            return;
        }
        let Ok(settings) = current_settings(&app) else {
            continue;
        };
        let Ok(endpoint) = ai::endpoint(settings.naming_provider) else {
            continue;
        };
        let state = app.state::<Mutex<Queue>>();
        if let Some(item) = run_naming_job(&state, id, &endpoint) {
            let _ = app.emit(ITEM_EVENT, item);
        }
    }
}

/// One naming job (#25): takes what it needs under the queue lock, asks `namer` with the lock
/// released (it can take seconds), then records the answer, which [`Queue::ai_named`] drops if
/// the row or the provider changed meanwhile. A panicking namer fails just this row.
pub fn run_naming_job(queue: &Mutex<Queue>, id: u64, namer: &dyn ai::Namer) -> Option<Item> {
    let (meta, generation) = lock(queue).ok()?.naming_job(id)?;
    let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| namer.name(&meta)))
        .unwrap_or_else(|_| Err("Naming failed unexpectedly.".into()));
    lock(queue).ok()?.ai_named(id, generation, answer)
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
    // A failure while quitting is the quit stopping it (#67): the row stays Fetching and
    // fetches again after a restart. A finished fetch is still recorded.
    if outcome.is_err() && proc::stopping() {
        return None;
    }
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

/// Renames a Queued row (N1, #15); the name is sanitized and returned as the updated row.
#[tauri::command]
fn queue_rename(
    queue: tauri::State<'_, Mutex<Queue>>,
    id: u64,
    name: String,
) -> Result<Item, String> {
    lock(&queue)?.rename(id, &name)
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

/// The AI naming provider (N4, #25).
#[tauri::command]
fn settings_naming_provider(app: AppHandle) -> Result<ai::Provider, String> {
    Ok(current_settings(&app)?.naming_provider)
}

/// Saves the AI naming provider (#25). Answers already on their way are dropped, and if the new
/// provider can be asked, every Queued row not renamed by hand is asked again.
#[tauri::command]
fn settings_set_naming_provider(
    app: AppHandle,
    provider: ai::Provider,
) -> Result<NamingStatus, String> {
    {
        let state = app.state::<Settings>();
        let mut current = state
            .current
            .lock()
            .map_err(|_| "settings lock poisoned".to_string())?;
        let mut next = current.clone();
        next.naming_provider = provider;
        if let Some(file) = &state.file {
            next.save(file)
                .map_err(|e| format!("Couldn't save your settings: {e}"))?;
        }
        *current = next;
    }
    let again = lock(&app.state::<Mutex<Queue>>())?.renaming();
    for id in again {
        ask_for_name(&app, id);
    }
    naming_status(app)
}

/// The chosen provider and why it can't be asked yet, if it can't (the banner, PRD §6.3).
#[derive(Clone, Debug, serde::Serialize)]
struct NamingStatus {
    provider: ai::Provider,
    unavailable: Option<ai::Unavailable>,
}

#[tauri::command]
fn naming_status(app: AppHandle) -> Result<NamingStatus, String> {
    let provider = current_settings(&app)?.naming_provider;
    Ok(NamingStatus {
        provider,
        unavailable: ai::endpoint(provider).err(),
    })
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

/// The naming template (N3, #17).
#[tauri::command]
fn settings_naming_template(app: AppHandle) -> Result<naming::Template, String> {
    Ok(current_settings(&app)?.naming_template)
}

/// Saves the naming template (#17), then renames the Queued rows whose name wasn't edited and
/// sends them as [`ITEM_EVENT`]s. Downloads already running keep the name they started with.
#[tauri::command]
fn settings_set_naming_template(
    app: AppHandle,
    template: naming::Template,
) -> Result<naming::Template, String> {
    {
        let state = app.state::<Settings>();
        let mut current = state
            .current
            .lock()
            .map_err(|_| "settings lock poisoned".to_string())?;
        let mut next = current.clone();
        next.naming_template = template;
        if let Some(file) = &state.file {
            next.save(file)
                .map_err(|e| format!("Couldn't save your settings: {e}"))?;
        }
        *current = next;
    }
    let changed = lock(&app.state::<Mutex<Queue>>())?.set_template(template);
    for item in changed {
        let _ = app.emit(ITEM_EVENT, item);
    }
    Ok(template)
}

/// Each template's name for a fixed sample item: the live preview in Settings (N3, #17).
#[tauri::command]
fn naming_previews() -> Vec<(naming::Template, String)> {
    naming::previews()
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

/// The queue's file in the app data folder (#12).
const QUEUE_FILE: &str = "queue.json";

/// How often the queue is saved if it changed.
const SAVE_EVERY: Duration = Duration::from_millis(500);

/// How long quitting waits for downloads and fetches to stop and record their results (#67): a
/// tree gets up to 3 s after SIGTERM before SIGKILL, and 1 s to be gone (proc).
const QUIT_WAIT: Duration = Duration::from_secs(5);

/// Where the queue is saved, and the change count it was last saved at.
struct QueueFile {
    path: Option<std::path::PathBuf>,
    saved: Mutex<u64>,
    /// The exclusive lock on `queue.lock`, held until the app exits.
    _lock: Option<std::fs::File>,
    /// Why the queue isn't being saved, for the UI; `None` when all is well. Set at startup if
    /// saving is off for the session, or by a failed save (and cleared by the next good one).
    problem: Mutex<Option<String>>,
}

/// How long startup waits for another copy to release the queue (its final save).
const LOCK_WAIT: Duration = Duration::from_secs(10);

/// [`lock_queue`], retrying for up to `wait` while another copy still holds the lock.
fn lock_queue_waiting(queue_file: &std::path::Path, wait: Duration) -> Option<std::fs::File> {
    let deadline = std::time::Instant::now() + wait;
    loop {
        if let Some(lock) = lock_queue(queue_file) {
            return Some(lock);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Why the queue won't be saved this session (#12), or `None` when it will be.
#[tauri::command]
fn queue_storage_problem(file: tauri::State<'_, QueueFile>) -> Option<String> {
    file.problem.lock().ok().and_then(|p| p.clone())
}

/// Emitted with the storage problem (a string, or null once saving works again).
const STORAGE_EVENT: &str = "queue://storage";

/// Takes the exclusive lock on the queue (`queue.lock` next to `queue.json`), or `None` if
/// another running copy of Queue holds it.
fn lock_queue(queue_file: &std::path::Path) -> Option<std::fs::File> {
    let path = queue_file.with_extension("lock");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .ok()?;
    file.try_lock().ok()?;
    Some(file)
}

/// Saves the queue if it changed since the last save (#12). The JSON is made under the queue
/// lock; the write happens after it's released, so downloads never wait on the disk. `saved`
/// is held across the write, so two saves can't overtake each other.
fn save_queue(app: &AppHandle) {
    let file = app.state::<QueueFile>();
    let queue = app.state::<Mutex<Queue>>();
    if let Some(problem) = persist(&file, &queue, settings::write_atomic) {
        let _ = app.emit(STORAGE_EVENT, problem);
    }
}

/// One save of the queue if it changed (see [`save_queue`]); `write` is injectable for tests.
/// A failed write leaves the saved count alone, so the next tick retries it, and reports the
/// problem; the next good write clears it. Returns the problem when it changed, to emit.
fn persist(
    file: &QueueFile,
    queue: &Mutex<Queue>,
    write: impl Fn(&std::path::Path, &[u8]) -> std::io::Result<()>,
) -> Option<Option<String>> {
    let path = file.path.as_ref()?;
    let mut saved = file.saved.lock().ok()?;
    let (changes, json) = match lock(queue) {
        Ok(q) if q.changes() != *saved => (q.changes(), q.to_json()),
        _ => return None,
    };
    let now = match write(path, &json) {
        Ok(()) => {
            *saved = changes;
            None
        }
        Err(e) => Some(format!(
            "Queue couldn't save the list ({e}), so recent changes would be lost if it quit now. It keeps trying."
        )),
    };
    let mut problem = file.problem.lock().ok()?;
    if *problem == now {
        return None;
    }
    *problem = now.clone();
    Some(now)
}

/// The cancel flag of each running download (#74), by row id. Changed only while the queue is
/// locked, so a row is active exactly while its flag is here.
#[derive(Default)]
struct Cancels(Mutex<HashMap<u64, Arc<AtomicBool>>>);

/// Everything a started download needs, captured while the queue was locked.
struct DownloadJob {
    id: u64,
    /// Set by `queue_cancel` (#74); the run's wait loops check it.
    cancel: Arc<AtomicBool>,
    url: String,
    info: Option<std::path::PathBuf>,
    sidecar_dir: std::path::PathBuf,
    work_dir: std::path::PathBuf,
    out_dir: std::path::PathBuf,
    /// The format chosen in Settings when the download started (#11).
    format: download::Format,
    /// The row's proposed name when the download started (N1, #15).
    name: Option<String>,
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
    let cancel = Arc::new(AtomicBool::new(false));
    if let Ok(mut cancels) = app.state::<Cancels>().0.lock() {
        cancels.insert(id, cancel.clone());
    }
    let job = DownloadJob {
        id,
        cancel,
        url: item.url.clone(),
        info,
        sidecar_dir,
        work_dir,
        out_dir,
        format,
        name: item.name.clone(),
    };
    Ok((item, job))
}

/// Runs a started download to the end (blocking): 403 retries with a pause (D-017), then the
/// outcome is recorded, the saved info deleted, and the row emitted.
fn complete_download(app: &AppHandle, job: DownloadJob) {
    // Counted until the result is recorded, so quitting saves after it (#67). Refused once
    // quitting: the row stays as begun and comes back interrupted.
    let Some(_running) = proc::job() else {
        return;
    };
    let state = app.state::<Mutex<Queue>>();
    let id = job.id;
    let emit = |updated: Option<Item>| {
        if let Some(item) = updated {
            let _ = app.emit(ITEM_EVENT, item);
        }
    };
    // Progress arrives already throttled (download::PROGRESS_EVERY); state changes always do.
    // A cancel (#74) stops it like a quit, but only this download.
    let outcome = proc::with_cancel(job.cancel.clone(), || {
        download::run_with_retries_named(
            &job.sidecar_dir,
            &job.work_dir,
            &job.out_dir,
            &job.url,
            job.info.as_deref(),
            job.format,
            job.name.as_deref(),
            proc::sleep_unless_quitting,
            |attempt| emit(lock(&state).ok().and_then(|mut q| q.retrying(id, attempt))),
            |event| match event {
                download::Event::Progress(p) => {
                    emit(lock(&state).ok().and_then(|mut q| q.progressed(id, p)));
                }
                download::Event::Processing(_) => {
                    emit(lock(&state).ok().and_then(|mut q| q.processing(id)));
                }
            },
        )
    });
    // A failure while quitting is the quit stopping it (#67): leave the row as it is, so the
    // final save keeps it as an interrupted download rather than a failure. A finished
    // download is still recorded.
    if outcome.is_err() && proc::stopping() {
        return;
    }
    // A "most compatible" format the site didn't have gets an honest notice (D-015), decided
    // from the codecs yt-dlp chose.
    let notice = outcome.as_ref().ok().and_then(|saved| {
        job.format
            .notice_for(std::path::Path::new(&saved.path), &saved.codecs)
    });
    let outcome = outcome.map(|saved| saved.path);
    let (updated, info) = record_download(
        &state,
        &app.state::<Cancels>(),
        id,
        &job.cancel,
        outcome,
        notice,
    );
    if let Some(path) = info {
        let _ = std::fs::remove_file(path);
    }
    if let Some(item) = updated {
        let _ = app.emit(ITEM_EVENT, item);
    }
}

/// Records a download's outcome and drops its cancel flag; returns the updated row and the
/// saved info file to delete. A cancel that lost the race to the finished file leaves the row
/// Done; otherwise the row goes back to Queued, marked Cancelled (owner's choice), never Failed.
/// The flag is read under the queue lock, which `queue_cancel` holds while setting it, so a
/// cancel it accepted is never recorded as a failure (#74).
fn record_download(
    queue: &Mutex<Queue>,
    cancels: &Cancels,
    id: u64,
    cancel: &AtomicBool,
    outcome: Result<String, String>,
    notice: Option<String>,
) -> (Option<Item>, Option<std::path::PathBuf>) {
    let Ok(mut q) = lock(queue) else {
        return (None, None);
    };
    if let Ok(mut cancels) = cancels.0.lock() {
        cancels.remove(&id);
    }
    let finished = if outcome.is_err() && cancel.load(Ordering::SeqCst) {
        q.cancelled(id)
    } else {
        q.finish(id, outcome)
    };
    let updated = match notice {
        Some(notice) => q.noticed(id, notice).or(finished),
        None => finished,
    };
    (updated, q.take_info(id))
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

/// Cancels a download in progress (#74): its whole yt-dlp/ffmpeg tree is stopped and its
/// staging and partial files removed, then the row goes back to Queued, marked Cancelled. The
/// updated row arrives as [`ITEM_EVENT`] once the download has stopped; a download that finishes
/// first stays Done.
#[tauri::command]
fn queue_cancel(
    queue: tauri::State<'_, Mutex<Queue>>,
    cancels: tauri::State<'_, Cancels>,
    id: u64,
) -> Result<(), String> {
    let q = lock(&queue)?;
    let active = q.items().iter().any(|i| i.id == id && i.is_downloading());
    let flag = cancels.0.lock().ok().and_then(|c| c.get(&id).cloned());
    match flag {
        Some(flag) if active => {
            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
        _ => Err("Only a download in progress can be cancelled.".into()),
    }
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
                    if proc::stopping() {
                        return false; // quitting (#67): start nothing more
                    }
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
        // One running copy (D-025, #12): launching Queue again brings the open window to the
        // front instead of starting a second copy that would overwrite the same queue.json.
        // Registered first, as the plugin requires.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .manage(Runner(Mutex::new(false)))
        .manage(Cancels::default())
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
            let template = current.naming_template;
            app.manage(Settings {
                file,
                current: Mutex::new(current),
            });
            // The queue from the last run (#12). Downloads cut off when Queue closed come back
            // Queued and wait for the user; rows still fetching their info fetch it again.
            let queue_file = app.path().app_data_dir().ok().map(|d| d.join(QUEUE_FILE));
            // A backstop to the single instance: only the copy holding an exclusive OS lock on
            // queue.lock (for its whole run) writes the queue. Taken before loading, waiting out
            // a copy that's still finishing its final save, so we load what it saved.
            let lock = queue_file
                .as_deref()
                .and_then(|f| lock_queue_waiting(f, LOCK_WAIT));
            // Staging and saved-info folders left by a quit or crash (#45). Only the copy that
            // owns the queue cleans up, so another copy's running download is never touched;
            // listed now, before this run makes its own, and deleted in the background.
            if lock.is_some()
                && let Ok(cache) = app.path().app_cache_dir()
            {
                let stale = download::stale_work_dirs(&cache);
                if !stale.is_empty() {
                    std::thread::spawn(move || {
                        for dir in stale {
                            let _ = download::remove_stale(&dir);
                        }
                    });
                }
            }
            let (mut queue, writable) = queue_file
                .as_deref()
                .map(Queue::load_checked)
                .unwrap_or_default();
            // Names follow the saved template (#17); hand-edited ones are kept.
            queue.set_template(template);
            let problem = match (&queue_file, lock.is_some(), writable) {
                (None, ..) => Some("Queue couldn't find its data folder, so this list won't be saved.".to_string()),
                (Some(_), false, _) => Some("Another copy of Queue is still using the saved list, so changes made now won't be saved. Quit and reopen Queue.".to_string()),
                (Some(_), true, false) => Some("Queue couldn't read or set aside its saved list, so changes made now won't be saved. The old file is untouched.".to_string()),
                _ => None,
            };
            let writable = writable && lock.is_some();
            let refetch = queue.fetching();
            let unnamed = queue.awaiting_ai();
            let saved = queue.changes();
            app.manage(Mutex::new(queue));
            app.manage(QueueFile {
                // A damaged file that couldn't be moved aside, or a queue another running copy
                // owns, is never saved over (D-025).
                path: queue_file.filter(|_| writable),
                _lock: lock,
                problem: Mutex::new(problem),
                saved: Mutex::new(saved),
            });
            // N4 (#25): one worker asks for AI names; a row restored as Queued is asked again.
            // Managed before the fetch worker starts, since a finished fetch asks for a name.
            let (names, named) = mpsc::channel();
            app.manage(Naming(Mutex::new(names)));
            let (jobs, queued) = mpsc::channel();
            for job in refetch {
                let _ = jobs.send(job);
            }
            app.manage(Fetcher(Mutex::new(jobs)));
            let handle = app.handle().clone();
            std::thread::spawn(move || fetch_worker(handle, queued));
            for id in unnamed {
                ask_for_name(app.handle(), id);
            }
            let handle = app.handle().clone();
            std::thread::Builder::new()
                .name("naming".into())
                .spawn(move || naming_worker(handle, named))
                .map_err(|e| format!("Couldn't start the naming worker: {e}"))?;
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(SAVE_EVERY);
                    save_queue(&handle);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            queue_add,
            queue_list,
            queue_download,
            queue_cancel,
            queue_remove,
            queue_rename,
            queue_retry,
            queue_run_all,
            queue_runner_state,
            queue_storage_problem,
            settings_download_folder,
            settings_choose_download_folder,
            settings_format,
            settings_set_format,
            settings_naming_template,
            settings_set_naming_template,
            naming_previews,
            settings_naming_provider,
            settings_set_naming_provider,
            naming_status,
            open_download_folder,
            reveal_download
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // Stop running downloads and fetches first, so none outlives Queue (#67), then
                // one last save: rows cut off now come back as interrupted after a restart.
                proc::shut_down();
                proc::wait_idle(QUIT_WAIT);
                save_queue(app);
            }
        });
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

    /// #74 review: a cancel accepted while a failed download waits to record its outcome makes
    /// the row Cancelled, not Failed; a finished file still wins over a late cancel.
    #[test]
    fn a_cancel_accepted_before_the_outcome_is_recorded_is_never_a_failure() {
        let queue = Mutex::new(Queue::default());
        let cancels = Cancels::default();
        let started = |url: &str| {
            let mut q = queue.lock().unwrap();
            let id = q.add(url).unwrap().id;
            q.fetched(id, Ok(Meta::default())).unwrap();
            q.start(id).unwrap();
            let flag = Arc::new(AtomicBool::new(false));
            cancels.0.lock().unwrap().insert(id, flag.clone());
            (id, flag)
        };
        let (id, flag) = started("https://e.test/a");
        let held = queue.lock().unwrap(); // queue_cancel holds the lock
        let updated = std::thread::scope(|s| {
            let recording =
                s.spawn(|| record_download(&queue, &cancels, id, &flag, Err("x".into()), None));
            std::thread::sleep(Duration::from_millis(100)); // let it reach the lock
            flag.store(true, Ordering::SeqCst); // queue_cancel accepts
            drop(held);
            recording.join().unwrap().0
        })
        .unwrap();
        assert_eq!(updated.status, Status::Queued);
        assert_eq!(updated.notice.as_deref(), Some(queue::CANCELLED));
        assert!(cancels.0.lock().unwrap().is_empty(), "flag dropped");

        queue.lock().unwrap().remove(id).unwrap();
        let (id, flag) = started("https://e.test/b");
        flag.store(true, Ordering::SeqCst);
        let (done, _) = record_download(&queue, &cancels, id, &flag, Ok("/d/b".into()), None);
        assert!(matches!(done.unwrap().status, Status::Done { .. }));
    }

    #[test]
    fn only_one_running_copy_can_hold_the_queue_lock() {
        let dir = std::env::temp_dir().join(format!("queue-q7-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let queue_file = dir.join("queue.json");
        let first = lock_queue(&queue_file).expect("the first copy gets the lock");
        assert!(
            lock_queue(&queue_file).is_none(),
            "a second copy can't save the queue"
        );
        assert!(dir.join("queue.lock").is_file());
        drop(first);
        assert!(
            lock_queue(&queue_file).is_some(),
            "free again once the first copy exits"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn startup_waits_for_a_copy_that_is_still_saving_then_takes_the_lock() {
        let dir = std::env::temp_dir().join(format!("queue-q7-wait-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let queue_file = dir.join("queue.json");
        let exiting = lock_queue(&queue_file).unwrap();
        let released = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300)); // its final save
            drop(exiting);
        });
        let started = std::time::Instant::now();
        assert!(lock_queue_waiting(&queue_file, Duration::from_secs(5)).is_some());
        assert!(started.elapsed() >= Duration::from_millis(250), "it waited");
        released.join().unwrap();
        let held = lock_queue(&queue_file).expect("free after the other copy exited");
        let started = std::time::Instant::now();
        assert!(
            lock_queue_waiting(&queue_file, Duration::from_millis(300)).is_none(),
            "gives up"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(held);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_save_is_reported_retried_and_cleared_once_saving_works() {
        let queue = Mutex::new(Queue::default());
        let file = QueueFile {
            path: Some(std::path::PathBuf::from("/q/queue.json")),
            saved: Mutex::new(0),
            _lock: None,
            problem: Mutex::new(None),
        };
        let disk_full = |_: &std::path::Path, _: &[u8]| -> std::io::Result<()> {
            Err(std::io::Error::other("No space left on device"))
        };
        let written = std::cell::RefCell::new(Vec::new());
        let works = |_: &std::path::Path, bytes: &[u8]| -> std::io::Result<()> {
            written.borrow_mut().push(bytes.to_vec());
            Ok(())
        };
        assert_eq!(
            persist(&file, &queue, works),
            None,
            "nothing changed, nothing to save"
        );
        queue.lock().unwrap().add("https://e.test/a").unwrap();
        let reported = persist(&file, &queue, disk_full).expect("the failure is reported");
        assert!(
            reported
                .as_deref()
                .unwrap()
                .contains("No space left on device")
        );
        assert_eq!(*file.saved.lock().unwrap(), 0, "not marked saved");
        assert_eq!(
            persist(&file, &queue, disk_full),
            None,
            "same problem: no repeat event"
        );
        queue.lock().unwrap().add("https://e.test/b").unwrap(); // a change while failing
        assert_eq!(
            persist(&file, &queue, works),
            Some(None),
            "cleared once a save works"
        );
        let last = Queue::from_json(written.borrow().last().unwrap()).unwrap();
        assert_eq!(
            last.items().len(),
            2,
            "the retry saved every pending change"
        );
        assert_eq!(*file.saved.lock().unwrap(), queue.lock().unwrap().changes());
        assert_eq!(persist(&file, &queue, works), None);
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
