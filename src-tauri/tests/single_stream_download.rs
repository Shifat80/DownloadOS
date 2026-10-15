//! Integration tests for the single-stream download engine.
//!
//! These use a controlled local `tiny_http` fixture server (per
//! `docs/TEST_PLAN.md`) and never touch a third-party site. Because the
//! server binds to loopback, the tests opt into `allow_loopback`; one test
//! confirms the *default* policy rejects loopback.

use std::io::{self, Cursor, Read};
use std::thread;
use std::time::Duration;

use downloados_lib::download_engine::single_stream::{self, DownloadRequest};
use downloados_lib::download_engine::{CancelToken, NetPolicy};
use downloados_lib::error::ErrorCode;

use tiny_http::{Header, Response, Server, StatusCode};

const HELLO: &[u8] = b"Hello, DownloadOS! 0123456789 payload bytes.";
const SLOW_TOTAL: usize = 4 * 1024 * 1024;

/// A reader that yields bytes slowly so a transfer can be cancelled mid-flight.
struct SlowReader {
    remaining: usize,
}

impl Read for SlowReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }
        thread::sleep(Duration::from_millis(20));
        let n = buf.len().min(self.remaining).min(64 * 1024);
        for b in &mut buf[..n] {
            *b = b'x';
        }
        self.remaining -= n;
        Ok(n)
    }
}

fn header(name: &[u8], value: &[u8]) -> Header {
    Header::from_bytes(name, value).unwrap()
}

/// Start the fixture server on an ephemeral loopback port; return its base URL.
fn start_server() -> String {
    let server = Server::http("127.0.0.1:0").expect("bind fixture server");
    let port = server.server_addr().to_ip().expect("ip listen addr").port();
    let base = format!("http://127.0.0.1:{port}");

    thread::spawn(move || {
        for request in server.incoming_requests() {
            let path = request.url().to_string();
            match path.as_str() {
                "/hello.bin" => {
                    let _ = request.respond(Response::from_data(HELLO.to_vec()));
                }
                "/named" => {
                    let resp = Response::from_data(HELLO.to_vec()).with_header(header(
                        b"Content-Disposition",
                        b"attachment; filename=\"renamed.dat\"",
                    ));
                    let _ = request.respond(resp);
                }
                "/notfound" => {
                    let _ = request.respond(Response::from_string("no").with_status_code(404));
                }
                "/denied" => {
                    let _ = request.respond(Response::from_string("no").with_status_code(401));
                }
                "/redirect" => {
                    let _ = request.respond(
                        Response::empty(302).with_header(header(b"Location", b"/hello.bin")),
                    );
                }
                "/redirect-bad-scheme" => {
                    let _ = request.respond(
                        Response::empty(302)
                            .with_header(header(b"Location", b"file:///etc/passwd")),
                    );
                }
                "/truncated" => {
                    // Declares 1000 bytes but sends only 5, then EOFs.
                    let resp = Response::new(
                        StatusCode(200),
                        vec![],
                        Cursor::new(b"short".to_vec()),
                        Some(1000),
                        None,
                    );
                    let _ = request.respond(resp);
                }
                "/slow" => {
                    let resp = Response::new(
                        StatusCode(200),
                        vec![],
                        SlowReader {
                            remaining: SLOW_TOTAL,
                        },
                        Some(SLOW_TOTAL),
                        None,
                    );
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

fn test_policy() -> NetPolicy {
    NetPolicy {
        allow_loopback: true,
    }
}

fn request(base: &str, path: &str, dir: &std::path::Path) -> DownloadRequest {
    DownloadRequest {
        url: format!("{base}{path}"),
        dest_dir: dir.to_path_buf(),
        filename: None,
    }
}

#[tokio::test]
async fn downloads_known_bytes_identically() {
    let base = start_server();
    let client = single_stream::build_client().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let req = request(&base, "/hello.bin", dir.path());

    let done = single_stream::download(&client, &req, test_policy(), &CancelToken::new(), |_| {})
        .await
        .unwrap();

    assert_eq!(done.bytes, HELLO.len() as u64);
    assert_eq!(done.path.file_name().unwrap(), "hello.bin");
    assert_eq!(std::fs::read(&done.path).unwrap(), HELLO);
}

#[tokio::test]
async fn progress_reports_actual_bytes() {
    let base = start_server();
    let client = single_stream::build_client().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let req = request(&base, "/hello.bin", dir.path());

    let mut last = 0u64;
    let mut total_seen = None;
    single_stream::download(&client, &req, test_policy(), &CancelToken::new(), |p| {
        last = p.downloaded;
        total_seen = p.total;
    })
    .await
    .unwrap();

    assert_eq!(last, HELLO.len() as u64);
    assert_eq!(total_seen, Some(HELLO.len() as u64));
}

#[tokio::test]
async fn uses_content_disposition_filename() {
    let base = start_server();
    let client = single_stream::build_client().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let req = request(&base, "/named", dir.path());

    let done = single_stream::download(&client, &req, test_policy(), &CancelToken::new(), |_| {})
        .await
        .unwrap();
    assert_eq!(done.path.file_name().unwrap(), "renamed.dat");
}

#[tokio::test]
async fn explicit_filename_overrides() {
    let base = start_server();
    let client = single_stream::build_client().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(&base, "/hello.bin", dir.path());
    req.filename = Some("custom name.bin".to_string());

    let done = single_stream::download(&client, &req, test_policy(), &CancelToken::new(), |_| {})
        .await
        .unwrap();
    assert_eq!(done.path.file_name().unwrap(), "custom name.bin");
}

#[tokio::test]
async fn maps_404_to_http_status() {
    let base = start_server();
    let client = single_stream::build_client().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let req = request(&base, "/notfound", dir.path());

    let err = single_stream::download(&client, &req, test_policy(), &CancelToken::new(), |_| {})
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::HttpStatus);
}

#[tokio::test]
async fn maps_401_to_auth_required() {
    let base = start_server();
    let client = single_stream::build_client().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let req = request(&base, "/denied", dir.path());

    let err = single_stream::download(&client, &req, test_policy(), &CancelToken::new(), |_| {})
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::AuthRequired);
}

#[tokio::test]
async fn does_not_overwrite_existing_destination() {
    let base = start_server();
    let client = single_stream::build_client().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let existing = dir.path().join("hello.bin");
    std::fs::write(&existing, b"ORIGINAL CONTENT").unwrap();

    let req = request(&base, "/hello.bin", dir.path());
    let err = single_stream::download(&client, &req, test_policy(), &CancelToken::new(), |_| {})
        .await
        .unwrap_err();

    assert_eq!(err.code, ErrorCode::DestinationExists);
    // The original file is untouched.
    assert_eq!(std::fs::read(&existing).unwrap(), b"ORIGINAL CONTENT");
}

#[tokio::test]
async fn cancellation_stops_and_cleans_up() {
    let base = start_server();
    let client = single_stream::build_client().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let req = request(&base, "/slow", dir.path());

    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    let err = single_stream::download(&client, &req, test_policy(), &cancel, move |p| {
        if p.downloaded > 0 {
            trigger.cancel();
        }
    })
    .await
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::Cancelled);

    // No partial (.part) or final file should remain in the directory.
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name())
        .collect();
    assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
}

#[tokio::test]
async fn truncated_response_fails_safely() {
    let base = start_server();
    let client = single_stream::build_client().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let req = request(&base, "/truncated", dir.path());

    let err = single_stream::download(&client, &req, test_policy(), &CancelToken::new(), |_| {})
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NetworkError);

    // Nothing is finalized.
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(leftovers.is_empty());
}

#[tokio::test]
async fn follows_valid_redirect() {
    let base = start_server();
    let client = single_stream::build_client().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let req = request(&base, "/redirect", dir.path());

    let done = single_stream::download(&client, &req, test_policy(), &CancelToken::new(), |_| {})
        .await
        .unwrap();
    assert_eq!(std::fs::read(&done.path).unwrap(), HELLO);
}

#[tokio::test]
async fn rejects_redirect_to_unsupported_scheme() {
    let base = start_server();
    let client = single_stream::build_client().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let req = request(&base, "/redirect-bad-scheme", dir.path());

    let err = single_stream::download(&client, &req, test_policy(), &CancelToken::new(), |_| {})
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::UnsupportedScheme);
}

#[tokio::test]
async fn default_policy_blocks_loopback() {
    let base = start_server();
    let client = single_stream::build_client().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let req = request(&base, "/hello.bin", dir.path());

    // Secure default must refuse a loopback target.
    let err = single_stream::download(
        &client,
        &req,
        NetPolicy::default(),
        &CancelToken::new(),
        |_| {},
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::PolicyRestricted);
}
