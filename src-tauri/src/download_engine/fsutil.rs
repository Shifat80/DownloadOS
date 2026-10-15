//! Shared filesystem helpers: temp-file naming, cleanup guard, io-error
//! mapping, and safe finalization.

use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::error::{EngineError, ErrorCode};

/// Map a filesystem error to the engine error taxonomy.
pub(crate) fn map_io_error(e: std::io::Error) -> EngineError {
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
pub(crate) fn temp_path_for(dest: &Path) -> PathBuf {
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

/// Finalize a completed temp file to its destination without overwriting.
///
/// Re-checks existence immediately before the rename to avoid clobbering a
/// file that appeared during the transfer. A small TOCTOU window remains; a
/// fully atomic reservation is a Phase 7 hardening item.
pub(crate) async fn finalize(temp: &Path, dest: &Path) -> Result<(), EngineError> {
    if dest.exists() {
        return Err(EngineError::new(
            ErrorCode::DestinationExists,
            "The destination file appeared during the download; not overwriting.",
        ));
    }
    tokio::fs::rename(temp, dest).await.map_err(map_io_error)?;
    Ok(())
}

/// Removes its temp file on drop unless disarmed after a successful rename.
pub(crate) struct TempGuard {
    path: PathBuf,
    armed: bool,
}

impl TempGuard {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }
    pub(crate) fn disarm(&mut self) {
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
