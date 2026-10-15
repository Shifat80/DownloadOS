//! Integration tests for Phase 3: range probing, segmented transfers,
//! fallback, changed-resource detection, and pause/resume.
//!
//! Uses a controlled local `tiny_http` fixture that implements byte ranges.
//! The fixture binds to loopback, so tests opt into `allow_loopback`.

use std::io::{self, Read};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use downloados_lib::download_engine::range::{plan_segments, probe};
use downloados_lib::download_engine::segmented::{
    SegmentPlanConfig, SegmentedOutcome, download_segmented, download_segments,
};
use downloados_lib::download_engine::single_stream::{self, StopState};
use downloados_lib::download_engine::{CancelToken, NetPolicy, PauseToken};
use downloados_lib::error::ErrorCode;

use tiny_http::{Header, Response, Server, StatusCode};

/// Deterministic test payload of `n` bytes.
fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

fn header(name: &[u8], value: &[u8]) -> Header {
    Header::from_bytes(name, value).unwrap()
}

fn header_value(req: &tiny_http::Request, name: &'static str) -> Option<String> {
    req.headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
}

/// Parse a request `Range: bytes=START-[END]` header into inclusive bounds.
fn parse_req_range(h: &str, total: u64) -> Option<(u64, u64)> {
    let r = h.trim().strip_prefix("bytes=")?;
    let (s, e) = r.split_once('-')?;
    let start = s.trim().parse::<u64>().ok()?;
    let end = if e.trim().is_empty() {
        total - 1
    } else {
        e.trim().parse::<u64>().ok()?
    };
    if start > end || end >= total {
        return None;
    }
    Some((start, end))
}

/// A reader that yields bytes slowly so a transfer can be paused mid-flight.
struct SlowReader {
    data: Vec<u8>,
    pos: usize,
}

impl Read for SlowReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.data.len() {
            return Ok(0);
        }
        thread::sleep(Duration::from_millis(15));
        let n = buf.len().min(self.data.len() - self.pos).min(16 * 1024);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

const ETAG: &str = "\"v1\"";

/// Start the fixture server; returns its base URL.
fn start_server(body: Vec<u8>) -> String {
    let server = Server::http("127.0.0.1:0").expect("bind fixture server");
    let port = server.server_addr().to_ip().expect("ip addr").port();
    let base = format!("http://127.0.0.1:{port}");
    let total = body.len() as u64;
    let changing_counter = AtomicU64::new(0);
    let body = Mutex::new(body);

    thread::spawn(move || {
        for request in server.incoming_requests() {
            let path = request.url().to_string();
            let range = header_value(&request, "Range");
            let if_range = header_value(&request, "If-Range");
            let data = body.lock().unwrap().clone();

            match path.as_str() {
                // Honors ranges with 206; serves full body with 200 otherwise.
                "/ranged" | "/resume" => {
                    let slow = path == "/resume";
                    respond_ranged(request, &data, total, range, if_range, ETAG, slow);
                }
                // Always ignores Range and returns the whole body (fallback).
                "/norange" => {
                    let _ = request.respond(Response::from_data(data));
                }
                // Rotates its ETag every request to simulate a changing
                // resource (probe sees one validator, segments see another).
                "/changing" => {
                    let n = changing_counter.fetch_add(1, Ordering::SeqCst);
                    let etag = format!("\"v{n}\"");
                    respond_ranged(request, &data, total, range, if_range, &etag, false);
                }
                // Returns a 206 whose Content-Range never matches the request.
                "/badrange" => {
                    let resp = Response::from_data(vec![data[0]])
                        .with_status_code(206)
                        .with_header(header(
                            b"Content-Range",
                            format!("bytes 0-0/{total}").as_bytes(),
                        ));
                    let _ = request.respond(resp);
                }
                _ => {
                    let _ = request.respond(Response::from_string("nf").with_status_code(404));
                }
            }
        }
    });

    base
}

fn respond_ranged(
    request: tiny_http::Request,
    data: &[u8],
    total: u64,
    range: Option<String>,
    if_range: Option<String>,
    etag: &str,
    slow: bool,
) {
    // If-Range mismatch => serve full 200 (forces a restart on the client).
    let honor = range.is_some() && if_range.as_deref().map(|v| v == etag).unwrap_or(true);

    if let Some(r) = range.filter(|_| honor) {
        if let Some((start, end)) = parse_req_range(&r, total) {
            let slice = data[start as usize..=end as usize].to_vec();
            let resp = Response::from_data(slice)
                .with_status_code(206)
                .with_header(header(
                    b"Content-Range",
                    format!("bytes {start}-{end}/{total}").as_bytes(),
                ))
                .with_header(header(b"ETag", etag.as_bytes()))
                .with_header(header(b"Accept-Ranges", b"bytes"));
            let _ = request.respond(resp);
            return;
        }
        let _ = request.respond(Response::from_string("bad range").with_status_code(416));
        return;
    }

    // Full body (200).
    let etag_header = header(b"ETag", etag.as_bytes());
    if slow {
        let reader = SlowReader {
            data: data.to_vec(),
            pos: 0,
        };
        let resp = Response::new(
            StatusCode(200),
            vec![etag_header, header(b"Accept-Ranges", b"bytes")],
            reader,
            Some(data.len()),
            None,
        );
        let _ = request.respond(resp);
    } else {
        let resp = Response::from_data(data.to_vec())
            .with_header(etag_header)
            .with_header(header(b"Accept-Ranges", b"bytes"));
        let _ = request.respond(resp);
    }
}

fn test_policy() -> NetPolicy {
    NetPolicy {
        allow_loopback: true,
    }
}

fn small_segment_config() -> SegmentPlanConfig {
    // Force several segments even for a small test payload.
    SegmentPlanConfig {
        max_segments: 4,
        min_segment_len: 256,
        concurrency: 4,
    }
}

#[tokio::test]
async fn probe_reports_range_support_and_size() {
    let body = pattern(4096);
    let base = start_server(body.clone());
    let client = single_stream::build_client().unwrap();
    let url = url::Url::parse(&format!("{base}/ranged")).unwrap();

    let (_final, info) = probe(&client, &url, test_policy()).await.unwrap();
    assert!(info.accepts_ranges);
    assert_eq!(info.total, Some(4096));
}

#[tokio::test]
async fn probe_detects_no_range_support() {
    let body = pattern(4096);
    let base = start_server(body);
    let client = single_stream::build_client().unwrap();
    let url = url::Url::parse(&format!("{base}/norange")).unwrap();

    let (_final, info) = probe(&client, &url, test_policy()).await.unwrap();
    assert!(!info.accepts_ranges);
}

#[tokio::test]
async fn segmented_download_is_byte_identical() {
    let body = pattern(4096);
    let base = start_server(body.clone());
    let client = single_stream::build_client().unwrap();
    let url = url::Url::parse(&format!("{base}/ranged")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");

    let (final_url, info) = probe(&client, &url, test_policy()).await.unwrap();
    let done = download_segmented(
        &client,
        &final_url,
        test_policy(),
        &info,
        &dest,
        small_segment_config(),
        &CancelToken::new(),
        |_p| {},
    )
    .await
    .unwrap();

    assert_eq!(done.bytes, 4096);
    assert_eq!(std::fs::read(&done.path).unwrap(), body);
}

#[tokio::test]
async fn segmented_detects_changed_resource() {
    let body = pattern(4096);
    let base = start_server(body);
    let client = single_stream::build_client().unwrap();
    let url = url::Url::parse(&format!("{base}/changing")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");

    let (final_url, info) = probe(&client, &url, test_policy()).await.unwrap();
    let err = download_segmented(
        &client,
        &final_url,
        test_policy(),
        &info,
        &dest,
        small_segment_config(),
        &CancelToken::new(),
        |_p| {},
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::ResourceChanged);
    assert!(!dest.exists(), "no file should be finalized");
}

#[tokio::test]
async fn segmented_rejects_bad_content_range() {
    let body = pattern(4096);
    let base = start_server(body);
    let client = single_stream::build_client().unwrap();
    let url = url::Url::parse(&format!("{base}/ranged")).unwrap();
    // Probe the honest endpoint for a valid ResourceInfo, then point the
    // segmented download at the lying endpoint.
    let (_final, info) = probe(&client, &url, test_policy()).await.unwrap();
    let bad_url = url::Url::parse(&format!("{base}/badrange")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");

    let err = download_segmented(
        &client,
        &bad_url,
        test_policy(),
        &info,
        &dest,
        small_segment_config(),
        &CancelToken::new(),
        |_p| {},
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::RangeInvalid);
}

#[tokio::test]
async fn segmented_resume_fills_remaining_segments() {
    // Deterministic resume: download some segments, then the rest, and verify
    // the assembled file is byte-identical with correct offsets.
    let body = pattern(4096);
    let base = start_server(body.clone());
    let client = single_stream::build_client().unwrap();
    let url = url::Url::parse(&format!("{base}/ranged")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let temp = dir.path().join("out.part");

    let plan = plan_segments(4096, 4, 256);
    assert!(plan.len() >= 2);

    // First pass: fetch only the first half of the segments.
    let half = plan.len() / 2;
    let first: Vec<usize> = (0..half).collect();
    let out1 = download_segments(
        &client,
        &url,
        test_policy(),
        &temp,
        4096,
        &plan,
        &first,
        0,
        None,
        4,
        &CancelToken::new(),
        &PauseToken::new(),
        |_idx| {},
        |_p| {},
    )
    .await
    .unwrap();
    assert_eq!(out1, SegmentedOutcome::Completed);

    // Second pass: fetch the remaining segments, with `already` bytes accounted.
    let rest: Vec<usize> = (half..plan.len()).collect();
    let already: u64 = plan[..half].iter().map(|s| s.byte_len()).sum();
    let out2 = download_segments(
        &client,
        &url,
        test_policy(),
        &temp,
        4096,
        &plan,
        &rest,
        already,
        None,
        4,
        &CancelToken::new(),
        &PauseToken::new(),
        |_idx| {},
        |_p| {},
    )
    .await
    .unwrap();
    assert_eq!(out2, SegmentedOutcome::Completed);

    assert_eq!(std::fs::read(&temp).unwrap(), body);
}

#[tokio::test]
async fn resume_completes_after_pause() {
    let body = pattern(200_000);
    let base = start_server(body.clone());
    let client = single_stream::build_client().unwrap();
    let url = url::Url::parse(&format!("{base}/resume")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("resumed.bin");

    // First attempt: pause once some bytes have arrived.
    let pause = PauseToken::new();
    let trigger = pause.clone();
    let first = single_stream::download_resumable(
        &client,
        &url,
        test_policy(),
        &dest,
        None,
        &CancelToken::new(),
        &pause,
        move |p| {
            if p.downloaded > 0 {
                trigger.pause();
            }
        },
    )
    .await
    .unwrap();

    let paused_at = match first {
        StopState::Paused { downloaded } => downloaded,
        StopState::Completed(_) => 0, // Completed too fast; resume is a no-op path below.
    };
    assert!(!dest.exists(), "paused transfer must not be finalized yet");

    // Second attempt: resume to completion.
    let done = single_stream::download_resumable(
        &client,
        &url,
        test_policy(),
        &dest,
        None,
        &CancelToken::new(),
        &PauseToken::new(),
        |_p| {},
    )
    .await
    .unwrap();

    match done {
        StopState::Completed(c) => {
            assert_eq!(c.bytes, body.len() as u64);
            assert_eq!(std::fs::read(&c.path).unwrap(), body);
            assert!(paused_at <= body.len() as u64);
        }
        StopState::Paused { .. } => panic!("second attempt should complete"),
    }
}
