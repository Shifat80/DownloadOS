//! DownloadOS core library (Tauri backend).

pub mod download_engine;
pub mod error;
pub mod manager;
pub mod security;
pub mod task_store;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::{Manager, State};

use download_engine::range::{Validators, probe};
use download_engine::single_stream::part_path;
use download_engine::{NetPolicy, net};
use error::{EngineError, ErrorCode, redact_url};
use manager::{ControlRegistry, emit_changed, spawn_run};
use security::filename::{filename_from_url, resolve_destination, sanitize_filename};
use security::url::validate_download_url;
use task_store::{DownloadMode, DownloadTask, StoreError, TaskState, TaskStore};

type Store<'a> = State<'a, Arc<TaskStore>>;
type Registry<'a> = State<'a, Arc<ControlRegistry>>;
type HttpClient<'a> = State<'a, reqwest::Client>;

fn policy() -> NetPolicy {
    NetPolicy::default()
}

/// Add a new download: probe the resource, create a task row, and start it.
#[tauri::command]
async fn add_download(
    app: tauri::AppHandle,
    store: Store<'_>,
    registry: Registry<'_>,
    client: HttpClient<'_>,
    url: String,
    dest_dir: String,
    filename: Option<String>,
) -> Result<DownloadTask, EngineError> {
    let validated = validate_download_url(&url)?;

    // Ensure the destination directory exists before resolving into it.
    std::fs::create_dir_all(&dest_dir).map_err(|e| {
        EngineError::new(
            ErrorCode::PermissionDenied,
            format!("Could not create the destination folder: {e}"),
        )
    })?;

    let (final_url, info) = probe(client.inner(), &validated, policy()).await?;

    let chosen = if let Some(name) = filename.as_deref().filter(|s| !s.trim().is_empty()) {
        sanitize_filename(name)
    } else if let Some(name) = info.suggested_filename.clone() {
        name
    } else {
        filename_from_url(&final_url)
    };

    let dest = resolve_destination(Path::new(&dest_dir), &chosen)?;
    if dest.exists() {
        return Err(EngineError::new(
            ErrorCode::DestinationExists,
            format!("A file named '{chosen}' already exists in the destination folder."),
        ));
    }

    let id = uuid::Uuid::new_v4().simple().to_string();
    let mut task = DownloadTask::new_queued(
        id.clone(),
        url,
        redact_url(validated.as_str()),
        chosen,
        dest.to_string_lossy().into_owned(),
        DownloadMode::Single,
    );
    task.total_bytes = info.total.map(|t| t as i64);
    task.etag = info.validators.etag.clone();
    task.last_modified = info.validators.last_modified.clone();

    store.create(&task).map_err(store_err)?;
    emit_changed(&app);

    spawn_run(
        app,
        store.inner().clone(),
        registry.inner().clone(),
        client.inner().clone(),
        policy(),
        id,
        final_url,
        dest,
        info.validators,
    );

    Ok(task)
}

/// Pause a running download (preserves the partial file).
#[tauri::command]
fn pause_download(id: String, registry: Registry<'_>) -> Result<(), EngineError> {
    if !registry.request_pause(&id) {
        return Err(EngineError::new(
            ErrorCode::InternalError,
            "That download is not currently running.",
        ));
    }
    Ok(())
}

/// Resume a paused download from its partial file.
#[tauri::command]
async fn resume_download(
    app: tauri::AppHandle,
    store: Store<'_>,
    registry: Registry<'_>,
    client: HttpClient<'_>,
    id: String,
) -> Result<(), EngineError> {
    let task = require_task(&store, &id)?;
    if task.state != TaskState::Paused {
        return Err(EngineError::new(
            ErrorCode::InternalError,
            "Only a paused download can be resumed.",
        ));
    }
    let validated = validate_download_url(&task.source_url)?;
    let dest = PathBuf::from(&task.destination_path);
    let validators = validators_of(&task);

    spawn_run(
        app,
        store.inner().clone(),
        registry.inner().clone(),
        client.inner().clone(),
        policy(),
        id,
        validated,
        dest,
        validators,
    );
    Ok(())
}

/// Cancel a download (running, queued, or paused).
#[tauri::command]
fn cancel_download(
    app: tauri::AppHandle,
    store: Store<'_>,
    registry: Registry<'_>,
    id: String,
) -> Result<(), EngineError> {
    if registry.request_cancel(&id) {
        // The running task will persist Cancelled and clean up its partial.
        return Ok(());
    }
    // Not running: drop any partial and mark cancelled directly.
    if let Ok(Some(task)) = store.get(&id) {
        let _ = std::fs::remove_file(part_path(Path::new(&task.destination_path)));
    }
    store
        .update_state(&id, TaskState::Cancelled)
        .map_err(store_err)?;
    emit_changed(&app);
    Ok(())
}

/// Retry a failed download from scratch.
#[tauri::command]
async fn retry_download(
    app: tauri::AppHandle,
    store: Store<'_>,
    registry: Registry<'_>,
    client: HttpClient<'_>,
    id: String,
) -> Result<(), EngineError> {
    let task = require_task(&store, &id)?;
    if task.state != TaskState::Failed {
        return Err(EngineError::new(
            ErrorCode::InternalError,
            "Only a failed download can be retried.",
        ));
    }
    let dest = PathBuf::from(&task.destination_path);
    let _ = std::fs::remove_file(part_path(&dest));
    let validated = validate_download_url(&task.source_url)?;
    let validators = validators_of(&task);

    store
        .update_state(&id, TaskState::Queued)
        .map_err(store_err)?;
    let _ = store.update_progress(&id, 0, task.total_bytes);
    emit_changed(&app);

    spawn_run(
        app,
        store.inner().clone(),
        registry.inner().clone(),
        client.inner().clone(),
        policy(),
        id,
        validated,
        dest,
        validators,
    );
    Ok(())
}

/// Remove a task from history (cancels it first if running).
#[tauri::command]
fn remove_download(
    app: tauri::AppHandle,
    store: Store<'_>,
    registry: Registry<'_>,
    id: String,
) -> Result<bool, EngineError> {
    registry.request_cancel(&id);
    let removed = store.remove(&id).map_err(store_err)?;
    emit_changed(&app);
    Ok(removed)
}

/// List all tasks, newest first.
#[tauri::command]
fn list_downloads(store: Store<'_>) -> Result<Vec<DownloadTask>, EngineError> {
    store.list().map_err(store_err)
}

/// Fetch one task's details.
#[tauri::command]
fn get_download_details(id: String, store: Store<'_>) -> Result<Option<DownloadTask>, EngineError> {
    store.get(&id).map_err(store_err)
}

/// The OS default downloads directory, for the UI's initial destination.
#[tauri::command]
fn default_download_dir(app: tauri::AppHandle) -> Result<String, EngineError> {
    app.path()
        .download_dir()
        .or_else(|_| app.path().home_dir())
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|_| {
            EngineError::new(
                ErrorCode::InternalError,
                "Could not determine a default downloads folder.",
            )
        })
}

fn require_task(store: &Arc<TaskStore>, id: &str) -> Result<DownloadTask, EngineError> {
    store.get(id).map_err(store_err)?.ok_or_else(|| {
        EngineError::new(ErrorCode::InternalError, "That download no longer exists.")
    })
}

fn validators_of(task: &DownloadTask) -> Validators {
    Validators {
        etag: task.etag.clone(),
        last_modified: task.last_modified.clone(),
    }
}

/// Map a store error to the engine error taxonomy for the UI boundary.
fn store_err(e: StoreError) -> EngineError {
    EngineError::new(ErrorCode::InternalError, e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;

            let store = Arc::new(TaskStore::open(&data_dir.join("tasks.db"))?);
            let recovered = store.recover_interrupted()?;
            if recovered > 0 {
                eprintln!("Recovered {recovered} interrupted task(s) to queued.");
            }

            let client = net::build_client().map_err(|e| e.to_string())?;

            app.manage(store);
            app.manage(Arc::new(ControlRegistry::default()));
            app.manage(client);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            add_download,
            pause_download,
            resume_download,
            cancel_download,
            retry_download,
            remove_download,
            list_downloads,
            get_download_details,
            default_download_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
