//! Byte-range support: Content-Range parsing, resource validators, range
//! probing, and segment planning.
//!
//! Range support is verified from a *real* ranged response (never from
//! `Accept-Ranges` alone), per `docs/SYSTEM_DESIGN.md` §7.

use url::Url;

use crate::download_engine::NetPolicy;
use crate::download_engine::net::{map_status, send_get};
use crate::error::{EngineError, ErrorCode};

/// A parsed `Content-Range: bytes START-END/TOTAL` value (END inclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentRange {
    pub start: u64,
    pub end: u64,
    pub total: Option<u64>,
}

/// Parse a `Content-Range` header value.
///
/// Accepts `bytes START-END/TOTAL` and `bytes START-END/*`. Returns `None` for
/// malformed values or an unsatisfied `bytes */TOTAL` form (no concrete range).
pub fn parse_content_range(value: &str) -> Option<ContentRange> {
    let rest = value.trim().strip_prefix("bytes ")?;
    let (range_part, total_part) = rest.split_once('/')?;

    let total = if total_part.trim() == "*" {
        None
    } else {
        Some(total_part.trim().parse::<u64>().ok()?)
    };

    // "bytes */TOTAL" has no concrete byte range.
    if range_part.trim() == "*" {
        return None;
    }

    let (s, e) = range_part.split_once('-')?;
    let start = s.trim().parse::<u64>().ok()?;
    let end = e.trim().parse::<u64>().ok()?;
    if end < start {
        return None;
    }
    if let Some(t) = total
        && (t == 0 || end >= t)
    {
        return None;
    }
    Some(ContentRange { start, end, total })
}

/// Resource consistency validators (ETag / Last-Modified).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Validators {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

impl Validators {
    /// The value to send in an `If-Range` header: prefer the (strong) ETag,
    /// else fall back to Last-Modified. `None` when no validator is available.
    pub fn if_range_value(&self) -> Option<String> {
        self.etag.clone().or_else(|| self.last_modified.clone())
    }

    /// Whether `later` is consistent with `self` (the resource has not changed
    /// in a way we can detect). Used as a secondary guard alongside `If-Range`.
    pub fn consistent_with(&self, later: &Validators) -> bool {
        if let (Some(a), Some(b)) = (&self.etag, &later.etag) {
            return a == b;
        }
        if let (Some(a), Some(b)) = (&self.last_modified, &later.last_modified) {
            return a == b;
        }
        // If we cannot compare (a validator is missing on one side), do not
        // claim a mismatch here; `If-Range` + the 206/200 status is the
        // authoritative check.
        true
    }

    fn from_headers(headers: &reqwest::header::HeaderMap) -> Self {
        let get = |name: reqwest::header::HeaderName| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        };
        Validators {
            etag: get(reqwest::header::ETAG),
            last_modified: get(reqwest::header::LAST_MODIFIED),
        }
    }
}

/// What a range probe learned about a resource.
#[derive(Debug, Clone)]
pub struct ResourceInfo {
    /// Total size in bytes, when the server reports it trustworthily.
    pub total: Option<u64>,
    /// Whether the server honored a real ranged request with `206`.
    pub accepts_ranges: bool,
    pub validators: Validators,
    /// Filename suggested by `Content-Disposition`, if any (sanitized).
    pub suggested_filename: Option<String>,
}

/// Probe a resource with a tiny ranged GET (`bytes=0-0`) to learn its size,
/// whether ranges are honored, and its validators. The response body is
/// discarded without downloading it.
pub async fn probe(
    client: &reqwest::Client,
    url: &Url,
    policy: NetPolicy,
) -> Result<(Url, ResourceInfo), EngineError> {
    let (final_url, resp) = send_get(client, url, policy, Some("bytes=0-0"), None).await?;
    let status = resp.status();
    let validators = Validators::from_headers(resp.headers());
    let suggested_filename = resp
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(crate::security::filename::filename_from_content_disposition);

    if status.as_u16() == 206 {
        let total = resp
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_range)
            .and_then(|cr| cr.total);
        Ok((
            final_url,
            ResourceInfo {
                total,
                accepts_ranges: true,
                validators,
                suggested_filename,
            },
        ))
    } else if status.is_success() {
        // Server ignored the range and returned the whole resource: no range
        // support. Do not read the body.
        Ok((
            final_url,
            ResourceInfo {
                total: resp.content_length(),
                accepts_ranges: false,
                validators,
                suggested_filename,
            },
        ))
    } else {
        Err(map_status(status.as_u16()))
    }
}

/// A planned contiguous byte segment (END inclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub index: usize,
    pub start: u64,
    pub end: u64,
}

impl Segment {
    /// Number of bytes in the segment (END inclusive). Always at least 1.
    pub fn byte_len(&self) -> u64 {
        self.end - self.start + 1
    }

    /// The `Range` request header value for this segment.
    pub fn range_header(&self) -> String {
        format!("bytes={}-{}", self.start, self.end)
    }
}

/// Split `total` bytes into at most `max_segments` contiguous, non-overlapping
/// segments, each at least `min_segment_len` bytes (except possibly the last).
///
/// Returns an empty plan for a zero-length resource.
pub fn plan_segments(total: u64, max_segments: usize, min_segment_len: u64) -> Vec<Segment> {
    if total == 0 {
        return Vec::new();
    }
    let max_segments = max_segments.max(1) as u64;
    let min_len = min_segment_len.max(1);

    // Number of segments is bounded by both the cap and the minimum size.
    let by_size = total.div_ceil(min_len).max(1);
    let count = by_size.min(max_segments);

    let base = total / count;
    let remainder = total % count;

    let mut segments = Vec::with_capacity(count as usize);
    let mut start = 0u64;
    for i in 0..count {
        // Distribute the remainder across the first `remainder` segments.
        let len = base + if i < remainder { 1 } else { 0 };
        let end = start + len - 1;
        segments.push(Segment {
            index: i as usize,
            start,
            end,
        });
        start = end + 1;
    }
    segments
}

/// Validate that a server's `Content-Range` for a segment exactly matches what
/// we requested and the expected total.
pub fn validate_segment_content_range(
    requested: &Segment,
    cr: &ContentRange,
    expected_total: u64,
) -> Result<(), EngineError> {
    if cr.start != requested.start || cr.end != requested.end {
        return Err(EngineError::new(
            ErrorCode::RangeInvalid,
            "The server returned a byte range that does not match the request.",
        ));
    }
    if let Some(t) = cr.total
        && t != expected_total
    {
        return Err(EngineError::new(
            ErrorCode::ResourceChanged,
            "The resource's total size changed during the download.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_content_ranges() {
        assert_eq!(
            parse_content_range("bytes 0-1023/2048"),
            Some(ContentRange {
                start: 0,
                end: 1023,
                total: Some(2048)
            })
        );
        assert_eq!(
            parse_content_range("bytes 100-199/*"),
            Some(ContentRange {
                start: 100,
                end: 199,
                total: None
            })
        );
    }

    #[test]
    fn reject_malformed_content_ranges() {
        for s in [
            "0-1023/2048",        // missing "bytes "
            "bytes */2048",       // unsatisfied, no concrete range
            "bytes 500-100/2048", // end < start
            "bytes 0-2048/2048",  // end >= total
            "bytes abc/2048",
            "bytes 0-10/xyz",
            "garbage",
        ] {
            assert!(parse_content_range(s).is_none(), "should reject {s:?}");
        }
    }

    #[test]
    fn plan_segments_cover_exactly_and_do_not_overlap() {
        let total = 10_000u64;
        let segs = plan_segments(total, 4, 1024);
        assert_eq!(segs.len(), 4);
        assert_eq!(segs[0].start, 0);
        assert_eq!(segs.last().unwrap().end, total - 1);

        let mut expected_start = 0u64;
        let mut sum = 0u64;
        for s in &segs {
            assert_eq!(s.start, expected_start, "segments must be contiguous");
            assert!(s.end >= s.start);
            sum += s.byte_len();
            expected_start = s.end + 1;
        }
        assert_eq!(sum, total, "segments must cover exactly the total");
    }

    #[test]
    fn plan_segments_respects_minimum_size() {
        // Small file with a large minimum => a single segment.
        let segs = plan_segments(1000, 8, 4096);
        assert_eq!(segs.len(), 1);
        assert_eq!(
            segs[0],
            Segment {
                index: 0,
                start: 0,
                end: 999
            }
        );
    }

    #[test]
    fn plan_segments_zero_length_is_empty() {
        assert!(plan_segments(0, 4, 1024).is_empty());
    }

    #[test]
    fn segment_content_range_validation() {
        let seg = Segment {
            index: 0,
            start: 0,
            end: 99,
        };
        assert!(
            validate_segment_content_range(
                &seg,
                &ContentRange {
                    start: 0,
                    end: 99,
                    total: Some(1000)
                },
                1000
            )
            .is_ok()
        );
        // Mismatched range.
        assert_eq!(
            validate_segment_content_range(
                &seg,
                &ContentRange {
                    start: 0,
                    end: 50,
                    total: Some(1000)
                },
                1000
            )
            .unwrap_err()
            .code,
            ErrorCode::RangeInvalid
        );
        // Changed total.
        assert_eq!(
            validate_segment_content_range(
                &seg,
                &ContentRange {
                    start: 0,
                    end: 99,
                    total: Some(2000)
                },
                1000
            )
            .unwrap_err()
            .code,
            ErrorCode::ResourceChanged
        );
    }

    #[test]
    fn if_range_prefers_etag() {
        let v = Validators {
            etag: Some("\"abc\"".into()),
            last_modified: Some("Mon, 01 Jan 2024 00:00:00 GMT".into()),
        };
        assert_eq!(v.if_range_value().as_deref(), Some("\"abc\""));
    }

    #[test]
    fn validators_consistency() {
        let a = Validators {
            etag: Some("\"v1\"".into()),
            last_modified: None,
        };
        let b = Validators {
            etag: Some("\"v1\"".into()),
            last_modified: None,
        };
        let c = Validators {
            etag: Some("\"v2\"".into()),
            last_modified: None,
        };
        assert!(a.consistent_with(&b));
        assert!(!a.consistent_with(&c));
        // Missing validator on one side is not treated as a mismatch here.
        let none = Validators::default();
        assert!(a.consistent_with(&none));
    }
}
