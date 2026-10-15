//! Filename sanitization and safe destination resolution.
//!
//! Server-supplied names (URL path, `Content-Disposition`) are untrusted.
//! We strip path separators, parent references, control characters, and
//! Windows-reserved names, then confirm the final path stays inside the
//! user-approved directory (see `docs/SECURITY_AND_POLICY.md` §5).

use std::path::{Component, Path, PathBuf};

use crate::error::{EngineError, ErrorCode};

// NOTE: we intentionally avoid pulling an extra percent-decoding dependency;
// `simple_percent_decode` below handles the `%XX` sequences we need for
// Content-Disposition's RFC 5987 extended form.

/// Windows reserved device names (case-insensitive, without extension).
const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Sanitize a candidate filename into a single safe path component.
///
/// Guarantees the result contains no path separators, is non-empty, is not a
/// reserved device name, and does not end in a dot or space (invalid on
/// Windows). Falls back to `"download"` when nothing usable remains.
pub fn sanitize_filename(candidate: &str) -> String {
    // Take only the final segment in case separators slipped in.
    let last = candidate.rsplit(['/', '\\']).next().unwrap_or(candidate);

    let mut out = String::with_capacity(last.len());
    for ch in last.chars() {
        let replaced = match ch {
            // Illegal on Windows / problematic across platforms.
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        };
        out.push(replaced);
    }

    // Trim trailing dots/spaces (invalid as Windows filenames) and whitespace.
    let trimmed = out.trim().trim_end_matches(['.', ' ']).trim();
    let mut name = trimmed.to_string();

    if name.is_empty() || name == "." || name == ".." {
        return "download".to_string();
    }

    // Guard against reserved device names (with or without extension).
    let stem = name.split('.').next().unwrap_or(&name).to_ascii_lowercase();
    if RESERVED.contains(&stem.as_str()) {
        name = format!("_{name}");
    }

    // Cap length to stay well under filesystem limits.
    if name.len() > 200 {
        name.truncate(200);
    }
    name
}

/// Derive a filename from the URL path when the server gives no better name.
pub fn filename_from_url(parsed: &url::Url) -> String {
    let last = parsed
        .path_segments()
        .and_then(|mut segs| segs.next_back())
        .unwrap_or("");
    sanitize_filename(last)
}

/// Parse a `Content-Disposition` header value for a `filename`.
///
/// Supports both `filename="x"` and RFC 5987 `filename*=UTF-8''x`. Returns a
/// sanitized name, or `None` if no filename is present.
pub fn filename_from_content_disposition(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    // Prefer the extended form if present.
    if let Some(idx) = lower.find("filename*=") {
        let rest = &value[idx + "filename*=".len()..];
        // Form: charset'lang'pct-encoded
        let encoded = rest.split(';').next().unwrap_or("").trim();
        if let Some(pos) = encoded.rfind('\'') {
            let name = &encoded[pos + 1..];
            let decoded = simple_percent_decode(name);
            let s = sanitize_filename(&decoded);
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    if let Some(idx) = lower.find("filename=") {
        let rest = &value[idx + "filename=".len()..];
        let raw = rest.split(';').next().unwrap_or("").trim();
        let unquoted = raw.trim_matches('"');
        let s = sanitize_filename(unquoted);
        if !s.is_empty() {
            return Some(s);
        }
    }
    None
}

/// Minimal percent-decoder for `%XX` sequences (ASCII/UTF-8 bytes).
fn simple_percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Resolve a final destination path inside `dir`, rejecting any escape.
///
/// `dir` must exist and be a directory (it is the user-approved folder). The
/// returned path is `dir`'s canonical form joined with the sanitized name, and
/// is guaranteed to have `dir` as its parent.
pub fn resolve_destination(dir: &Path, filename: &str) -> Result<PathBuf, EngineError> {
    let safe_name = sanitize_filename(filename);

    let canonical_dir = dir.canonicalize().map_err(|e| {
        EngineError::new(
            ErrorCode::PermissionDenied,
            format!("The destination folder is not accessible: {e}"),
        )
    })?;
    if !canonical_dir.is_dir() {
        return Err(EngineError::new(
            ErrorCode::PermissionDenied,
            "The destination is not a directory.",
        ));
    }

    let candidate = canonical_dir.join(&safe_name);

    // Defense in depth: the joined path must have exactly the canonical dir as
    // its parent and must not contain any `..`/root components.
    let has_bad_component = Path::new(&safe_name)
        .components()
        .any(|c| !matches!(c, Component::Normal(_)));
    if has_bad_component || candidate.parent() != Some(canonical_dir.as_path()) {
        return Err(EngineError::new(
            ErrorCode::PermissionDenied,
            "The resolved destination escapes the selected folder.",
        ));
    }

    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_separators_and_traversal() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("a/b/c.txt"), "c.txt");
        assert_eq!(sanitize_filename("..\\..\\win.ini"), "win.ini");
        assert_eq!(sanitize_filename(".."), "download");
        assert_eq!(sanitize_filename("."), "download");
    }

    #[test]
    fn strips_illegal_and_control_chars() {
        assert_eq!(sanitize_filename("a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(sanitize_filename("na\u{0000}me.bin"), "na_me.bin");
    }

    #[test]
    fn empty_and_dotted_fallback() {
        assert_eq!(sanitize_filename(""), "download");
        assert_eq!(sanitize_filename("   "), "download");
        assert_eq!(sanitize_filename("..."), "download");
    }

    #[test]
    fn guards_reserved_names() {
        assert_eq!(sanitize_filename("CON"), "_CON");
        assert_eq!(sanitize_filename("nul.txt"), "_nul.txt");
        assert_eq!(sanitize_filename("coma.txt"), "coma.txt"); // not reserved
    }

    #[test]
    fn content_disposition_plain_and_extended() {
        assert_eq!(
            filename_from_content_disposition("attachment; filename=\"report.pdf\""),
            Some("report.pdf".to_string())
        );
        assert_eq!(
            filename_from_content_disposition("attachment; filename*=UTF-8''my%20file.bin"),
            Some("my file.bin".to_string())
        );
        // A traversal attempt in the header is neutralized.
        assert_eq!(
            filename_from_content_disposition("attachment; filename=\"../../evil.sh\""),
            Some("evil.sh".to_string())
        );
    }

    #[test]
    fn url_filename_fallback() {
        let u = url::Url::parse("https://host/a/b/video.mp4?x=1").unwrap();
        assert_eq!(filename_from_url(&u), "video.mp4");
        let root = url::Url::parse("https://host/").unwrap();
        assert_eq!(filename_from_url(&root), "download");
    }

    #[test]
    fn resolve_destination_stays_inside_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        let ok = resolve_destination(dir, "file.bin").unwrap();
        assert_eq!(ok.parent().unwrap(), dir.canonicalize().unwrap());

        // Even a malicious name resolves to a single component inside dir.
        let escaped = resolve_destination(dir, "../../../etc/passwd").unwrap();
        assert_eq!(escaped.parent().unwrap(), dir.canonicalize().unwrap());
        assert_eq!(escaped.file_name().unwrap(), "passwd");
    }
}
