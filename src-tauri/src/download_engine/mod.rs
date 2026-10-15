//! The download engine.
//!
//! Single-stream (Phase 1) and segmented/range transfers with resume and
//! bounded retries (Phase 3) share the networking, filesystem, and range
//! helpers in the submodules below.

pub mod fsutil;
pub mod net;
pub mod orchestrator;
pub mod range;
pub mod retry;
pub mod segmented;
pub mod single_stream;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

/// A cooperative cancellation flag shared with a running download.
///
/// The engine checks this between chunks and stops promptly, removing the
/// partial temp file. Cheap to clone; all clones observe the same state.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    /// Request cancellation. Idempotent.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// A cooperative pause flag shared with a running download.
///
/// Unlike [`CancelToken`], pausing preserves the partial file so the transfer
/// can later resume from where it stopped.
#[derive(Clone, Default)]
pub struct PauseToken(Arc<AtomicBool>);

impl PauseToken {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    /// Request a pause. Idempotent.
    pub fn pause(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_paused(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Network safety policy applied before every connection and redirect hop.
///
/// Secure by default: loopback and private/link-local/metadata addresses are
/// rejected. Integration tests set `allow_loopback` to reach a controlled
/// local fixture server (see `docs/TEST_PLAN.md`). Production paths use
/// [`NetPolicy::default`].
#[derive(Clone, Copy, Debug, Default)]
pub struct NetPolicy {
    pub allow_loopback: bool,
}

/// A progress update emitted as bytes are written.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Progress {
    /// Bytes written to disk so far (actual, never synthetic).
    pub downloaded: u64,
    /// Total expected bytes when the server provides a trustworthy length.
    pub total: Option<u64>,
}

/// The successful result of a completed download.
#[derive(Debug, Clone, Serialize)]
pub struct DownloadComplete {
    /// The finalized destination path.
    pub path: PathBuf,
    /// Total bytes written and verified.
    pub bytes: u64,
}
