//! URL validation and SSRF defenses.
//!
//! Policy (see `docs/SECURITY_AND_POLICY.md` §4 and `docs/SYSTEM_DESIGN.md` §8):
//! - Only `http` and `https` are accepted.
//! - Embedded credentials (`user:pass@host`) are rejected.
//! - Resolved destination addresses in loopback, private, link-local,
//!   multicast, unspecified, or cloud-metadata ranges are blocked.
//!
//! The IP classification is pure and unit-tested. The resolve-and-check step
//! is applied before each connection (including every redirect hop) by the
//! engine. Pinning the resolved address to defeat DNS rebinding is a known
//! residual hardening item tracked for Phase 7.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use url::Url;

use crate::error::{EngineError, ErrorCode};

/// The IPv4 / IPv6 cloud metadata service address (AWS/GCP/Azure, etc.).
const METADATA_V4: Ipv4Addr = Ipv4Addr::new(169, 254, 169, 254);

/// Parse and validate a user- or browser-supplied download URL.
///
/// Returns the parsed [`Url`] on success, or a structured [`EngineError`].
pub fn validate_download_url(raw: &str) -> Result<Url, EngineError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(EngineError::new(ErrorCode::InvalidUrl, "The URL is empty."));
    }

    let parsed = Url::parse(trimmed)
        .map_err(|_| EngineError::new(ErrorCode::InvalidUrl, "The URL could not be parsed."))?;

    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(EngineError::new(
                ErrorCode::UnsupportedScheme,
                format!("The scheme '{other}' is not supported; only http and https are allowed."),
            ));
        }
    }

    // Reject embedded credentials. They are a common SSRF/phishing vector and
    // the product never replays passwords from a URL.
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(EngineError::new(
            ErrorCode::InvalidUrl,
            "URLs with embedded credentials are not allowed.",
        ));
    }

    if parsed.host_str().is_none() {
        return Err(EngineError::new(
            ErrorCode::InvalidUrl,
            "The URL has no host.",
        ));
    }

    Ok(parsed)
}

/// Classify an IP address as unsafe to connect to (SSRF policy).
///
/// Blocks loopback, private, link-local (incl. the metadata service),
/// unspecified, multicast, broadcast, documentation, and IPv6 unique-local /
/// mapped equivalents.
pub fn is_blocked_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_ipv4(v4),
        IpAddr::V6(v6) => is_blocked_ipv6(v6),
    }
}

fn is_blocked_ipv4(ip: &Ipv4Addr) -> bool {
    if *ip == METADATA_V4 {
        return true;
    }
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        // 100.64.0.0/10 carrier-grade NAT (shared address space, RFC 6598).
        || (ip.octets()[0] == 100 && (ip.octets()[1] & 0xC0) == 64)
        // 0.0.0.0/8 "this network".
        || ip.octets()[0] == 0
}

fn is_blocked_ipv6(ip: &Ipv6Addr) -> bool {
    // IPv4-mapped (::ffff:a.b.c.d) must be judged by its embedded v4 address.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_blocked_ipv4(&v4);
    }
    // IPv4-compatible (deprecated) addresses embed a v4 address too.
    if let Some(v4) = ip.to_ipv4()
        && is_blocked_ipv4(&v4)
    {
        return true;
    }
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        // Unique local addresses fc00::/7.
        || (ip.segments()[0] & 0xfe00) == 0xfc00
        // Link-local unicast fe80::/10.
        || (ip.segments()[0] & 0xffc0) == 0xfe80
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_http_and_https() {
        assert!(validate_download_url("http://example.com/a.bin").is_ok());
        assert!(validate_download_url("https://example.com/a.bin").is_ok());
    }

    #[test]
    fn rejects_non_http_schemes() {
        for raw in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ftp://example.com/x",
            "data:text/plain,hi",
        ] {
            let err = validate_download_url(raw).unwrap_err();
            assert_eq!(err.code, ErrorCode::UnsupportedScheme, "for {raw}");
        }
    }

    #[test]
    fn rejects_embedded_credentials() {
        let err = validate_download_url("https://user:pass@example.com/x").unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidUrl);
    }

    #[test]
    fn rejects_garbage_and_empty() {
        assert_eq!(
            validate_download_url("").unwrap_err().code,
            ErrorCode::InvalidUrl
        );
        assert_eq!(
            validate_download_url("not a url").unwrap_err().code,
            ErrorCode::InvalidUrl
        );
    }

    #[test]
    fn blocks_loopback_and_private_and_metadata() {
        let blocked = [
            "127.0.0.1",
            "10.0.0.5",
            "192.168.1.1",
            "172.16.0.1",
            "169.254.169.254", // metadata
            "0.0.0.0",
            "100.64.0.1", // CGNAT
            "::1",
            "fe80::1",
            "fc00::1",
            "::ffff:127.0.0.1", // v4-mapped loopback
            "::ffff:169.254.169.254",
        ];
        for s in blocked {
            let ip: IpAddr = s.parse().unwrap();
            assert!(is_blocked_ip(&ip), "{s} should be blocked");
        }
    }

    #[test]
    fn allows_public_addresses() {
        let allowed = ["8.8.8.8", "1.1.1.1", "93.184.216.34", "2606:2800:220:1::1"];
        for s in allowed {
            let ip: IpAddr = s.parse().unwrap();
            assert!(!is_blocked_ip(&ip), "{s} should be allowed");
        }
    }
}
