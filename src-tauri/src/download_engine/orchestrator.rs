//! High-level download entry point: probe, choose a strategy, and retry.
//!
//! Chooses segmented transfer when the server verifiably supports ranges and
//! the resource is large enough to benefit; otherwise falls back to a single
//! stream. Transient failures are retried with bounded backoff.

use crate::download_engine::range::probe;
use crate::download_engine::retry::{
    DEFAULT_BASE_DELAY, DEFAULT_MAX_ATTEMPTS, DEFAULT_MAX_DELAY, with_retries,
};
use crate::download_engine::segmented::{SegmentPlanConfig, download_segmented};
use crate::download_engine::single_stream::{self, DownloadRequest};
use crate::download_engine::{CancelToken, DownloadComplete, NetPolicy, Progress};
use crate::error::{EngineError, ErrorCode};
use crate::security::filename::{filename_from_url, sanitize_filename};
use crate::security::url::validate_download_url;

/// Resources at least this large use segmented transfer when ranges are
/// supported; smaller ones are not worth the extra requests.
const MIN_SEGMENTED_SIZE: u64 = 2 * 1024 * 1024;

/// Download a resource, choosing the best verified strategy and retrying
/// transient failures.
pub async fn download_resource<F>(
    client: &reqwest::Client,
    req: &DownloadRequest,
    policy: NetPolicy,
    cancel: &CancelToken,
    on_progress: F,
) -> Result<DownloadComplete, EngineError>
where
    F: Fn(Progress) + Sync,
{
    let validated = validate_download_url(&req.url)?;

    let op = || async {
        let (final_url, info) = probe(client, &validated, policy).await?;

        let use_segmented =
            info.accepts_ranges && info.total.is_some_and(|t| t >= MIN_SEGMENTED_SIZE);

        if use_segmented {
            // Resolve the destination here (segmented needs a concrete path).
            let filename = if let Some(name) = req.filename.as_deref() {
                sanitize_filename(name)
            } else if let Some(name) = info.suggested_filename.clone() {
                name
            } else {
                filename_from_url(&final_url)
            };
            let dest = crate::security::filename::resolve_destination(&req.dest_dir, &filename)?;
            if dest.exists() {
                return Err(EngineError::new(
                    ErrorCode::DestinationExists,
                    format!(
                        "A file named '{}' already exists in the destination folder.",
                        dest.file_name().and_then(|n| n.to_str()).unwrap_or("")
                    ),
                ));
            }
            download_segmented(
                client,
                &final_url,
                policy,
                &info,
                &dest,
                SegmentPlanConfig::default(),
                cancel,
                &on_progress,
            )
            .await
        } else {
            // Fallback: single stream (resolves its own filename/destination).
            single_stream::download(client, req, policy, cancel, &on_progress).await
        }
    };

    with_retries(
        DEFAULT_MAX_ATTEMPTS,
        DEFAULT_BASE_DELAY,
        DEFAULT_MAX_DELAY,
        cancel,
        op,
    )
    .await
}
