//! DownloadOS core library (Tauri backend).

pub mod download_engine;
pub mod error;
pub mod security;
pub mod task_store;

use tauri::Manager;

use download_engine::single_stream::{self, DownloadRequest};
use download_engine::{CancelToken, DownloadComplete, NetPolicy};
use error::{EngineError, ErrorCode};
use task_store::{DownloadTask, TaskState, TaskStore};

/// Start a single-stream download and return the finalized result.
///
/// Progress events, cancellation wiring, and persisting the running transfer to
/// the task store arrive in Phase 4; this command exercises the real engine end
/// to end from the Tauri shell.
#[tauri::command]
async fn start_download(req: DownloadRequest) -> Result<DownloadComplete, EngineError> {
    let client = single_stream::build_client()?;
    let cancel = CancelToken::new();
    single_stream::download(&client, &req, NetPolicy::default(), &cancel, |_progress| {}).await
}

/// List all tasks in the history, newest first.
#[tauri::command]
fn list_downloads(store: tauri::State<'_, TaskStore>) -> Result<Vec<DownloadTask>, EngineError> {
    store.list().map_err(store_err)
}

/// Fetch one task's details.
#[tauri::command]
fn get_download_details(
    id: String,
    store: tauri::State<'_, TaskStore>,
) -> Result<Option<DownloadTask>, EngineError> {
    store.get(&id).map_err(store_err)
}

/// Retry a failed task by moving it back to the queue.
#[tauri::command]
fn retry_download(id: String, store: tauri::State<'_, TaskStore>) -> Result<(), EngineError> {
    store
        .update_state(&id, TaskState::Queued)
        .map_err(store_err)
}

/// Cancel a task (valid from any live state).
#[tauri::command]
fn cancel_download(id: String, store: tauri::State<'_, TaskStore>) -> Result<(), EngineError> {
    store
        .update_state(&id, TaskState::Cancelled)
        .map_err(store_err)
}

/// Remove a task from the history. Returns whether a row was deleted.
#[tauri::command]
fn remove_download(id: String, store: tauri::State<'_, TaskStore>) -> Result<bool, EngineError> {
    store.remove(&id).map_err(store_err)
}

/// Map a store error to the engine error taxonomy for the UI boundary.
fn store_err(e: task_store::StoreError) -> EngineError {
    EngineError::new(ErrorCode::InternalError, e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Persist tasks under the app's data directory.
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let store = TaskStore::open(&data_dir.join("tasks.db"))?;
            // Recover tasks interrupted by an unclean shutdown.
            let recovered = store.recover_interrupted()?;
            if recovered > 0 {
                eprintln!("Recovered {recovered} interrupted task(s) to queued.");
            }
            app.manage(store);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_download,
            list_downloads,
            get_download_details,
            retry_download,
            cancel_download,
            remove_download,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
