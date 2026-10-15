//! DownloadOS core library (Tauri backend).

pub mod bridge;
pub mod download_engine;
pub mod error;
pub mod manager;
pub mod security;
pub mod task_store;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{Manager, State};

use bridge::{BridgeConfig, HandoffAck, HandoffDenied, HandoffRequest};
use download_engine::range::{Validators, probe};
use download_engine::single_stream::part_path;
use download_engine::{NetPolicy, net};
use error::{EngineError, ErrorCode, redact_url};
use manager::{ControlRegistry, MIN_SEGMENT_LEN, RunSpec, emit_changed, spawn_run};
use security::filename::{filename_from_url, resolve_destination, sanitize_filename};
use security::url::validate_download_url;
use task_store::{DownloadMode, DownloadTask, StoreError, TaskState, TaskStore};

type Store<'a> = State<'a, Arc<TaskStore>>;
type Registry<'a> = State<'a, Arc<ControlRegistry>>;
type HttpClient<'a> = State<'a, reqwest::Client>;

/// Resources at least this large use segmented transfer when ranges are
/// supported (and more than one connection is configured).
const MIN_SEGMENTED_SIZE: u64 = 2 * MIN_SEGMENT_LEN;
const DEFAULT_CONNECTIONS: usize = 4;
const MAX_CONNECTIONS: usize = 16;

fn policy() -> NetPolicy {
    NetPolicy::default()
}

/// Persisted, user-facing settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Settings {
    download_dir: String,
    connections: usize,
}

#[tauri::command]
fn get_settings(app: tauri::AppHandle, store: Store<'_>) -> Result<Settings, EngineError> {
    Ok(Settings {
        download_dir: resolve_dest_dir(&app, &store)?,
        connections: read_connections(&store),
    })
}

#[tauri::command]
fn set_settings(
    store: Store<'_>,
    download_dir: Option<String>,
    connections: Option<usize>,
) -> Result<(), EngineError> {
    if let Some(dir) = download_dir.filter(|d| !d.trim().is_empty()) {
        store
            .set_setting("download_dir", dir.trim())
            .map_err(store_err)?;
    }
    if let Some(c) = connections {
        let clamped = c.clamp(1, MAX_CONNECTIONS);
        store
            .set_setting("connections", &clamped.to_string())
            .map_err(store_err)?;
    }
    Ok(())
}

/// Add a new download: probe the resource, create a task row, and start it.
#[tauri::command]
async fn add_download(
    app: tauri::AppHandle,
    store: Store<'_>,
    registry: Registry<'_>,
    client: HttpClient<'_>,
    url: String,
    filename: Option<String>,
) -> Result<DownloadTask, EngineError> {
    start_new_download(
        &app,
        store.inner(),
        registry.inner(),
        client.inner(),
        url,
        filename,
    )
    .await
}

/// Shared core used by both the `add_download` command and the browser bridge.
async fn start_new_download(
    app: &tauri::AppHandle,
    store: &Arc<TaskStore>,
    registry: &Arc<ControlRegistry>,
    client: &reqwest::Client,
    url: String,
    filename: Option<String>,
) -> Result<DownloadTask, EngineError> {
    let validated = validate_download_url(&url)?;
    let dest_dir = resolve_dest_dir(app, store)?;
    std::fs::create_dir_all(&dest_dir).map_err(|e| {
        EngineError::new(
            ErrorCode::PermissionDenied,
            format!("Could not create the destination folder: {e}"),
        )
    })?;

    let (final_url, info) = probe(client, &validated, policy()).await?;

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

    let connections = read_connections(store);
    let segmented = info.accepts_ranges
        && info.total.is_some_and(|t| t >= MIN_SEGMENTED_SIZE)
        && connections > 1;

    let id = uuid::Uuid::new_v4().simple().to_string();
    let mut task = DownloadTask::new_queued(
        id.clone(),
        url,
        redact_url(validated.as_str()),
        chosen,
        dest.to_string_lossy().into_owned(),
        if segmented {
            DownloadMode::Segmented
        } else {
            DownloadMode::Single
        },
    );
    task.total_bytes = info.total.map(|t| t as i64);
    task.etag = info.validators.etag.clone();
    task.last_modified = info.validators.last_modified.clone();

    store.create(&task).map_err(store_err)?;
    emit_changed(app);

    spawn_run(
        app.clone(),
        store.clone(),
        registry.clone(),
        client.clone(),
        policy(),
        RunSpec {
            id,
            url: final_url,
            dest,
            validators: info.validators,
            total: info.total,
            connections,
            segmented,
        },
    );

    Ok(task)
}

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
    spawn_run(
        app,
        store.inner().clone(),
        registry.inner().clone(),
        client.inner().clone(),
        policy(),
        spec_from_task(&store, &task)?,
    );
    Ok(())
}

#[tauri::command]
fn cancel_download(
    app: tauri::AppHandle,
    store: Store<'_>,
    registry: Registry<'_>,
    id: String,
) -> Result<(), EngineError> {
    if registry.request_cancel(&id) {
        return Ok(());
    }
    if let Ok(Some(task)) = store.get(&id) {
        let _ = std::fs::remove_file(part_path(Path::new(&task.destination_path)));
        let _ = store.clear_segments(&id);
    }
    store
        .update_state(&id, TaskState::Cancelled)
        .map_err(store_err)?;
    emit_changed(&app);
    Ok(())
}

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
    let _ = store.clear_segments(&id);

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
        spec_from_task(&store, &task)?,
    );
    Ok(())
}

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

#[tauri::command]
fn list_downloads(store: Store<'_>) -> Result<Vec<DownloadTask>, EngineError> {
    store.list().map_err(store_err)
}

#[tauri::command]
fn get_download_details(id: String, store: Store<'_>) -> Result<Option<DownloadTask>, EngineError> {
    store.get(&id).map_err(store_err)
}

/// Browser-pairing info shown in Settings (the port + pairing code).
#[derive(Debug, Clone, Serialize)]
struct BridgeInfo {
    port: u16,
    token: String,
}

#[tauri::command]
fn bridge_info(info: State<'_, BridgeInfo>) -> BridgeInfo {
    info.inner().clone()
}

// --- helpers ---

fn handoff_status(code: ErrorCode) -> u16 {
    match code {
        ErrorCode::DestinationExists => 409,
        ErrorCode::InvalidUrl | ErrorCode::UnsupportedScheme => 400,
        ErrorCode::AuthRequired | ErrorCode::AuthExpired => 502,
        ErrorCode::PolicyRestricted => 403,
        _ => 500,
    }
}

fn spec_from_task(store: &Arc<TaskStore>, task: &DownloadTask) -> Result<RunSpec, EngineError> {
    Ok(RunSpec {
        id: task.id.clone(),
        url: validate_download_url(&task.source_url)?,
        dest: PathBuf::from(&task.destination_path),
        validators: Validators {
            etag: task.etag.clone(),
            last_modified: task.last_modified.clone(),
        },
        total: task.total_bytes.map(|t| t as u64),
        connections: read_connections(store),
        segmented: task.mode == DownloadMode::Segmented,
    })
}

fn require_task(store: &Arc<TaskStore>, id: &str) -> Result<DownloadTask, EngineError> {
    store.get(id).map_err(store_err)?.ok_or_else(|| {
        EngineError::new(ErrorCode::InternalError, "That download no longer exists.")
    })
}

fn read_connections(store: &Arc<TaskStore>) -> usize {
    store
        .get_setting("connections")
        .ok()
        .flatten()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(DEFAULT_CONNECTIONS)
        .clamp(1, MAX_CONNECTIONS)
}

fn resolve_dest_dir(app: &tauri::AppHandle, store: &Arc<TaskStore>) -> Result<String, EngineError> {
    if let Ok(Some(dir)) = store.get_setting("download_dir")
        && !dir.trim().is_empty()
    {
        return Ok(dir);
    }
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
            let registry = Arc::new(ControlRegistry::default());

            // Browser-handoff bridge (loopback only). Reuse a persisted pairing
            // token so the extension stays paired across restarts.
            let token = match store.get_setting("bridge_token") {
                Ok(Some(t)) if !t.is_empty() => t,
                _ => {
                    let t = uuid::Uuid::new_v4().simple().to_string();
                    let _ = store.set_setting("bridge_token", &t);
                    t
                }
            };
            let enqueue: bridge::EnqueueFn = {
                let app_h = app.handle().clone();
                let store_b = store.clone();
                let registry_b = registry.clone();
                let client_b = client.clone();
                Arc::new(move |req: HandoffRequest| {
                    let app_h = app_h.clone();
                    let store_b = store_b.clone();
                    let registry_b = registry_b.clone();
                    let client_b = client_b.clone();
                    tauri::async_runtime::block_on(async move {
                        start_new_download(
                            &app_h,
                            &store_b,
                            &registry_b,
                            &client_b,
                            req.url,
                            req.filename,
                        )
                        .await
                    })
                    .map(|t| HandoffAck {
                        id: t.id,
                        filename: t.filename,
                    })
                    .map_err(|e| {
                        HandoffDenied::new(handoff_status(e.code()), e.code().as_str(), e.message)
                    })
                })
            };
            let bridge_port = match bridge::spawn(
                BridgeConfig {
                    port: 14900,
                    token: token.clone(),
                },
                enqueue,
            ) {
                Ok(handle) => handle.port,
                Err(e) => {
                    eprintln!("Browser bridge failed to start: {e}");
                    0
                }
            };

            app.manage(store);
            app.manage(registry);
            app.manage(client);
            app.manage(BridgeInfo {
                port: bridge_port,
                token,
            });
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
            get_settings,
            set_settings,
            bridge_info,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
