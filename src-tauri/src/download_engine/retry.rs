//! Bounded retries with exponential backoff.
//!
//! Only transient failures are retried; permanent failures (auth, policy,
//! invalid URL, disk, changed resource, cancellation, bad range, and definite
//! HTTP statuses) return immediately so a download never loops forever
//! (`docs/SYSTEM_DESIGN.md` §6, `docs/SECURITY_AND_POLICY.md`).

use std::future::Future;
use std::time::Duration;

use crate::download_engine::CancelToken;
use crate::error::{EngineError, ErrorCode};

/// Default retry policy knobs.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 4;
pub const DEFAULT_BASE_DELAY: Duration = Duration::from_millis(500);
pub const DEFAULT_MAX_DELAY: Duration = Duration::from_secs(10);

/// Whether an error is worth retrying.
///
/// Conservatively retries only transient network errors. HTTP status errors
/// are treated as permanent here because the engine collapses them to a single
/// code; status-aware retry of 429/503 is a Phase 7 refinement.
pub fn is_retryable(code: ErrorCode) -> bool {
    matches!(code, ErrorCode::NetworkError)
}

/// Exponential backoff for a 1-based attempt number, capped at `max`.
pub fn backoff_delay(attempt: u32, base: Duration, max: Duration) -> Duration {
    if attempt <= 1 {
        return base.min(max);
    }
    // base * 2^(attempt-1), saturating, capped at max.
    let shift = (attempt - 1).min(16);
    let factor = 1u64 << shift;
    let scaled = base
        .as_millis()
        .saturating_mul(factor as u128)
        .min(max.as_millis());
    Duration::from_millis(scaled as u64)
}

/// Run `op`, retrying transient failures up to `max_attempts` with backoff.
///
/// Stops early (returning the last error) on a permanent error, when attempts
/// are exhausted, or when `cancel` is tripped.
pub async fn with_retries<F, Fut, T>(
    max_attempts: u32,
    base: Duration,
    max: Duration,
    cancel: &CancelToken,
    mut op: F,
) -> Result<T, EngineError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, EngineError>>,
{
    let mut attempt = 1u32;
    loop {
        match op().await {
            Ok(value) => return Ok(value),
            Err(err) => {
                if !is_retryable(err.code) || attempt >= max_attempts || cancel.is_cancelled() {
                    return Err(err);
                }
                tokio::time::sleep(backoff_delay(attempt, base, max)).await;
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_network_errors_retry() {
        assert!(is_retryable(ErrorCode::NetworkError));
        for code in [
            ErrorCode::AuthRequired,
            ErrorCode::UnsupportedScheme,
            ErrorCode::PolicyRestricted,
            ErrorCode::HttpStatus,
            ErrorCode::ResourceChanged,
            ErrorCode::RangeInvalid,
            ErrorCode::DiskFull,
            ErrorCode::Cancelled,
            ErrorCode::DestinationExists,
        ] {
            assert!(!is_retryable(code), "{code:?} must not retry");
        }
    }

    #[test]
    fn backoff_grows_and_caps() {
        let base = Duration::from_millis(500);
        let max = Duration::from_secs(10);
        assert_eq!(backoff_delay(1, base, max), Duration::from_millis(500));
        assert_eq!(backoff_delay(2, base, max), Duration::from_millis(1000));
        assert_eq!(backoff_delay(3, base, max), Duration::from_millis(2000));
        assert_eq!(backoff_delay(4, base, max), Duration::from_millis(4000));
        // Caps at max.
        assert_eq!(backoff_delay(10, base, max), max);
    }
}
