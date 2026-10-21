pub mod ai;
pub mod download;
pub mod link;
pub mod local;
pub mod meta;
pub mod models;
pub mod naming;
pub mod proc;
pub mod queue;
pub mod reveal;
pub mod secrets;
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

/// Emitted with the [`ModelStatus`] whenever the local model's download changes (#31).
const MODEL_EVENT: &str = "queue://model";

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

/// Rows waiting for an AI name (#25), each with the naming generation it was queued under. One
/// worker asks for them in order.
struct Naming(Mutex<Sender<(u64, u64)>>);

/// Where the API key is kept (#32): the OS credential store.
struct Vault(Box<dyn secrets::Secrets>);

/// Hands a row to the naming worker, if the chosen provider can be asked. Nothing is sent for
/// an unavailable one (D-029): the row keeps its Tier-1 name with no tag. This check doesn't
/// read the key; the job resolves the endpoint itself. The job carries the current generation,
/// so one queued before a later provider or API change is dropped, not asked twice (#32 review).
fn ask_for_name(app: &AppHandle, id: u64) {
    if !current_settings(app).is_ok_and(|s| askable(app, &s)) {
        return;
    }
    let generation = lock(&app.state::<Mutex<Queue>>())
        .ok()
        .and_then(|q| q.naming_job(id))
        .map(|(_, g)| g);
    if let Some(generation) = generation
        && let Ok(jobs) = app.state::<Naming>().0.lock()
    {
        let _ = jobs.send((id, generation));
    }
}

/// Whether the chosen provider could be asked: [`ai::askable`], or a downloaded local model,
/// whose server starts when the job runs (#30).
fn askable(app: &AppHandle, settings: &settings::Settings) -> bool {
    ai::askable(settings)
        || settings.naming_provider == ai::Provider::Local
            && matches!(local_model(app, settings), ai::LocalModel::Downloaded)
}

/// Asks for AI names one row at a time. It doesn't hold a quit up (#67): an answer lost at quit
/// just leaves the Tier-1 name. The provider is resolved inside each job, after the row's
/// generation is taken (#32), so a provider changed meanwhile is never sent to.
fn naming_worker(app: AppHandle, jobs: Receiver<(u64, u64)>) {
    let mut claims = NamingClaims::default();
    for (id, generation) in jobs {
        if proc::stopping() {
            return;
        }
        if !claims.claim(id, generation) {
            continue;
        }
        // The local server starts here, inside the job (#30): a start that fails is a failed
        // ask, so the row shows the rules tag.
        let resolve = ai::Starting(|| {
            let Ok(settings) = current_settings(&app) else {
                return ai::Ready::Nothing;
            };
            let local = match local_model(&app, &settings) {
                ai::LocalModel::Downloaded if settings.naming_provider == ai::Provider::Local => {
                    match start_local(&app, &settings) {
                        Ok(endpoint) => ai::LocalModel::Running(endpoint),
                        Err(error) => return ai::Ready::Failed(error),
                    }
                }
                local => local,
            };
            match ai::endpoint(&settings, app.state::<Vault>().0.as_ref(), local) {
                Ok(endpoint) => ai::Ready::Ask(endpoint),
                Err(_) => ai::Ready::Nothing,
            }
        });
        let state = app.state::<Mutex<Queue>>();
        if let Some(item) = run_naming_job_at(&state, id, Some(generation), &resolve) {
            let _ = app.emit(ITEM_EVENT, item);
        }
    }
}

/// Which rows the naming worker already took for the newest generation (#32 review). A row can
/// be queued twice for one generation (a fetch finishing as the API setup changes); only the
/// first job is asked. Older generations are dropped by [`run_naming_job_at`] anyway.
#[derive(Default)]
pub struct NamingClaims {
    generation: u64,
    ids: std::collections::HashSet<u64>,
}

impl NamingClaims {
    /// Whether this job is the first for `id` at `generation`.
    pub fn claim(&mut self, id: u64, generation: u64) -> bool {
        if generation > self.generation {
            self.generation = generation;
            self.ids.clear();
        }
        generation < self.generation || self.ids.insert(id)
    }
}

/// One naming job (#25): takes what it needs under the queue lock, asks `namer` with the lock
/// released (it can take seconds; an [`ai::Resolving`] namer finds its endpoint only then, and
/// sends only if the job is still current), then records the answer, which [`Queue::ai_named`]
/// drops if the row or the provider changed meanwhile. A panicking namer fails just this row.
pub fn run_naming_job(queue: &Mutex<Queue>, id: u64, namer: &dyn ai::Namer) -> Option<Item> {
    run_naming_job_at(queue, id, None, namer)
}

/// [`run_naming_job`] for a job queued under `queued_at`: if the generation has moved on since,
/// a newer job for the row is already queued, so this one does nothing (no second request).
pub fn run_naming_job_at(
    queue: &Mutex<Queue>,
    id: u64,
    queued_at: Option<u64>,
    namer: &dyn ai::Namer,
) -> Option<Item> {
    let (meta, generation) = lock(queue).ok()?.naming_job(id)?;
    if queued_at.is_some_and(|g| g != generation) {
        return None;
    }
    // Still this row's job: Queued, not edited, and no provider change since the snapshot.
    let current = || {
        lock(queue)
            .ok()
            .and_then(|q| q.naming_job(id))
            .is_some_and(|(_, g)| g == generation)
    };
    let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        namer.name_if(&meta, &current)
    }))
    .unwrap_or_else(|_| Some(Err("Naming failed unexpectedly.".into())))?;
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

/// Runs a command's `work` on a blocking thread (#32 review): anything that can wait on the OS
/// credential store (an unlock prompt, a stalled Secret Service) must never freeze the window.
async fn off_main<T: Send + 'static>(
    app: AppHandle,
    work: impl FnOnce(&AppHandle) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || work(&app))
        .await
        .map_err(|_| "That stopped unexpectedly.".to_string())?
}

/// The AI naming provider (N4, #25).
#[tauri::command]
fn settings_naming_provider(app: AppHandle) -> Result<ai::Provider, String> {
    Ok(current_settings(&app)?.naming_provider)
}

/// Saves the AI naming provider (#25). Answers already on their way are dropped, and if the new
/// provider can be asked, every Queued row not renamed by hand is asked again.
#[tauri::command]
async fn settings_set_naming_provider(
    app: AppHandle,
    provider: ai::Provider,
) -> Result<NamingStatus, String> {
    off_main(app, move |app| set_naming_provider(app, provider)).await
}

fn set_naming_provider(app: &AppHandle, provider: ai::Provider) -> Result<NamingStatus, String> {
    let save = || -> Result<(), String> {
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
        Ok(())
    };
    // Leaving Local gives its memory back now rather than after the idle time (#30).
    if provider == ai::Provider::Local {
        save()?;
    } else {
        app.state::<local::Runtime>().stop_while(save)?;
    }
    let again = lock(&app.state::<Mutex<Queue>>())?.renaming();
    for id in again {
        ask_for_name(app, id);
    }
    status_of_naming(app)
}

/// The chosen provider and why it can't be asked yet, if it can't (the banner, PRD §6.3).
#[derive(Clone, Debug, serde::Serialize)]
struct NamingStatus {
    provider: ai::Provider,
    unavailable: Option<ai::Unavailable>,
}

#[tauri::command]
async fn naming_status(app: AppHandle) -> Result<NamingStatus, String> {
    off_main(app, status_of_naming).await
}

fn status_of_naming(app: &AppHandle) -> Result<NamingStatus, String> {
    let settings = current_settings(app)?;
    Ok(NamingStatus {
        provider: settings.naming_provider,
        unavailable: ai::unavailable(
            &settings,
            app.state::<Vault>().0.as_ref(),
            local_model(app, &settings),
        ),
    })
}

/// The API service's setup (N8, #32), as the Settings screen shows it. The key itself is never
/// sent to the window: only whether one is saved.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiSetup {
    kind: ai::ApiKind,
    base_url: String,
    model: String,
    key_set: bool,
    /// Why the credential store couldn't be read, if it couldn't.
    store_error: Option<String>,
}

fn api_setup(app: &AppHandle) -> Result<ApiSetup, String> {
    let settings = current_settings(app)?;
    let key = app.state::<Vault>().0.get();
    Ok(ApiSetup {
        kind: settings.api_kind,
        base_url: settings.api_base_url,
        model: settings.api_model,
        key_set: matches!(key, Ok(Some(_))),
        store_error: key.err(),
    })
}

/// The API service changed (#32): if it's the chosen provider, answers on their way are dropped
/// and every Queued row not renamed by hand is asked again, as for a provider change.
fn api_changed(app: &AppHandle) -> Result<(), String> {
    if current_settings(app)?.naming_provider != ai::Provider::Api {
        return Ok(());
    }
    let again = lock(&app.state::<Mutex<Queue>>())?.renaming();
    for id in again {
        ask_for_name(app, id);
    }
    Ok(())
}

#[tauri::command]
async fn settings_api(app: AppHandle) -> Result<ApiSetup, String> {
    off_main(app, api_setup).await
}

/// Saves the API service's kind (#91), base URL and model (#32). The URL must be https://, or
/// http:// to this computer, the same rule every request follows. Saved together, so a kind
/// change asks rows again like any other API change.
#[tauri::command]
async fn settings_set_api(
    app: AppHandle,
    kind: ai::ApiKind,
    base_url: String,
    model: String,
) -> Result<ApiSetup, String> {
    let base_url = ai::base_url(&base_url)?;
    let model = model.trim().to_owned();
    if model.is_empty() {
        return Err("Enter the model's name.".into());
    }
    off_main(app, move |app| set_api(app, kind, base_url, model)).await
}

fn set_api(
    app: &AppHandle,
    kind: ai::ApiKind,
    base_url: String,
    model: String,
) -> Result<ApiSetup, String> {
    {
        let state = app.state::<Settings>();
        let mut current = state
            .current
            .lock()
            .map_err(|_| "settings lock poisoned".to_string())?;
        let mut next = current.clone();
        next.api_kind = kind;
        next.api_base_url = base_url;
        next.api_model = model;
        if let Some(file) = &state.file {
            next.save(file)
                .map_err(|e| format!("Couldn't save your settings: {e}"))?;
        }
        *current = next;
    }
    api_changed(app)?;
    api_setup(app)
}

/// Saves the API key in the OS credential store (#32). It's never written to a file, and never
/// sent back to the window.
#[tauri::command]
async fn api_key_set(app: AppHandle, key: String) -> Result<ApiSetup, String> {
    let key = key.trim().to_owned();
    if key.is_empty() {
        return Err("Paste the key first.".into());
    }
    off_main(app, move |app| {
        app.state::<Vault>().0.set(&key)?;
        api_changed(app)?;
        api_setup(app)
    })
    .await
}

/// Removes the API key from the credential store (#32). A missing key isn't an error.
#[tauri::command]
async fn api_key_remove(app: AppHandle) -> Result<ApiSetup, String> {
    off_main(app, |app| {
        app.state::<Vault>().0.remove()?;
        api_changed(app)?;
        api_setup(app)
    })
    .await
}

/// Test connection (#32): names the preview sample with the saved service, off the main thread.
#[tauri::command]
async fn api_test_connection(app: AppHandle) -> Result<ai::Tested, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = current_settings(&app)?;
        Ok(ai::test_connection(
            &settings,
            app.state::<Vault>().0.as_ref(),
        ))
    })
    .await
    .map_err(|_| "The test stopped unexpectedly.".to_string())?
}

/// The local model's download (#31): one at a time, with its cancel flag, and how the last one
/// ended if it failed.
#[derive(Default)]
struct Downloads {
    active: Option<(models::ModelId, Arc<AtomicBool>)>,
    verifying: bool,
    error: Option<(models::ModelId, String)>,
}

/// Where models live: the local (not roaming) app-data folder (D-029).
fn models_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_local_data_dir()
        .map(|d| d.join("models"))
        .map_err(|_| "Queue couldn't find its data folder.".to_string())
}

/// The model in use: the user's choice, else this computer's suggestion, else the default.
fn chosen_model(settings: &settings::Settings) -> models::ModelId {
    settings
        .local_model
        .or_else(|| models::suggested(models::total_memory(), models::GPU))
        .unwrap_or_default()
}

/// Whether the chosen local model is downloaded and checked, and this build can run it. Reads
/// the disk only; nothing is started.
fn local_model(app: &AppHandle, settings: &settings::Settings) -> ai::LocalModel {
    let pin = models::pinned(chosen_model(settings)).pin();
    match models_dir(app).map(|dir| models::state(&dir, &pin)) {
        Ok(models::State::Ready) if app.state::<local::Runtime>().exists() => {
            ai::LocalModel::Downloaded
        }
        Ok(models::State::Ready) => ai::LocalModel::NoRuntime,
        _ => ai::LocalModel::Missing,
    }
}

/// The local server running the chosen model, started if needed (#30). Blocks while it loads.
/// It starts only if, once no switch or delete is running, Local and this model are still
/// chosen and the model is still there (#30(b) review).
fn start_local(app: &AppHandle, settings: &settings::Settings) -> Result<ai::Endpoint, String> {
    let id = chosen_model(settings);
    let pinned = models::pinned(id);
    let dir = models_dir(app)?;
    let wanted = || {
        current_settings(app).is_ok_and(|now| uses_local_model(&now, id))
            && models::state(&dir, &pinned.pin()) == models::State::Ready
    };
    app.state::<local::Runtime>()
        .endpoint_if(&dir.join(pinned.file), pinned.file, &wanted)
}

/// Whether Local is the provider and `model` is the chosen local model: only then does a
/// change to that model (downloaded, switched to) ask Queued rows again (#30(b) review).
fn uses_local_model(settings: &settings::Settings, model: models::ModelId) -> bool {
    settings.naming_provider == ai::Provider::Local && chosen_model(settings) == model
}

/// Asks again for every Queued row not renamed by hand (#30): the local model became usable
/// (downloaded, or switched to one that is), so rows named by the rules alone get their AI name.
fn ask_again(app: &AppHandle) {
    let Ok(again) = lock(&app.state::<Mutex<Queue>>()).map(|mut q| q.renaming()) else {
        return;
    };
    for id in again {
        ask_for_name(app, id);
    }
}

/// One model, as Settings and the banner show it.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelInfo {
    id: models::ModelId,
    name: &'static str,
    file: &'static str,
    source: String,
    size: u64,
    #[serde(flatten)]
    state: models::State,
    downloading: bool,
    verifying: bool,
    error: Option<String>,
}

/// The local model manager's state (#31).
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelStatus {
    /// Rises with every snapshot, in the order they were taken; the window keeps only the
    /// newest, whether it came as an event or a reply (#31 review).
    rev: u64,
    chosen: models::ModelId,
    /// Whether the user picked it (else it's the suggestion or the default).
    picked: bool,
    suggested: Option<models::ModelId>,
    dir: String,
    models: Vec<ModelInfo>,
}

/// The last [`ModelStatus::rev`] handed out. Held while a snapshot is taken and, for an event,
/// sent, so revisions follow the state and events go out in revision order.
static MODEL_REV: Mutex<u64> = Mutex::new(0);

fn model_status_now(app: &AppHandle) -> Result<ModelStatus, String> {
    publish_model_status(app, false)
}

fn emit_model_status(app: &AppHandle) {
    let _ = publish_model_status(app, true);
}

fn publish_model_status(app: &AppHandle, emit: bool) -> Result<ModelStatus, String> {
    let mut rev = MODEL_REV
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *rev += 1;
    let status = model_snapshot(app, *rev)?;
    if emit {
        let _ = app.emit(MODEL_EVENT, status.clone());
    }
    Ok(status)
}

fn model_snapshot(app: &AppHandle, rev: u64) -> Result<ModelStatus, String> {
    let settings = current_settings(app)?;
    let dir = models_dir(app)?;
    let downloads = app
        .state::<Mutex<Downloads>>()
        .lock()
        .map_err(|_| "download state lock poisoned".to_string())?
        .clone_view();
    Ok(ModelStatus {
        rev,
        chosen: chosen_model(&settings),
        picked: settings.local_model.is_some(),
        suggested: models::suggested(models::total_memory(), models::GPU),
        dir: dir.display().to_string(),
        models: models::PINS
            .iter()
            .map(|p| {
                let active = downloads.0 == Some(p.id);
                ModelInfo {
                    id: p.id,
                    name: p.name,
                    file: p.file,
                    source: format!("huggingface.co/{}", p.repo),
                    size: p.size,
                    state: models::state(&dir, &p.pin()),
                    downloading: active,
                    verifying: active && downloads.1,
                    error: downloads
                        .2
                        .as_ref()
                        .filter(|(id, _)| *id == p.id)
                        .map(|(_, e)| e.clone()),
                }
            })
            .collect(),
    })
}

impl Downloads {
    /// What the status needs, so the lock isn't held while the disk is read.
    fn clone_view(
        &self,
    ) -> (
        Option<models::ModelId>,
        bool,
        Option<(models::ModelId, String)>,
    ) {
        (
            self.active.as_ref().map(|(id, _)| *id),
            self.verifying,
            self.error.clone(),
        )
    }
}

#[tauri::command]
async fn model_status(app: AppHandle) -> Result<ModelStatus, String> {
    off_main(app, model_status_now).await
}

/// Saves the user's choice of local model (#31). A download already running carries on: it's
/// a file, not a session. The server running the old model is stopped first (#30), and rows
/// are asked again if Local uses the new choice.
#[tauri::command]
async fn settings_set_local_model(
    app: AppHandle,
    model: models::ModelId,
) -> Result<ModelStatus, String> {
    off_main(app, move |app| {
        app.state::<local::Runtime>()
            .stop_while(|| -> Result<(), String> {
                let state = app.state::<Settings>();
                let mut current = state
                    .current
                    .lock()
                    .map_err(|_| "settings lock poisoned".to_string())?;
                let mut next = current.clone();
                next.local_model = Some(model);
                if let Some(file) = &state.file {
                    next.save(file)
                        .map_err(|e| format!("Couldn't save your settings: {e}"))?;
                }
                *current = next;
                Ok(())
            })?;
        if current_settings(app).is_ok_and(|s| uses_local_model(&s, model)) {
            ask_again(app);
        }
        // An event, not only the reply: a reply can lose to an older progress event (#31
        // review), so the newest state always arrives as one.
        emit_model_status(app);
        model_status_now(app)
    })
    .await
}

/// Starts downloading a model (#31). This click is the user's consent: the banner and Settings
/// say the size and the source next to the button. One download at a time; it runs on its own
/// thread, counted as a quit-time job, and stops (keeping the partial file) on Cancel or quit.
#[tauri::command]
async fn model_download(app: AppHandle, model: models::ModelId) -> Result<ModelStatus, String> {
    off_main(app, move |app| {
        let dir = models_dir(app)?;
        let flag = {
            let state = app.state::<Mutex<Downloads>>();
            let mut downloads = state
                .lock()
                .map_err(|_| "download state lock poisoned".to_string())?;
            match &downloads.active {
                Some((id, _)) if *id == model => None,
                Some(_) => {
                    return Err(
                        "One model downloads at a time. Cancel the other download first.".into(),
                    );
                }
                None => {
                    let flag = Arc::new(AtomicBool::new(false));
                    downloads.active = Some((model, flag.clone()));
                    downloads.verifying = false;
                    downloads.error = None;
                    Some(flag)
                }
            }
        };
        if let Some(flag) = flag {
            let Some(job) = proc::job() else {
                if let Ok(mut d) = app.state::<Mutex<Downloads>>().lock() {
                    d.active = None;
                }
                return Err("Queue is quitting.".into());
            };
            emit_model_status(app);
            let app = app.clone();
            std::thread::spawn(move || {
                let _job = job;
                let pin = models::pinned(model).pin();
                let mut last = std::time::Instant::now();
                let result = proc::with_cancel(flag, || {
                    models::download(&pin, &dir, &models::Options::default(), &mut |p| {
                        if p == models::Progress::Verifying {
                            if let Ok(mut d) = app.state::<Mutex<Downloads>>().lock() {
                                d.verifying = true;
                            }
                        } else if last.elapsed() < Duration::from_millis(250) {
                            return;
                        }
                        last = std::time::Instant::now();
                        emit_model_status(&app);
                    })
                });
                let done = result.is_ok();
                if let Ok(mut d) = app.state::<Mutex<Downloads>>().lock() {
                    d.active = None;
                    d.verifying = false;
                    d.error = result.err().map(|e| (model, e));
                }
                // The model can be asked now (#30): rows named by the rules alone get asked, if
                // it's the one Local uses. Rows an API named aren't asked again (#30(b) review).
                if done && current_settings(&app).is_ok_and(|s| uses_local_model(&s, model)) {
                    ask_again(&app);
                }
                emit_model_status(&app);
            });
        }
        model_status_now(app)
    })
    .await
}

/// Cancels the running download; its partial file is kept, to resume.
#[tauri::command]
async fn model_cancel(app: AppHandle) -> Result<ModelStatus, String> {
    off_main(app, |app| {
        if let Ok(d) = app.state::<Mutex<Downloads>>().lock()
            && let Some((_, flag)) = &d.active
        {
            flag.store(true, Ordering::SeqCst);
        }
        model_status_now(app)
    })
    .await
}

/// Deletes a model, its checksum file and any partial download (#31). Refused while that model
/// downloads. The local server is stopped first, so it never runs a deleted file (#30).
#[tauri::command]
async fn model_delete(app: AppHandle, model: models::ModelId) -> Result<ModelStatus, String> {
    off_main(app, move |app| {
        let busy = app
            .state::<Mutex<Downloads>>()
            .lock()
            .map_err(|_| "download state lock poisoned".to_string())?
            .active
            .as_ref()
            .is_some_and(|(id, _)| *id == model);
        if busy {
            return Err("Cancel the download first.".into());
        }
        let dir = models_dir(app)?;
        app.state::<local::Runtime>()
            .stop_while(|| models::delete(&dir, &models::pinned(model).pin()))
            .map_err(|e| format!("Couldn't delete the model: {e}"))?;
        if let Ok(mut d) = app.state::<Mutex<Downloads>>().lock()
            && d.error.as_ref().is_some_and(|(id, _)| *id == model)
        {
            d.error = None;
        }
        publish_model_status(app, true)
    })
    .await
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

/// How often the local server's idle time is checked (#30).
const IDLE_CHECK: Duration = Duration::from_secs(5);

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
            app.manage(Vault(Box::new(secrets::Keyring::new())));
            app.manage(Mutex::new(Downloads::default()));
            // The local model's server (#30): started by the naming worker, stopped by its own
            // idle timer, at quit, and before a model is deleted or switched.
            let resources = app.path().resource_dir().unwrap_or_default();
            app.manage(local::Runtime::new(
                local::server_in(&resources),
                models::GPU,
            ));
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(IDLE_CHECK);
                    handle.state::<local::Runtime>().stop_if_idle();
                }
            });
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
            settings_api,
            settings_set_api,
            api_key_set,
            api_key_remove,
            api_test_connection,
            model_status,
            settings_set_local_model,
            model_download,
            model_cancel,
            model_delete,
            open_download_folder,
            reveal_download
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // Stop running downloads and fetches first, so none outlives Queue (#67), and the
                // local model's server (#30), then one last save: rows cut off now come back as
                // interrupted after a restart.
                proc::shut_down();
                if let Some(runtime) = app.try_state::<local::Runtime>() {
                    runtime.stop();
                }
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

    /// #30(b) review: a model download finishing, or a model switch, asks rows again only when
    /// Local is the provider and that model is the chosen one, so API-named rows aren't resent.
    #[test]
    fn only_the_model_local_uses_asks_rows_again() {
        use models::ModelId::{Qwen3_4bInstruct2507 as Big, Qwen3_5_2b as Small};
        let with = |provider, model| settings::Settings {
            naming_provider: provider,
            local_model: Some(model),
            ..Default::default()
        };
        assert!(uses_local_model(&with(ai::Provider::Local, Small), Small));
        assert!(
            !uses_local_model(&with(ai::Provider::Local, Big), Small),
            "not the chosen one"
        );
        assert!(
            !uses_local_model(&with(ai::Provider::Api, Small), Small),
            "an API is in use"
        );
        assert!(!uses_local_model(&with(ai::Provider::Off, Small), Small));
    }

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
