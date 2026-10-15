//! Single-stream HTTP/HTTPS download with safe finalization.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use url::{Host, Url};
use uuid::Uuid;

use crate::download_engine::{CancelToken, DownloadComplete, NetPolicy, Progress};
use crate::error::{EngineError, ErrorCode, redact_url};
use crate::security::filename::{
    filename_from_content_disposition, filename_from_url, resolve_destination,
};
use crate::security::url::{is_blocked_ip, validate_download_url};

const MAX_REDIRECTS: usize = 5;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const READ_TIMEOUT: Duration = Duration::from_secs(120);

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

/// Download `req.url` into `req.dest_dir` as a single stream.
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

    // Follow redirects manually, re-validating scheme and SSRF policy each hop.
    let mut current = validated;
    let mut response = None;
    for _hop in 0..=MAX_REDIRECTS {
        ensure_host_allowed(&current, policy).await?;

        let resp = client
            .get(current.clone())
            .send()
            .await
            .map_err(|e| map_reqwest_error(&current, e))?;

        let status = resp.status();
        if status.is_redirection() {
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

        response = Some(resp);
        break;
    }

    let response = response.ok_or_else(|| {
        EngineError::new(
            ErrorCode::NetworkError,
            "The server exceeded the maximum number of redirects.",
        )
    })?;

    // Map the final HTTP status.
    let status = response.status();
    if !status.is_success() {
        return Err(map_status(status.as_u16()));
    }

    // Decide the filename: explicit override, else Content-Disposition, else URL.
    let filename = if let Some(name) = req.filename.as_deref() {
        crate::security::filename::sanitize_filename(name)
    } else if let Some(name) = response
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(filename_from_content_disposition)
    {
        name
    } else {
        filename_from_url(&current)
    };

    let dest = resolve_destination(&req.dest_dir, &filename)?;

    // Never overwrite silently: refuse before doing any work.
    if dest.exists() {
        return Err(EngineError::new(
            ErrorCode::DestinationExists,
            format!(
                "A file named '{}' already exists in the destination folder.",
                dest.file_name().and_then(|n| n.to_str()).unwrap_or("")
            ),
        ));
    }

    let total = response.content_length();

    // Write to a unique temporary file in the same directory, then rename.
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
            // Guard removes the temp file on drop.
            return Err(EngineError::new(
                ErrorCode::Cancelled,
                "The download was cancelled.",
            ));
        }

        let bytes = chunk.map_err(|e| map_reqwest_error(&current, e))?;
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

    // A declared length that was not reached means a truncated transfer.
    if let Some(expected) = total
        && downloaded != expected
    {
        return Err(EngineError::new(
            ErrorCode::NetworkError,
            "The connection ended before the whole file was received.",
        ));
    }

    // Flush durably before finalizing.
    file.flush().await.map_err(map_io_error)?;
    file.sync_all().await.map_err(map_io_error)?;
    drop(file);

    // Re-check right before the rename to avoid clobbering a file that
    // appeared during the transfer. (A small TOCTOU window remains; full
    // atomic reservation is a Phase 7 hardening item.)
    if dest.exists() {
        return Err(EngineError::new(
            ErrorCode::DestinationExists,
            "The destination file appeared during the download; not overwriting.",
        ));
    }

    tokio::fs::rename(&temp_path, &dest)
        .await
        .map_err(map_io_error)?;
    guard.disarm();

    Ok(DownloadComplete {
        path: dest,
        bytes: downloaded,
    })
}

/// Resolve the host to IP addresses and reject any blocked target.
async fn ensure_host_allowed(u: &Url, policy: NetPolicy) -> Result<(), EngineError> {
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
fn map_status(status: u16) -> EngineError {
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

fn map_reqwest_error(url: &Url, e: reqwest::Error) -> EngineError {
    // Never include the raw URL (may carry signed query params) in messages.
    let _ = url;
    EngineError::new(
        ErrorCode::NetworkError,
        format!("A network error occurred: {e}"),
    )
}

fn map_io_error(e: std::io::Error) -> EngineError {
    use std::io::ErrorKind;
    match e.kind() {
        ErrorKind::PermissionDenied => EngineError::new(
            ErrorCode::PermissionDenied,
            "Permission to write the file was denied.",
        ),
        _ => {
            // ENOSPC (28, Unix) / ERROR_DISK_FULL (112, Windows).
            if matches!(e.raw_os_error(), Some(28) | Some(112)) {
                EngineError::new(ErrorCode::DiskFull, "The disk is full.")
            } else {
                EngineError::new(
                    ErrorCode::InternalError,
                    format!("Writing to disk failed: {e}"),
                )
            }
        }
    }
}

/// Build a unique temp path next to the destination (same filesystem so the
/// final rename is atomic).
fn temp_path_for(dest: &Path) -> PathBuf {
    let mut name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download")
        .to_string();
    name.push('.');
    name.push_str(&Uuid::new_v4().simple().to_string());
    name.push_str(".part");
    dest.with_file_name(name)
}

/// Removes its temp file on drop unless disarmed after a successful rename.
struct TempGuard {
    path: PathBuf,
    armed: bool,
}

impl TempGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if self.armed {
            // Best-effort cleanup of the partial file.
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
