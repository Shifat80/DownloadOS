//! Structured error taxonomy for the download engine.
//!
//! Every failure is mapped to a stable [`ErrorCode`] (see
//! `docs/SYSTEM_DESIGN.md` §10) plus a user-facing message. Messages are
//! constructed to never contain secrets (signed query strings, tokens,
//! credentials); use [`redact_url`] when a URL must appear in a message.

use std::fmt;

use serde::Serialize;

/// Stable, machine-readable error codes. Serialized as SCREAMING_SNAKE_CASE
/// so the UI and logs can branch on a code rather than a free-text string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidUrl,
    UnsupportedScheme,
    NetworkError,
    HttpStatus,
    AuthRequired,
    AuthExpired,
    RangeUnsupported,
    RangeInvalid,
    ResourceChanged,
    DiskFull,
    PermissionDenied,
    DestinationExists,
    Cancelled,
    UnsupportedMedia,
    PolicyRestricted,
    InternalError,
}

impl ErrorCode {
    /// A short, stable string form (useful for logs).
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::InvalidUrl => "INVALID_URL",
            ErrorCode::UnsupportedScheme => "UNSUPPORTED_SCHEME",
            ErrorCode::NetworkError => "NETWORK_ERROR",
            ErrorCode::HttpStatus => "HTTP_STATUS",
            ErrorCode::AuthRequired => "AUTH_REQUIRED",
            ErrorCode::AuthExpired => "AUTH_EXPIRED",
            ErrorCode::RangeUnsupported => "RANGE_UNSUPPORTED",
            ErrorCode::RangeInvalid => "RANGE_INVALID",
            ErrorCode::ResourceChanged => "RESOURCE_CHANGED",
            ErrorCode::DiskFull => "DISK_FULL",
            ErrorCode::PermissionDenied => "PERMISSION_DENIED",
            ErrorCode::DestinationExists => "DESTINATION_EXISTS",
            ErrorCode::Cancelled => "CANCELLED",
            ErrorCode::UnsupportedMedia => "UNSUPPORTED_MEDIA",
            ErrorCode::PolicyRestricted => "POLICY_RESTRICTED",
            ErrorCode::InternalError => "INTERNAL_ERROR",
        }
    }
}

/// A structured engine error: a stable code plus a redacted, user-safe message.
#[derive(Debug, Clone, Serialize)]
pub struct EngineError {
    pub code: ErrorCode,
    pub message: String,
}

impl EngineError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> ErrorCode {
        self.code
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for EngineError {}

/// Produce a display-safe form of a URL: scheme + host + path only.
///
/// Query strings and user-info (`user:pass@`) are dropped because they can
/// carry signed tokens, session identifiers, or credentials that must never
/// reach logs or the UI (see `docs/SECURITY_AND_POLICY.md` §8).
pub fn redact_url(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(u) => {
            let scheme = u.scheme();
            let host = u.host_str().unwrap_or("");
            let port = u.port().map(|p| format!(":{p}")).unwrap_or_default();
            let path = u.path();
            if u.query().is_some() {
                format!("{scheme}://{host}{port}{path}?<redacted>")
            } else {
                format!("{scheme}://{host}{port}{path}")
            }
        }
        // If it does not even parse, reveal nothing but the fact it was a URL.
        Err(_) => "<unparseable-url>".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_query_tokens() {
        let got = redact_url("https://cdn.example.com/a/b.mp4?token=SECRET&sig=abc");
        assert_eq!(got, "https://cdn.example.com/a/b.mp4?<redacted>");
        assert!(!got.contains("SECRET"));
        assert!(!got.contains("abc"));
    }

    #[test]
    fn redacts_userinfo() {
        let got = redact_url("https://user:pass@example.com/file.bin");
        assert!(!got.contains("pass"));
        assert!(!got.contains("user:"));
        assert_eq!(got, "https://example.com/file.bin");
    }

    #[test]
    fn keeps_port_and_path() {
        let got = redact_url("http://example.com:8080/dir/file.zip");
        assert_eq!(got, "http://example.com:8080/dir/file.zip");
    }

    #[test]
    fn unparseable_is_opaque() {
        assert_eq!(redact_url("not a url"), "<unparseable-url>");
    }
}
