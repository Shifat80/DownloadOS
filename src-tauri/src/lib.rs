//! DownloadOS core library (Tauri backend).

pub mod download_engine;
pub mod error;
pub mod security;

use download_engine::single_stream::{self, DownloadRequest};
use download_engine::{CancelToken, DownloadComplete, NetPolicy};
use error::EngineError;

/// Start a single-stream download and return the finalized result.
///
/// Progress events and cancellation wiring to the UI arrive in Phase 4; this
/// command exercises the real engine end to end from the Tauri shell.
#[tauri::command]
async fn start_download(req: DownloadRequest) -> Result<DownloadComplete, EngineError> {
    let client = single_stream::build_client()?;
    let cancel = CancelToken::new();
    single_stream::download(&client, &req, NetPolicy::default(), &cancel, |_progress| {}).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![start_download])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
