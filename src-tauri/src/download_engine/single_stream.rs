//! Single-stream HTTP/HTTPS download, with a resumable/pausable variant.

use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use serde::Deserialize;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use url::Url;

use crate::download_engine::fsutil::{TempGuard, finalize, map_io_error, temp_path_for};
use crate::download_engine::net::{map_reqwest_error, map_status, send_get};
use crate::download_engine::range::{Validators, parse_content_range};
use crate::download_engine::{CancelToken, DownloadComplete, NetPolicy, PauseToken, Progress};
use crate::error::{EngineError, ErrorCode};
use crate::security::filename::{
    filename_from_content_disposition, filename_from_url, resolve_destination, sanitize_filename,
};
use crate::security::url::validate_download_url;

// The HTTP client builder lives in `net`; re-export for a stable API.
pub use crate::download_engine::net::build_client;

/// A download request as received from a command or test.
#[derive(Debug, Clone, Deserialize)]
pub struct DownloadRequest {
    /// The source URL (untrusted).
    pub url: String,
    /// The user-approved destination directory (must already exist).
    pub dest_dir: PathBuf,
    /// Optional explicit filename; otherwise derived from the response/URL.
    #[serde(default)]
    pub filename: Option<String>,
}

/// Where a resumable transfer stopped.
#[derive(Debug, Clone)]
pub enum StopState {
    /// The transfer finished and was finalized.
    Completed(DownloadComplete),
    /// The transfer was paused; the partial file is preserved.
    Paused { downloaded: u64 },
}

/// Download `req.url` into `req.dest_dir` as a single stream (no resume).
///
/// Cancellation removes the partial file.
pub async fn download<F>(
    client: &reqwest::Client,
    req: &DownloadRequest,
    policy: NetPolicy,
    cancel: &CancelToken,
    mut on_progress: F,
) -> Result<DownloadComplete, EngineError>
where
    F: FnMut(Progress),
{
    let validated = validate_download_url(&req.url)?;
    let (final_url, response) = send_get(client, &validated, policy, None, None).await?;

    let status = response.status();
    if !status.is_success() {
        return Err(map_status(status.as_u16()));
    }

    let filename = resolve_filename(req, response.headers(), &final_url);
    let dest = resolve_destination(&req.dest_dir, &filename)?;
    if dest.exists() {
        return Err(destination_exists(&dest));
    }

    let total = response.content_length();

    let temp_path = temp_path_for(&dest);
    let mut guard = TempGuard::new(temp_path.clone());
    let mut file = tokio::fs::File::create(&temp_path)
        .await
        .map_err(map_io_error)?;

    let mut downloaded: u64 = 0;
    on_progress(Progress { downloaded, total });

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        if cancel.is_cancelled() {
            return Err(EngineError::new(
                ErrorCode::Cancelled,
                "The download was cancelled.",
            ));
        }
        let bytes = chunk.map_err(|e| map_reqwest_error(&final_url, e))?;
        file.write_all(&bytes).await.map_err(map_io_error)?;
        downloaded += bytes.len() as u64;

        if let Some(expected) = total
            && downloaded > expected
        {
            return Err(EngineError::new(
                ErrorCode::ResourceChanged,
                "The server sent more data than it declared.",
            ));
        }
        on_progress(Progress { downloaded, total });
    }

    if let Some(expected) = total
        && downloaded != expected
    {
        return Err(EngineError::new(
            ErrorCode::NetworkError,
            "The connection ended before the whole file was received.",
        ));
    }

    file.flush().await.map_err(map_io_error)?;
    file.sync_all().await.map_err(map_io_error)?;
    drop(file);

    finalize(&temp_path, &dest).await?;
    guard.disarm();

    Ok(DownloadComplete {
        path: dest,
        bytes: downloaded,
    })
}

/// Download into a known `dest`, resuming from an existing `.part` file when
/// present and honoring cooperative pause/cancel.
///
/// On resume, a `Range` request with `If-Range` is used; if the server answers
/// `200` (range ignored or the resource changed) the partial is discarded and
/// the transfer restarts from zero — old and new bytes are never merged.
/// Pausing preserves the partial file; cancelling removes it.
#[allow(clippy::too_many_arguments)]
pub async fn download_resumable<F>(
    client: &reqwest::Client,
    url: &Url,
    policy: NetPolicy,
    dest: &Path,
    validators: Option<&Validators>,
    cancel: &CancelToken,
    pause: &PauseToken,
    mut on_progress: F,
) -> Result<StopState, EngineError>
where
    F: FnMut(Progress),
{
    if dest.exists() {
        return Err(destination_exists(dest));
    }

    let temp_path = part_path(dest);
    let existing = tokio::fs::metadata(&temp_path).await.ok().map(|m| m.len());
    let resume_from = existing.unwrap_or(0);

    let if_range = validators.and_then(|v| v.if_range_value());
    let range = (resume_from > 0).then(|| format!("bytes={resume_from}-"));
    let (final_url, response) =
        send_get(client, url, policy, range.as_deref(), if_range.as_deref()).await?;

    let status = response.status();
    if !status.is_success() {
        return Err(map_status(status.as_u16()));
    }

    // Decide whether we are appending (206 with a matching start) or starting
    // over (200, i.e. the server ignored the range or the resource changed).
    let (mut downloaded, total, append) = if resume_from > 0 && status.as_u16() == 206 {
        let cr = response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_range)
            .ok_or_else(|| {
                EngineError::new(
                    ErrorCode::RangeInvalid,
                    "The resume response had no valid Content-Range.",
                )
            })?;
        if cr.start != resume_from {
            return Err(EngineError::new(
                ErrorCode::RangeInvalid,
                "The server resumed from the wrong offset.",
            ));
        }
        (resume_from, cr.total, true)
    } else {
        // Fresh download (or forced restart). The total is the full length.
        (0u64, response.content_length(), false)
    };

    let mut file = if append {
        let mut f = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&temp_path)
            .await
            .map_err(map_io_error)?;
        f.seek(std::io::SeekFrom::Start(resume_from))
            .await
            .map_err(map_io_error)?;
        f
    } else {
        // Overwrite any stale partial from a changed resource.
        tokio::fs::File::create(&temp_path)
            .await
            .map_err(map_io_error)?
    };

    on_progress(Progress { downloaded, total });

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        if cancel.is_cancelled() {
            drop(file);
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(EngineError::new(
                ErrorCode::Cancelled,
                "The download was cancelled.",
            ));
        }
        if pause.is_paused() {
            file.flush().await.map_err(map_io_error)?;
            file.sync_all().await.map_err(map_io_error)?;
            return Ok(StopState::Paused { downloaded });
        }

        let bytes = chunk.map_err(|e| map_reqwest_error(&final_url, e))?;
        file.write_all(&bytes).await.map_err(map_io_error)?;
        downloaded += bytes.len() as u64;

        if let Some(expected) = total
            && downloaded > expected
        {
            return Err(EngineError::new(
                ErrorCode::ResourceChanged,
                "The server sent more data than it declared.",
            ));
        }
        on_progress(Progress { downloaded, total });
    }

    if let Some(expected) = total
        && downloaded != expected
    {
        // Keep the partial so a later resume can continue.
        return Err(EngineError::new(
            ErrorCode::NetworkError,
            "The connection ended before the whole file was received.",
        ));
    }

    file.flush().await.map_err(map_io_error)?;
    file.sync_all().await.map_err(map_io_error)?;
    drop(file);

    finalize(&temp_path, dest).await?;
    Ok(StopState::Completed(DownloadComplete {
        path: dest.to_path_buf(),
        bytes: downloaded,
    }))
}

/// Resolve the output filename: explicit override, else Content-Disposition,
/// else the URL's last path segment.
pub(crate) fn resolve_filename(
    req: &DownloadRequest,
    headers: &reqwest::header::HeaderMap,
    final_url: &Url,
) -> String {
    if let Some(name) = req.filename.as_deref() {
        return sanitize_filename(name);
    }
    if let Some(name) = headers
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(filename_from_content_disposition)
    {
        return name;
    }
    filename_from_url(final_url)
}

fn destination_exists(dest: &Path) -> EngineError {
    EngineError::new(
        ErrorCode::DestinationExists,
        format!(
            "A file named '{}' already exists in the destination folder.",
            dest.file_name().and_then(|n| n.to_str()).unwrap_or("")
        ),
    )
}

/// The deterministic partial-file path next to a known destination.
fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download")
        .to_string();
    name.push_str(".part");
    dest.with_file_name(name)
}
