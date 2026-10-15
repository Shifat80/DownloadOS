//! Integration tests for the loopback browser-handoff bridge.

use std::sync::{Arc, Mutex};

use downloados_lib::bridge::{BridgeConfig, EnqueueFn, HandoffAck, HandoffRequest, spawn};
use downloados_lib::download_engine::single_stream::build_client;

const TOKEN: &str = "pair-secret";
const ORIGIN: &str = "chrome-extension://abcdefghijklmnop";

/// Start a bridge whose enqueue records requests; returns (base_url, calls).
fn start() -> (String, Arc<Mutex<Vec<HandoffRequest>>>) {
    let calls: Arc<Mutex<Vec<HandoffRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = calls.clone();
    let enqueue: EnqueueFn = Arc::new(move |req: HandoffRequest| {
        recorder.lock().unwrap().push(req);
        Ok(HandoffAck {
            id: "task-1".into(),
            filename: "file.bin".into(),
        })
    });
    let handle = spawn(
        BridgeConfig {
            port: 0,
            token: TOKEN.into(),
        },
        enqueue,
    )
    .expect("bridge binds");
    (format!("http://127.0.0.1:{}", handle.port), calls)
}

async fn post(base: &str, origin: Option<&str>, token: Option<&str>, body: &str) -> u16 {
    let client = build_client().unwrap();
    let mut req = client
        .post(format!("{base}/handoff"))
        .header("Content-Type", "application/json")
        .body(body.to_string());
    if let Some(o) = origin {
        req = req.header("Origin", o);
    }
    if let Some(t) = token {
        req = req.header("X-DownloadOS-Token", t);
    }
    req.send().await.unwrap().status().as_u16()
}

#[tokio::test]
async fn ping_is_unauthenticated_ok() {
    let (base, _calls) = start();
    let client = build_client().unwrap();
    let res = client.get(format!("{base}/ping")).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 200);
}

#[tokio::test]
async fn valid_handoff_is_accepted_and_enqueued() {
    let (base, calls) = start();
    let status = post(
        &base,
        Some(ORIGIN),
        Some(TOKEN),
        r#"{"url":"https://example.com/file.bin"}"#,
    )
    .await;
    assert_eq!(status, 200);
    let recorded = calls.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].url, "https://example.com/file.bin");
}

#[tokio::test]
async fn rejects_bad_token() {
    let (base, calls) = start();
    let status = post(
        &base,
        Some(ORIGIN),
        Some("wrong"),
        r#"{"url":"https://example.com/f"}"#,
    )
    .await;
    assert_eq!(status, 401);
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn rejects_missing_token() {
    let (base, calls) = start();
    let status = post(
        &base,
        Some(ORIGIN),
        None,
        r#"{"url":"https://example.com/f"}"#,
    )
    .await;
    assert_eq!(status, 401);
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn rejects_web_origin() {
    let (base, calls) = start();
    let status = post(
        &base,
        Some("https://evil.example.com"),
        Some(TOKEN),
        r#"{"url":"https://example.com/f"}"#,
    )
    .await;
    assert_eq!(status, 403);
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn rejects_oversized_body() {
    let (base, calls) = start();
    let big = format!(r#"{{"url":"https://example.com/{}"}}"#, "a".repeat(9000));
    let status = post(&base, Some(ORIGIN), Some(TOKEN), &big).await;
    assert_eq!(status, 413);
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn rejects_malformed_json() {
    let (base, calls) = start();
    let status = post(&base, Some(ORIGIN), Some(TOKEN), "not json").await;
    assert_eq!(status, 400);
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn rejects_non_http_url() {
    let (base, calls) = start();
    let status = post(
        &base,
        Some(ORIGIN),
        Some(TOKEN),
        r#"{"url":"ftp://example.com/x"}"#,
    )
    .await;
    assert_eq!(status, 400);
    assert!(calls.lock().unwrap().is_empty());
}
