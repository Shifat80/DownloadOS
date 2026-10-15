//! Download manager: ties the engine, task store, and UI together.
//!
//! Owns control tokens for running transfers, runs each download on a
//! background task (multi-connection segmented when the server supports ranges,
//! single-stream otherwise), persists real byte progress and segment
//! checkpoints, and emits events the UI listens to. No synthetic progress.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use url::Url;

use crate::download_engine::fsutil::finalize;
use crate::download_engine::range::{Segment, Validators};
use crate::download_engine::segmented::{SegmentedOutcome, download_segments};
use crate::download_engine::single_stream::{self, StopState, part_path};
use crate::download_engine::{CancelToken, NetPolicy, PauseToken, Progress};
use crate::error::{EngineError, ErrorCode};
use crate::task_store::{TaskState, TaskStore};

/// Minimum bytes per segment when planning a segmented download.
pub const MIN_SEGMENT_LEN: u64 = 1024 * 1024;
/// Progress is persisted/emitted at most this often (milliseconds).
const PROGRESS_THROTTLE_MS: u64 = 200;

/// Event emitted (throttled) as bytes are written.
#[derive(Debug, Clone, Serialize)]
pub struct ProgressEvent {
    pub id: String,
    pub downloaded: u64,
    pub total: Option<u64>,
}

/// Everything needed to run (or resume) one download.
pub struct RunSpec {
    pub id: String,
    pub url: Url,
    pub dest: PathBuf,
    pub validators: Validators,
    pub total: Option<u64>,
    pub connections: usize,
    pub segmented: bool,
}

#[derive(Debug, Clone, Copy)]
enum RunOutcome {
    Completed,
    Paused,
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

    pub fn is_running(&self, id: &str) -> bool {
        self.inner
            .lock()
            .expect("control registry mutex")
            .contains_key(id)
    }

    pub fn request_pause(&self, id: &str) -> bool {
        match self.get(id) {
            Some((_, pause)) => {
                pause.pause();
                true
            }
            None => false,
        }
    }

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
pub fn spawn_run(
    app: AppHandle,
    store: Arc<TaskStore>,
    registry: Arc<ControlRegistry>,
    client: reqwest::Client,
    policy: NetPolicy,
    spec: RunSpec,
) {
    tauri::async_runtime::spawn(async move {
        run_download(app, store, registry, client, policy, spec).await;
    });
}

async fn run_download(
    app: AppHandle,
    store: Arc<TaskStore>,
    registry: Arc<ControlRegistry>,
    client: reqwest::Client,
    policy: NetPolicy,
    spec: RunSpec,
) {
    let cancel = CancelToken::new();
    let pause = PauseToken::new();
    registry.insert(&spec.id, cancel.clone(), pause.clone());

    let _ = store.update_state(&spec.id, TaskState::Resolving);
    let _ = store.update_state(&spec.id, TaskState::Downloading);
    emit_changed(&app);

    let result = if spec.segmented && spec.total.is_some() {
        run_segmented(&app, &store, &client, policy, &spec, &cancel, &pause).await
    } else {
        run_single(&app, &store, &client, policy, &spec, &cancel, &pause).await
    };

    registry.remove(&spec.id);

    match result {
        Ok(RunOutcome::Completed) => {
            let _ = store.update_state(&spec.id, TaskState::Verifying);
            let _ = store.update_state(&spec.id, TaskState::Completed);
            let _ = store.clear_segments(&spec.id);
        }
        Ok(RunOutcome::Paused) => {
            let _ = store.update_state(&spec.id, TaskState::Paused);
        }
        Err(e) if e.code() == ErrorCode::Cancelled => {
            let _ = tokio::fs::remove_file(part_path(&spec.dest)).await;
            let _ = store.clear_segments(&spec.id);
            let _ = store.update_state(&spec.id, TaskState::Cancelled);
        }
        Err(e) => {
            let _ = store.set_error(&spec.id, e.code().as_str());
            let _ = store.update_state(&spec.id, TaskState::Failed);
        }
    }
    emit_changed(&app);
}

async fn run_segmented(
    app: &AppHandle,
    store: &Arc<TaskStore>,
    client: &reqwest::Client,
    policy: NetPolicy,
    spec: &RunSpec,
    cancel: &CancelToken,
    pause: &PauseToken,
) -> Result<RunOutcome, EngineError> {
    let total = spec.total.expect("segmented requires a total");
    let temp = part_path(&spec.dest);

    // Load the persisted plan, or create and persist one on first run.
    let mut rows = store.list_segments(&spec.id).map_err(store_err)?;
    if rows.is_empty() {
        let plan =
            crate::download_engine::range::plan_segments(total, spec.connections, MIN_SEGMENT_LEN);
        let tuples: Vec<(i64, i64, i64)> = plan
            .iter()
            .map(|s| (s.index as i64, s.start as i64, s.end as i64))
            .collect();
        store
            .replace_segments(&spec.id, &tuples)
            .map_err(store_err)?;
        rows = store.list_segments(&spec.id).map_err(store_err)?;
    }

    let plan: Vec<Segment> = rows
        .iter()
        .map(|r| Segment {
            index: r.idx as usize,
            start: r.start_byte as u64,
            end: r.end_byte as u64,
        })
        .collect();
    let pending: Vec<usize> = rows
        .iter()
        .filter(|r| !r.done)
        .map(|r| r.idx as usize)
        .collect();
    let already: u64 = rows
        .iter()
        .filter(|r| r.done)
        .map(|r| (r.end_byte - r.start_byte + 1) as u64)
        .sum();

    let sink = progress_sink(app.clone(), store.clone(), spec.id.clone());
    let store_done = store.clone();
    let id_done = spec.id.clone();
    let on_segment_done = move |idx: usize| {
        let _ = store_done.mark_segment_done(&id_done, idx as i64);
    };

    let outcome = download_segments(
        client,
        &spec.url,
        policy,
        &temp,
        total,
        &plan,
        &pending,
        already,
        Some(&spec.validators),
        spec.connections,
        cancel,
        pause,
        on_segment_done,
        sink,
    )
    .await?;

    match outcome {
        SegmentedOutcome::Completed => {
            finalize(&temp, &spec.dest).await?;
            Ok(RunOutcome::Completed)
        }
        SegmentedOutcome::Paused => Ok(RunOutcome::Paused),
    }
}

async fn run_single(
    app: &AppHandle,
    store: &Arc<TaskStore>,
    client: &reqwest::Client,
    policy: NetPolicy,
    spec: &RunSpec,
    cancel: &CancelToken,
    pause: &PauseToken,
) -> Result<RunOutcome, EngineError> {
    let validators = if spec.validators.etag.is_some() || spec.validators.last_modified.is_some() {
        Some(&spec.validators)
    } else {
        None
    };
    let sink = progress_sink(app.clone(), store.clone(), spec.id.clone());

    match single_stream::download_resumable(
        client, &spec.url, policy, &spec.dest, validators, cancel, pause, sink,
    )
    .await?
    {
        StopState::Completed(_) => Ok(RunOutcome::Completed),
        StopState::Paused { .. } => Ok(RunOutcome::Paused),
    }
}

/// Build a throttled progress callback that persists and emits real bytes.
fn progress_sink(app: AppHandle, store: Arc<TaskStore>, id: String) -> impl Fn(Progress) {
    let last = Arc::new(AtomicU64::new(0));
    move |p: Progress| {
        let finished = p.total == Some(p.downloaded);
        let now = now_millis();
        let prev = last.load(Ordering::SeqCst);
        if finished || now.saturating_sub(prev) >= PROGRESS_THROTTLE_MS {
            last.store(now, Ordering::SeqCst);
            let _ = store.update_progress(&id, p.downloaded as i64, p.total.map(|t| t as i64));
            let _ = app.emit(
                "download:progress",
                ProgressEvent {
                    id: id.clone(),
                    downloaded: p.downloaded,
                    total: p.total,
                },
            );
        }
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn store_err(e: crate::task_store::StoreError) -> EngineError {
    EngineError::new(ErrorCode::InternalError, e.to_string())
}
