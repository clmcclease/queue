pub mod download;
pub mod link;
pub mod meta;
pub mod proc;
pub mod queue;
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
    let outcome = outcome.map(|(meta, saved)| {
        if let Some(path) = saved {
            q.set_info(id, path, std::time::SystemTime::now());
        }
        meta
    });
    q.fetched(id, outcome)
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
    let (item, info) = {
        let mut q = lock(&queue)?;
        let item = q.start(id)?;
        (item, q.fresh_info(id, std::time::SystemTime::now()))
    };
    let url = item.url.clone();
    std::thread::spawn(move || {
        let state = app.state::<Mutex<Queue>>();
        // 403s are retried with a fresh extraction after a pause; the row shows the attempt.
        let outcome = download::run_with_retries(
            &sidecar_dir,
            &work_dir,
            &out_dir,
            &url,
            info.as_deref(),
            std::thread::sleep,
            |attempt| {
                let updated = lock(&state).ok().and_then(|mut q| q.retrying(id, attempt));
                if let Some(item) = updated {
                    let _ = app.emit(ITEM_EVENT, item);
                }
            },
        );
        let (updated, info) = match lock(&state) {
            Ok(mut q) => (q.finish(id, outcome), q.take_info(id)),
            Err(_) => (None, None),
        };
        if let Some(path) = info {
            let _ = std::fs::remove_file(path);
        }
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
        .setup(|app| {
            let (jobs, queued) = mpsc::channel();
            app.manage(Fetcher(Mutex::new(jobs)));
            let handle = app.handle().clone();
            std::thread::spawn(move || fetch_worker(handle, queued));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            queue_add,
            queue_list,
            queue_download
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
