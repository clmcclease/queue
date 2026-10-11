pub mod download;
pub mod link;
pub mod queue;
pub mod sidecar;

use std::sync::Mutex;

use queue::{AddError, Item, Queue};
use tauri::{AppHandle, Emitter, Manager};

/// Emitted with the updated [`Item`] whenever a background download changes its status.
const ITEM_EVENT: &str = "queue://item";

fn lock(queue: &Mutex<Queue>) -> Result<std::sync::MutexGuard<'_, Queue>, String> {
    queue.lock().map_err(|_| "queue lock poisoned".to_string())
}

#[tauri::command]
fn queue_add(queue: tauri::State<'_, Mutex<Queue>>, url: String) -> Result<Item, AddError> {
    lock(&queue)
        .map_err(|message| AddError::Invalid { message })?
        .add(&url)
}

#[tauri::command]
fn queue_list(queue: tauri::State<'_, Mutex<Queue>>) -> Result<Vec<Item>, String> {
    Ok(lock(&queue)?.items().to_vec())
}

/// Starts downloading one queued item into the OS Downloads folder (fixed until #10) on a
/// background thread, staged in a fresh folder under the app cache. Returns the item as
/// `Downloading`; the outcome arrives as [`ITEM_EVENT`].
#[tauri::command]
fn queue_download(
    app: AppHandle,
    queue: tauri::State<'_, Mutex<Queue>>,
    id: u64,
) -> Result<Item, String> {
    let out_dir = app
        .path()
        .download_dir()
        .map_err(|e| format!("Couldn't find your Downloads folder: {e}"))?;
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
    let item = lock(&queue)?.start(id)?;
    let url = item.url.clone();
    std::thread::spawn(move || {
        let outcome = download::run(&sidecar_dir, &work_dir, &out_dir, &url);
        let state = app.state::<Mutex<Queue>>();
        let updated = lock(&state).ok().and_then(|mut q| q.finish(id, outcome));
        if let Some(item) = updated {
            let _ = app.emit(ITEM_EVENT, item);
        }
    });
    Ok(item)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(Mutex::new(Queue::default()))
        .invoke_handler(tauri::generate_handler![
            queue_add,
            queue_list,
            queue_download
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
