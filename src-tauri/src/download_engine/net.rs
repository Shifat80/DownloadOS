//! Shared HTTP networking: client construction, SSRF checks, redirect-safe
//! GET requests, and status/error mapping. Used by both single-stream and
//! segmented downloads.

use std::net::IpAddr;
use std::time::Duration;

use url::{Host, Url};

use crate::download_engine::NetPolicy;
use crate::error::{EngineError, ErrorCode, redact_url};
use crate::security::url::is_blocked_ip;

const MAX_REDIRECTS: usize = 5;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const READ_TIMEOUT: Duration = Duration::from_secs(120);

/// Build the engine's HTTP client.
///
/// Redirects are disabled so the engine can re-validate every hop against the
/// SSRF policy. Connect/read timeouts bound stalled connections without
/// capping the total time of a legitimately large transfer.
pub fn build_client() -> Result<reqwest::Client, EngineError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .user_agent(concat!("DownloadOS/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| {
            EngineError::new(
                ErrorCode::InternalError,
                format!("Could not initialize the HTTP client: {e}"),
            )
        })
}

/// Issue a GET, following redirects while re-validating each hop against the
/// scheme and SSRF policy. Returns the final URL and the (unread) response.
///
/// Optional `range` and `if_range` header values support probing, segmented
/// transfers, and resume.
pub(crate) async fn send_get(
    client: &reqwest::Client,
    start: &Url,
    policy: NetPolicy,
    range: Option<&str>,
    if_range: Option<&str>,
) -> Result<(Url, reqwest::Response), EngineError> {
    let mut current = start.clone();
    for _hop in 0..=MAX_REDIRECTS {
        ensure_host_allowed(&current, policy).await?;

        let mut builder = client.get(current.clone());
        if let Some(r) = range {
            builder = builder.header(reqwest::header::RANGE, r);
        }
        if let Some(ir) = if_range {
            builder = builder.header(reqwest::header::IF_RANGE, ir);
        }

        let resp = builder
            .send()
            .await
            .map_err(|e| map_reqwest_error(&current, e))?;

        if resp.status().is_redirection() {
            let location = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    EngineError::new(
                        ErrorCode::NetworkError,
                        "The server sent a redirect without a Location header.",
                    )
                })?;
            let next = current.join(location).map_err(|_| {
                EngineError::new(
                    ErrorCode::InvalidUrl,
                    "The server sent an invalid redirect target.",
                )
            })?;
            revalidate_redirect(&next)?;
            current = next;
            continue;
        }

        return Ok((current, resp));
    }

    Err(EngineError::new(
        ErrorCode::NetworkError,
        "The server exceeded the maximum number of redirects.",
    ))
}

/// Resolve the host to IP addresses and reject any blocked target.
pub(crate) async fn ensure_host_allowed(u: &Url, policy: NetPolicy) -> Result<(), EngineError> {
    match u.host() {
        Some(Host::Ipv4(ip)) => check_ip(IpAddr::V4(ip), policy),
        Some(Host::Ipv6(ip)) => check_ip(IpAddr::V6(ip), policy),
        Some(Host::Domain(domain)) => {
            let port = u.port_or_known_default().unwrap_or(0);
            let mut addrs = tokio::net::lookup_host((domain, port))
                .await
                .map_err(|e| {
                    EngineError::new(
                        ErrorCode::NetworkError,
                        format!(
                            "Could not resolve the host for {}: {e}",
                            redact_url(u.as_str())
                        ),
                    )
                })?
                .peekable();

            if addrs.peek().is_none() {
                return Err(EngineError::new(
                    ErrorCode::NetworkError,
                    "The host did not resolve to any address.",
                ));
            }
            for addr in addrs {
                check_ip(addr.ip(), policy)?;
            }
            Ok(())
        }
        None => Err(EngineError::new(
            ErrorCode::InvalidUrl,
            "The URL has no host.",
        )),
    }
}

fn check_ip(ip: IpAddr, policy: NetPolicy) -> Result<(), EngineError> {
    if policy.allow_loopback && ip.is_loopback() {
        return Ok(());
    }
    if is_blocked_ip(&ip) {
        return Err(EngineError::new(
            ErrorCode::PolicyRestricted,
            "The destination address is not permitted (loopback, private, \
             link-local, or metadata range).",
        ));
    }
    Ok(())
}

/// Re-validate a redirect target's scheme and credentials.
fn revalidate_redirect(next: &Url) -> Result<(), EngineError> {
    match next.scheme() {
        "http" | "https" => {}
        other => {
            return Err(EngineError::new(
                ErrorCode::UnsupportedScheme,
                format!("A redirect pointed to an unsupported scheme '{other}'."),
            ));
        }
    }
    if !next.username().is_empty() || next.password().is_some() {
        return Err(EngineError::new(
            ErrorCode::InvalidUrl,
            "A redirect contained embedded credentials.",
        ));
    }
    Ok(())
}

/// Map an HTTP status code to the error taxonomy.
pub(crate) fn map_status(status: u16) -> EngineError {
    let (code, msg): (ErrorCode, String) = match status {
        401 => (
            ErrorCode::AuthRequired,
            "Authentication is required.".into(),
        ),
        403 => (
            ErrorCode::AuthRequired,
            "Access to this resource was refused.".into(),
        ),
        404 => (
            ErrorCode::HttpStatus,
            "The file was not found (404).".into(),
        ),
        410 => (
            ErrorCode::HttpStatus,
            "The file is no longer available (410).".into(),
        ),
        416 => (
            ErrorCode::RangeInvalid,
            "The server rejected the requested byte range (416).".into(),
        ),
        429 => (
            ErrorCode::HttpStatus,
            "The server is rate-limiting requests (429).".into(),
        ),
        s => (
            ErrorCode::HttpStatus,
            format!("The server returned HTTP status {s}."),
        ),
    };
    EngineError::new(code, msg)
}

pub(crate) fn map_reqwest_error(url: &Url, e: reqwest::Error) -> EngineError {
    // Never include the raw URL (may carry signed query params) in messages.
    let _ = url;
    EngineError::new(
        ErrorCode::NetworkError,
        format!("A network error occurred: {e}"),
    )
}
