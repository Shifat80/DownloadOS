//! Bounded segmented (parallel-range) download.
//!
//! Preconditions are established by a range [`probe`](crate::download_engine::range::probe):
//! the resource must report a trustworthy total size and honor ranges with
//! `206`. Each segment is fetched with a `Range` request, its `Content-Range`
//! is validated against what we asked for, resource validators are re-checked
//! (so a resource that changes mid-download is never silently merged), and the
//! bytes are written at the correct offset of a preallocated temp file.

use std::io::SeekFrom;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use futures_util::{StreamExt, TryStreamExt, stream};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use url::Url;

use crate::download_engine::fsutil::{TempGuard, finalize, map_io_error, temp_path_for};
use crate::download_engine::net::{map_reqwest_error, map_status, send_get};
use crate::download_engine::range::{
    ResourceInfo, Segment, Validators, parse_content_range, plan_segments,
    validate_segment_content_range,
};
use crate::download_engine::{CancelToken, DownloadComplete, NetPolicy, PauseToken, Progress};
use crate::error::{EngineError, ErrorCode};

/// Tuning knobs for segmentation.
#[derive(Debug, Clone, Copy)]
pub struct SegmentPlanConfig {
    pub max_segments: usize,
    pub min_segment_len: u64,
    pub concurrency: usize,
}

impl Default for SegmentPlanConfig {
    fn default() -> Self {
        Self {
            max_segments: 4,
            min_segment_len: 1024 * 1024, // 1 MiB
            concurrency: 4,
        }
    }
}

/// Download a range-capable resource into `dest` using segments.
///
/// `dest` must not already exist (the caller checks). `info` must come from a
/// successful probe with `accepts_ranges == true` and a known total size.
#[allow(clippy::too_many_arguments)]
pub async fn download_segmented<F>(
    client: &reqwest::Client,
    final_url: &Url,
    policy: NetPolicy,
    info: &ResourceInfo,
    dest: &Path,
    config: SegmentPlanConfig,
    cancel: &CancelToken,
    on_progress: F,
) -> Result<DownloadComplete, EngineError>
where
    F: Fn(Progress) + Sync,
{
    let total = info.total.ok_or_else(|| {
        EngineError::new(
            ErrorCode::RangeUnsupported,
            "The resource size is unknown; segmented download is not possible.",
        )
    })?;
    if !info.accepts_ranges {
        return Err(EngineError::new(
            ErrorCode::RangeUnsupported,
            "The resource does not support byte ranges.",
        ));
    }

    let plan = plan_segments(total, config.max_segments, config.min_segment_len);

    let temp_path = temp_path_for(dest);
    let mut guard = TempGuard::new(temp_path.clone());

    // Preallocate the output so each segment can write at its offset.
    {
        let file = tokio::fs::File::create(&temp_path)
            .await
            .map_err(map_io_error)?;
        file.set_len(total).await.map_err(map_io_error)?;
        file.sync_all().await.map_err(map_io_error)?;
    }

    let progress = Arc::new(AtomicU64::new(0));
    let if_range = info.validators.if_range_value();

    on_progress(Progress {
        downloaded: 0,
        total: Some(total),
    });

    // Fetch segments with bounded concurrency; abort all on the first error.
    let futures = plan.iter().map(|seg| {
        download_one_segment(
            client,
            final_url,
            policy,
            &info.validators,
            if_range.as_deref(),
            &temp_path,
            seg,
            total,
            cancel,
            &progress,
            &on_progress,
        )
    });
    stream::iter(futures)
        .buffer_unordered(config.concurrency.max(1))
        .try_collect::<Vec<()>>()
        .await?;

    // Sanity: the preallocated file must be exactly the expected size.
    let meta = tokio::fs::metadata(&temp_path)
        .await
        .map_err(map_io_error)?;
    if meta.len() != total {
        return Err(EngineError::new(
            ErrorCode::NetworkError,
            "The assembled file size did not match the expected total.",
        ));
    }

    finalize(&temp_path, dest).await?;
    guard.disarm();

    Ok(DownloadComplete {
        path: dest.to_path_buf(),
        bytes: total,
    })
}

/// Outcome of a resumable segmented run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentedOutcome {
    Completed,
    Paused,
}

#[derive(Debug, Clone, Copy)]
enum SegmentStatus {
    Done,
    Paused,
}

/// Fill a preallocated temp file using a fixed segment plan, fetching only the
/// `pending` segment indices with bounded concurrency. Segment completion is
/// reported via `on_segment_done` so the caller can persist checkpoints; this
/// function does not finalize (rename) — the caller does that on `Completed`.
///
/// Supports cooperative pause (stops without marking in-flight segments done,
/// preserving the file for resume) and cancel (returns `CANCELLED`).
#[allow(clippy::too_many_arguments)]
pub async fn download_segments<FP, FS>(
    client: &reqwest::Client,
    url: &Url,
    policy: NetPolicy,
    temp_path: &Path,
    total: u64,
    plan: &[Segment],
    pending: &[usize],
    already_done_bytes: u64,
    validators: Option<&Validators>,
    concurrency: usize,
    cancel: &CancelToken,
    pause: &PauseToken,
    on_segment_done: FS,
    on_progress: FP,
) -> Result<SegmentedOutcome, EngineError>
where
    FP: Fn(Progress),
    FS: Fn(usize),
{
    // Ensure the output file exists at the full size without truncating any
    // bytes already written by completed segments (resume-safe).
    {
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(temp_path)
            .await
            .map_err(map_io_error)?;
        file.set_len(total).await.map_err(map_io_error)?;
        file.sync_all().await.map_err(map_io_error)?;
    }

    let progress = Arc::new(AtomicU64::new(already_done_bytes));
    let if_range = validators.and_then(|v| v.if_range_value());
    let validators_cloned = validators.cloned().unwrap_or_default();

    on_progress(Progress {
        downloaded: already_done_bytes,
        total: Some(total),
    });

    if pending.is_empty() {
        return Ok(SegmentedOutcome::Completed);
    }

    let pending_segs: Vec<Segment> = pending
        .iter()
        .filter_map(|&i| plan.get(i).copied())
        .collect();
    let futures = pending_segs.into_iter().map(|seg| {
        run_resumable_segment(
            client,
            url,
            policy,
            &validators_cloned,
            if_range.as_deref(),
            temp_path,
            seg,
            total,
            cancel,
            pause,
            &progress,
            &on_progress,
            &on_segment_done,
        )
    });

    let statuses = stream::iter(futures)
        .buffer_unordered(concurrency.max(1))
        .try_collect::<Vec<SegmentStatus>>()
        .await?;

    if statuses.iter().any(|s| matches!(s, SegmentStatus::Paused)) {
        Ok(SegmentedOutcome::Paused)
    } else {
        Ok(SegmentedOutcome::Completed)
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_resumable_segment<FP, FS>(
    client: &reqwest::Client,
    url: &Url,
    policy: NetPolicy,
    expected_validators: &Validators,
    if_range: Option<&str>,
    temp_path: &Path,
    seg: Segment,
    total: u64,
    cancel: &CancelToken,
    pause: &PauseToken,
    progress: &AtomicU64,
    on_progress: &FP,
    on_segment_done: &FS,
) -> Result<SegmentStatus, EngineError>
where
    FP: Fn(Progress),
    FS: Fn(usize),
{
    if cancel.is_cancelled() {
        return Err(EngineError::new(
            ErrorCode::Cancelled,
            "The download was cancelled.",
        ));
    }
    if pause.is_paused() {
        return Ok(SegmentStatus::Paused);
    }

    let (_u, response) = send_get(client, url, policy, Some(&seg.range_header()), if_range).await?;
    let status = response.status();
    if status.as_u16() != 206 {
        if status.is_success() {
            return Err(EngineError::new(
                ErrorCode::ResourceChanged,
                "The server stopped honoring byte ranges mid-download.",
            ));
        }
        return Err(map_status(status.as_u16()));
    }

    let cr = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_content_range)
        .ok_or_else(|| {
            EngineError::new(
                ErrorCode::RangeInvalid,
                "The server returned no valid Content-Range.",
            )
        })?;
    validate_segment_content_range(&seg, &cr, total)?;

    let now = Validators {
        etag: response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(String::from),
        last_modified: response
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .map(String::from),
    };
    if !expected_validators.consistent_with(&now) {
        return Err(EngineError::new(
            ErrorCode::ResourceChanged,
            "The resource changed during the download.",
        ));
    }

    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(temp_path)
        .await
        .map_err(map_io_error)?;
    file.seek(SeekFrom::Start(seg.start))
        .await
        .map_err(map_io_error)?;

    let seg_len = seg.byte_len();
    let mut written: u64 = 0;
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        if cancel.is_cancelled() {
            return Err(EngineError::new(
                ErrorCode::Cancelled,
                "The download was cancelled.",
            ));
        }
        if pause.is_paused() {
            // Leave the segment un-acked so it is re-fetched on resume.
            let _ = file.flush().await;
            return Ok(SegmentStatus::Paused);
        }
        let bytes = chunk.map_err(|e| map_reqwest_error(url, e))?;
        written += bytes.len() as u64;
        if written > seg_len {
            return Err(EngineError::new(
                ErrorCode::ResourceChanged,
                "A segment returned more data than its range.",
            ));
        }
        file.write_all(&bytes).await.map_err(map_io_error)?;
        let done = progress.fetch_add(bytes.len() as u64, Ordering::SeqCst) + bytes.len() as u64;
        on_progress(Progress {
            downloaded: done,
            total: Some(total),
        });
    }

    if written != seg_len {
        return Err(EngineError::new(
            ErrorCode::NetworkError,
            "A segment ended before all its bytes were received.",
        ));
    }
    file.flush().await.map_err(map_io_error)?;
    file.sync_all().await.map_err(map_io_error)?;
    on_segment_done(seg.index);
    Ok(SegmentStatus::Done)
}

#[allow(clippy::too_many_arguments)]
async fn download_one_segment<F>(
    client: &reqwest::Client,
    final_url: &Url,
    policy: NetPolicy,
    expected_validators: &Validators,
    if_range: Option<&str>,
    temp_path: &Path,
    seg: &Segment,
    total: u64,
    cancel: &CancelToken,
    progress: &AtomicU64,
    on_progress: &F,
) -> Result<(), EngineError>
where
    F: Fn(Progress) + Sync,
{
    let (_url, response) = send_get(
        client,
        final_url,
        policy,
        Some(&seg.range_header()),
        if_range,
    )
    .await?;

    let status = response.status();
    // A range-capable resource must answer a ranged request with 206. A 200
    // means ranges stopped working or the resource changed (If-Range miss).
    if status.as_u16() != 206 {
        if status.is_success() {
            return Err(EngineError::new(
                ErrorCode::ResourceChanged,
                "The server stopped honoring byte ranges mid-download.",
            ));
        }
        return Err(map_status(status.as_u16()));
    }

    // Validate the returned range matches exactly what we requested.
    let cr = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_content_range)
        .ok_or_else(|| {
            EngineError::new(
                ErrorCode::RangeInvalid,
                "The server did not return a valid Content-Range.",
            )
        })?;
    validate_segment_content_range(seg, &cr, total)?;

    // Secondary guard: validators must remain consistent.
    let now = Validators {
        etag: response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(String::from),
        last_modified: response
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .map(String::from),
    };
    if !expected_validators.consistent_with(&now) {
        return Err(EngineError::new(
            ErrorCode::ResourceChanged,
            "The resource changed during the download.",
        ));
    }

    // Open an independent handle and seek to this segment's offset.
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(temp_path)
        .await
        .map_err(map_io_error)?;
    file.seek(SeekFrom::Start(seg.start))
        .await
        .map_err(map_io_error)?;

    let seg_len = seg.byte_len();
    let mut written: u64 = 0;
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        if cancel.is_cancelled() {
            return Err(EngineError::new(
                ErrorCode::Cancelled,
                "The download was cancelled.",
            ));
        }
        let bytes = chunk.map_err(|e| map_reqwest_error(final_url, e))?;
        written += bytes.len() as u64;
        if written > seg_len {
            return Err(EngineError::new(
                ErrorCode::ResourceChanged,
                "A segment returned more data than its range.",
            ));
        }
        file.write_all(&bytes).await.map_err(map_io_error)?;

        let done = progress.fetch_add(bytes.len() as u64, Ordering::SeqCst) + bytes.len() as u64;
        on_progress(Progress {
            downloaded: done,
            total: Some(total),
        });
    }

    if written != seg_len {
        return Err(EngineError::new(
            ErrorCode::NetworkError,
            "A segment ended before all its bytes were received.",
        ));
    }
    file.flush().await.map_err(map_io_error)?;
    file.sync_all().await.map_err(map_io_error)?;
    Ok(())
}
