//! Hardened loopback bridge for browser handoff.
//!
//! Binds to `127.0.0.1` only. Every `/handoff` request must present the
//! pairing token and originate from an extension origin; bodies are size- and
//! rate-limited and schema-validated, and only http/https URLs are accepted.
//! See `docs/BRIDGE_SECURITY.md` and `docs/SECURITY_AND_POLICY.md` §6.

use std::collections::VecDeque;
use std::io::{Cursor, Read};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tiny_http::{Header, Request, Response, Server};

use crate::security::url::validate_download_url;

const MAX_BODY_BYTES: usize = 8 * 1024; // 8 KiB is plenty for a URL + filename.
const RATE_WINDOW: Duration = Duration::from_secs(10);
const RATE_MAX: usize = 20; // handoffs per window.
const ALLOWED_ORIGIN_PREFIXES: [&str; 2] = ["chrome-extension://", "moz-extension://"];

/// Configuration for the bridge server.
#[derive(Clone)]
pub struct BridgeConfig {
    /// Loopback port to bind. `0` binds an ephemeral port (used by tests).
    pub port: u16,
    /// Pairing token the extension must present.
    pub token: String,
}

/// A validated handoff request body.
#[derive(Debug, Clone, Deserialize)]
pub struct HandoffRequest {
    pub url: String,
    #[serde(default)]
    pub filename: Option<String>,
}

/// Success acknowledgement returned to the extension.
#[derive(Debug, Clone)]
pub struct HandoffAck {
    pub id: String,
    pub filename: String,
}

/// A rejected request, mapped to an HTTP status.
#[derive(Debug, Clone)]
pub struct HandoffDenied {
    pub status: u16,
    pub code: String,
    pub message: String,
}

impl HandoffDenied {
    pub fn new(status: u16, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
        }
    }
}

/// Callback invoked for a valid handoff; returns the created task or a denial.
pub type EnqueueFn = Arc<dyn Fn(HandoffRequest) -> Result<HandoffAck, HandoffDenied> + Send + Sync>;

/// A running bridge.
pub struct BridgeHandle {
    pub port: u16,
}

/// Bind the loopback bridge and run its request loop on a background thread.
pub fn spawn(config: BridgeConfig, enqueue: EnqueueFn) -> std::io::Result<BridgeHandle> {
    let server = Server::http(("127.0.0.1", config.port))
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .unwrap_or(config.port);

    let token = config.token.clone();
    std::thread::spawn(move || {
        let limiter = RateLimiter::new(RATE_WINDOW, RATE_MAX);
        for request in server.incoming_requests() {
            handle_request(request, &token, &limiter, &enqueue);
        }
    });

    Ok(BridgeHandle { port })
}

fn handle_request(mut request: Request, token: &str, limiter: &RateLimiter, enqueue: &EnqueueFn) {
    let method = request.method().as_str().to_uppercase();
    let path = request.url().split('?').next().unwrap_or("").to_string();

    // Availability probe: unauthenticated, reveals only that the app is here.
    if method == "GET" && path == "/ping" {
        let body = serde_json::json!({ "app": "DownloadOS", "version": env!("CARGO_PKG_VERSION") });
        let _ = request.respond(json_response(200, &body));
        return;
    }

    if method != "POST" || path != "/handoff" {
        let _ = request.respond(json_response(
            404,
            &denied_body("NOT_FOUND", "Unknown endpoint."),
        ));
        return;
    }

    let origin = header_value(&request, "Origin");
    let token_hdr = header_value(&request, "X-DownloadOS-Token");

    if let Err(d) = authorize(origin.as_deref(), token_hdr.as_deref(), token) {
        let _ = request.respond(json_response(d.status, &denied_body(&d.code, &d.message)));
        return;
    }

    if !limiter.allow() {
        let _ = request.respond(json_response(
            429,
            &denied_body("RATE_LIMITED", "Too many requests; slow down."),
        ));
        return;
    }

    // Enforce the body-size limit using the declared length and a capped read.
    if let Some(len) = request.body_length()
        && len > MAX_BODY_BYTES
    {
        let _ = request.respond(json_response(
            413,
            &denied_body("TOO_LARGE", "Request body is too large."),
        ));
        return;
    }

    let mut buf = Vec::new();
    if request
        .as_reader()
        .take((MAX_BODY_BYTES + 1) as u64)
        .read_to_end(&mut buf)
        .is_err()
        || buf.len() > MAX_BODY_BYTES
    {
        let _ = request.respond(json_response(
            413,
            &denied_body("TOO_LARGE", "Request body is too large."),
        ));
        return;
    }

    let parsed: HandoffRequest = match serde_json::from_slice(&buf) {
        Ok(p) => p,
        Err(_) => {
            let _ = request.respond(json_response(
                400,
                &denied_body("BAD_JSON", "The request body was not valid JSON."),
            ));
            return;
        }
    };

    // Only accept http/https URLs (full SSRF checks run at download time).
    if validate_download_url(&parsed.url).is_err() {
        let _ = request.respond(json_response(
            400,
            &denied_body("BAD_URL", "Only http(s) URLs can be sent."),
        ));
        return;
    }

    match enqueue(parsed) {
        Ok(ack) => {
            let body = serde_json::json!({ "id": ack.id, "filename": ack.filename });
            let _ = request.respond(json_response(200, &body));
        }
        Err(d) => {
            let _ = request.respond(json_response(d.status, &denied_body(&d.code, &d.message)));
        }
    }
}

/// Authorize a handoff by origin and token. Pure and unit-tested.
pub fn authorize(
    origin: Option<&str>,
    token_header: Option<&str>,
    expected_token: &str,
) -> Result<(), HandoffDenied> {
    let origin = origin
        .ok_or_else(|| HandoffDenied::new(403, "BAD_ORIGIN", "Missing Origin header."))?;
    if !ALLOWED_ORIGIN_PREFIXES
        .iter()
        .any(|p| origin.starts_with(p))
    {
        return Err(HandoffDenied::new(
            403,
            "BAD_ORIGIN",
            "Requests must come from the browser extension.",
        ));
    }

    let provided = token_header
        .ok_or_else(|| HandoffDenied::new(401, "NO_TOKEN", "Missing pairing token."))?;
    if expected_token.is_empty() || !ct_eq(provided.as_bytes(), expected_token.as_bytes()) {
        return Err(HandoffDenied::new(
            401,
            "BAD_TOKEN",
            "The pairing token is incorrect.",
        ));
    }
    Ok(())
}

/// Constant-time byte comparison (avoids leaking the token via timing).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn header_value(request: &Request, name: &'static str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
}

fn denied_body(code: &str, message: &str) -> serde_json::Value {
    serde_json::json!({ "code": code, "message": message })
}

fn json_response(status: u16, body: &serde_json::Value) -> Response<Cursor<Vec<u8>>> {
    let data = serde_json::to_vec(body).unwrap_or_default();
    Response::from_data(data).with_status_code(status).with_header(
        Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("valid header"),
    )
}

/// A simple fixed-window rate limiter (single-threaded request loop).
struct RateLimiter {
    window: Duration,
    max: usize,
    hits: Mutex<VecDeque<Instant>>,
}

impl RateLimiter {
    fn new(window: Duration, max: usize) -> Self {
        Self {
            window,
            max,
            hits: Mutex::new(VecDeque::new()),
        }
    }

    fn allow(&self) -> bool {
        let now = Instant::now();
        let mut hits = self.hits.lock().expect("rate limiter mutex");
        while let Some(&front) = hits.front() {
            if now.duration_since(front) > self.window {
                hits.pop_front();
            } else {
                break;
            }
        }
        if hits.len() >= self.max {
            return false;
        }
        hits.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_accepts_extension_origin_and_token() {
        assert!(
            authorize(
                Some("chrome-extension://abcdef"),
                Some("secret"),
                "secret"
            )
            .is_ok()
        );
    }

    #[test]
    fn authorize_rejects_web_origin() {
        let d = authorize(Some("https://evil.com"), Some("secret"), "secret").unwrap_err();
        assert_eq!(d.status, 403);
        assert_eq!(d.code, "BAD_ORIGIN");
    }

    #[test]
    fn authorize_rejects_missing_origin() {
        let d = authorize(None, Some("secret"), "secret").unwrap_err();
        assert_eq!(d.status, 403);
    }

    #[test]
    fn authorize_rejects_bad_and_missing_token() {
        assert_eq!(
            authorize(Some("chrome-extension://x"), Some("wrong"), "secret")
                .unwrap_err()
                .code,
            "BAD_TOKEN"
        );
        assert_eq!(
            authorize(Some("chrome-extension://x"), None, "secret")
                .unwrap_err()
                .code,
            "NO_TOKEN"
        );
    }

    #[test]
    fn authorize_rejects_when_no_token_configured() {
        // An empty configured token must never authorize.
        assert!(authorize(Some("chrome-extension://x"), Some(""), "").is_err());
    }

    #[test]
    fn rate_limiter_blocks_after_max() {
        let rl = RateLimiter::new(Duration::from_secs(10), 3);
        assert!(rl.allow());
        assert!(rl.allow());
        assert!(rl.allow());
        assert!(!rl.allow(), "4th request in window should be blocked");
    }

    #[test]
    fn ct_eq_matches_only_equal() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }
}
