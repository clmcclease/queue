pub mod queue;
pub mod sidecar;

use std::sync::Mutex;

use queue::{Item, Queue};

#[tauri::command]
fn queue_add(queue: tauri::State<'_, Mutex<Queue>>, url: String) -> Result<Item, String> {
    let mut queue = queue
        .lock()
        .map_err(|_| "queue lock poisoned".to_string())?;
    queue
        .add(&url)
        .ok_or_else(|| "Enter a link to add.".to_string())
}

#[tauri::command]
fn queue_list(queue: tauri::State<'_, Mutex<Queue>>) -> Result<Vec<Item>, String> {
    let queue = queue
        .lock()
        .map_err(|_| "queue lock poisoned".to_string())?;
    Ok(queue.items().to_vec())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(Mutex::new(Queue::default()))
        .invoke_handler(tauri::generate_handler![queue_add, queue_list])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
