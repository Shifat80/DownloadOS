//! Download manager: ties the engine, task store, and UI together.
//!
//! Owns the control tokens for running transfers, runs each download on a
//! background task, persists real byte progress and lifecycle state, and emits
//! events the UI listens to. There is no synthetic progress: every number
//! comes from bytes actually written.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use url::Url;

use crate::download_engine::range::Validators;
use crate::download_engine::single_stream::{self, StopState};
use crate::download_engine::{CancelToken, NetPolicy, PauseToken, Progress};
use crate::error::EngineError;
use crate::task_store::{TaskState, TaskStore};

/// Event emitted (throttled) as bytes are written.
#[derive(Debug, Clone, Serialize)]
pub struct ProgressEvent {
    pub id: String,
    pub downloaded: u64,
    pub total: Option<u64>,
}

/// Registry of control handles for currently running downloads.
#[derive(Default)]
pub struct ControlRegistry {
    inner: Mutex<HashMap<String, (CancelToken, PauseToken)>>,
}

impl ControlRegistry {
    fn insert(&self, id: &str, cancel: CancelToken, pause: PauseToken) {
        self.inner
            .lock()
            .expect("control registry mutex")
            .insert(id.to_string(), (cancel, pause));
    }

    fn get(&self, id: &str) -> Option<(CancelToken, PauseToken)> {
        self.inner
            .lock()
            .expect("control registry mutex")
            .get(id)
            .cloned()
    }

    fn remove(&self, id: &str) {
        self.inner
            .lock()
            .expect("control registry mutex")
            .remove(id);
    }

    /// Whether a download with this id is currently running.
    pub fn is_running(&self, id: &str) -> bool {
        self.inner
            .lock()
            .expect("control registry mutex")
            .contains_key(id)
    }

    /// Signal a running download to pause. Returns false if not running.
    pub fn request_pause(&self, id: &str) -> bool {
        match self.get(id) {
            Some((_, pause)) => {
                pause.pause();
                true
            }
            None => false,
        }
    }

    /// Signal a running download to cancel. Returns false if not running.
    pub fn request_cancel(&self, id: &str) -> bool {
        match self.get(id) {
            Some((cancel, _)) => {
                cancel.cancel();
                true
            }
            None => false,
        }
    }
}

/// Emit a "downloads changed" signal so the UI refreshes its list.
pub fn emit_changed(app: &AppHandle) {
    let _ = app.emit("downloads:changed", ());
}

/// Spawn the background task that runs (or resumes) a download.
#[allow(clippy::too_many_arguments)]
pub fn spawn_run(
    app: AppHandle,
    store: Arc<TaskStore>,
    registry: Arc<ControlRegistry>,
    client: reqwest::Client,
    policy: NetPolicy,
    id: String,
    url: Url,
    dest: PathBuf,
    validators: Validators,
) {
    tauri::async_runtime::spawn(async move {
        run_download(
            app, store, registry, client, policy, id, url, dest, validators,
        )
        .await;
    });
}

#[allow(clippy::too_many_arguments)]
async fn run_download(
    app: AppHandle,
    store: Arc<TaskStore>,
    registry: Arc<ControlRegistry>,
    client: reqwest::Client,
    policy: NetPolicy,
    id: String,
    url: Url,
    dest: PathBuf,
    validators: Validators,
) {
    let cancel = CancelToken::new();
    let pause = PauseToken::new();
    registry.insert(&id, cancel.clone(), pause.clone());

    // Advance to Downloading (tolerate benign transition races).
    let _ = store.update_state(&id, TaskState::Resolving);
    let _ = store.update_state(&id, TaskState::Downloading);
    emit_changed(&app);

    let validators_opt = if validators.etag.is_some() || validators.last_modified.is_some() {
        Some(validators)
    } else {
        None
    };

    let result = {
        let store = store.clone();
        let app = app.clone();
        let id = id.clone();
        let mut last_emit = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);

        single_stream::download_resumable(
            &client,
            &url,
            policy,
            &dest,
            validators_opt.as_ref(),
            &cancel,
            &pause,
            move |p: Progress| {
                let finished = p.total == Some(p.downloaded);
                let now = Instant::now();
                if finished || now.duration_since(last_emit) >= Duration::from_millis(200) {
                    last_emit = now;
                    let _ =
                        store.update_progress(&id, p.downloaded as i64, p.total.map(|t| t as i64));
                    let _ = app.emit(
                        "download:progress",
                        ProgressEvent {
                            id: id.clone(),
                            downloaded: p.downloaded,
                            total: p.total,
                        },
                    );
                }
            },
        )
        .await
    };

    registry.remove(&id);
    apply_result(&store, &id, result);
    emit_changed(&app);
}

/// Persist the terminal state corresponding to a transfer outcome.
fn apply_result(store: &TaskStore, id: &str, result: Result<StopState, EngineError>) {
    match result {
        Ok(StopState::Completed(_)) => {
            let _ = store.update_state(id, TaskState::Verifying);
            let _ = store.update_state(id, TaskState::Completed);
        }
        Ok(StopState::Paused { .. }) => {
            let _ = store.update_state(id, TaskState::Paused);
        }
        Err(e) if e.code() == crate::error::ErrorCode::Cancelled => {
            let _ = store.update_state(id, TaskState::Cancelled);
        }
        Err(e) => {
            let _ = store.set_error(id, e.code().as_str());
            let _ = store.update_state(id, TaskState::Failed);
        }
    }
}
