//! The download task state machine (centralized and validated).
//!
//! Mirrors the diagram in `docs/SYSTEM_DESIGN.md` §6. All transitions go
//! through [`TaskState::can_transition`] so no component can move a task into
//! an inconsistent state.

use serde::{Deserialize, Serialize};

/// The lifecycle state of a download task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Resolving,
    Downloading,
    Paused,
    Retrying,
    Verifying,
    Completed,
    Failed,
    Cancelled,
}

impl TaskState {
    /// Stable lowercase string used for database storage.
    pub fn as_str(self) -> &'static str {
        match self {
            TaskState::Queued => "queued",
            TaskState::Resolving => "resolving",
            TaskState::Downloading => "downloading",
            TaskState::Paused => "paused",
            TaskState::Retrying => "retrying",
            TaskState::Verifying => "verifying",
            TaskState::Completed => "completed",
            TaskState::Failed => "failed",
            TaskState::Cancelled => "cancelled",
        }
    }

    /// Parse the stored string form back into a state.
    pub fn from_db(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => TaskState::Queued,
            "resolving" => TaskState::Resolving,
            "downloading" => TaskState::Downloading,
            "paused" => TaskState::Paused,
            "retrying" => TaskState::Retrying,
            "verifying" => TaskState::Verifying,
            "completed" => TaskState::Completed,
            "failed" => TaskState::Failed,
            "cancelled" => TaskState::Cancelled,
            _ => return None,
        })
    }

    /// Terminal states never transition further.
    pub fn is_terminal(self) -> bool {
        matches!(self, TaskState::Completed | TaskState::Cancelled)
    }

    /// Whether a task in this state was actively in flight and must be
    /// recovered to a safe state after an unclean shutdown.
    pub fn is_interrupted_on_restart(self) -> bool {
        matches!(
            self,
            TaskState::Resolving
                | TaskState::Downloading
                | TaskState::Retrying
                | TaskState::Verifying
        )
    }

    /// Validate a transition against the state diagram.
    ///
    /// Cancellation is permitted from any non-terminal state (a user may
    /// cancel a queued, resolving, running, paused, or retrying task).
    pub fn can_transition(self, to: TaskState) -> bool {
        use TaskState::*;

        // No transition out of a terminal state.
        if self.is_terminal() {
            return false;
        }
        // Cancel from any live state.
        if to == Cancelled {
            return true;
        }

        matches!(
            (self, to),
            (Queued, Resolving)
                | (Resolving, Downloading)
                | (Resolving, Failed)
                | (Downloading, Paused)
                | (Downloading, Retrying)
                | (Downloading, Verifying)
                | (Downloading, Failed)
                | (Retrying, Resolving)
                | (Paused, Resolving)
                | (Verifying, Completed)
                | (Verifying, Failed)
                // Retry a failed task.
                | (Failed, Queued)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::TaskState::*;
    use super::*;

    #[test]
    fn string_roundtrip() {
        for s in [
            Queued,
            Resolving,
            Downloading,
            Paused,
            Retrying,
            Verifying,
            Completed,
            Failed,
            Cancelled,
        ] {
            assert_eq!(TaskState::from_db(s.as_str()), Some(s));
        }
        assert_eq!(TaskState::from_db("bogus"), None);
    }

    #[test]
    fn happy_path_transitions_allowed() {
        assert!(Queued.can_transition(Resolving));
        assert!(Resolving.can_transition(Downloading));
        assert!(Downloading.can_transition(Verifying));
        assert!(Verifying.can_transition(Completed));
        assert!(Downloading.can_transition(Paused));
        assert!(Paused.can_transition(Resolving));
        assert!(Downloading.can_transition(Retrying));
        assert!(Retrying.can_transition(Resolving));
        assert!(Failed.can_transition(Queued)); // retry
    }

    #[test]
    fn cancel_allowed_from_live_states() {
        for s in [Queued, Resolving, Downloading, Paused, Retrying, Verifying] {
            assert!(s.can_transition(Cancelled), "{s:?} -> Cancelled");
        }
    }

    #[test]
    fn terminal_states_are_frozen() {
        for s in [Completed, Cancelled] {
            for to in [Queued, Resolving, Downloading, Failed, Cancelled] {
                assert!(!s.can_transition(to), "{s:?} must not move to {to:?}");
            }
        }
    }

    #[test]
    fn invalid_jumps_rejected() {
        assert!(!Queued.can_transition(Downloading)); // must resolve first
        assert!(!Queued.can_transition(Completed));
        assert!(!Downloading.can_transition(Completed)); // must verify first
        assert!(!Resolving.can_transition(Paused));
        assert!(!Completed.can_transition(Queued));
    }
}
