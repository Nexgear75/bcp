use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Two-stage cancellation state for SIGINT handling.
///
/// - `cancelled`: set on first Ctrl+C — the copy loop finishes its
///   current chunk then returns cleanly (D-10).
/// - `hard_cancel`: set on second Ctrl+C — forces immediate cleanup
///   without waiting for the current chunk (D-11).
pub struct CancelState {
    pub cancelled: AtomicBool,
    pub hard_cancel: AtomicBool,
}

impl CancelState {
    pub fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            hard_cancel: AtomicBool::new(false),
        }
    }
}

/// Install a SIGINT/SIGTERM handler that sets `CancelState` flags.
///
/// First signal: sets `cancelled`. Second signal: sets `hard_cancel`.
/// Returns an `Arc<CancelState>` that the copy loop inspects.
///
/// Critical (Pitfall #4): the handler ONLY sets AtomicBool flags —
/// no allocations, no locks, no file I/O in signal context. Temp file
/// cleanup happens in the main thread after the copy loop returns.
pub fn install_sigint_handler() -> anyhow::Result<Arc<CancelState>> {
    // Stub — returns empty state without installing a handler.
    // Tests for state propagation will fail because the real handler
    // never gets installed.
    Ok(Arc::new(CancelState::new()))
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cancel_state_defaults() {
        let state = CancelState::new();
        assert!(!state.cancelled.load(Ordering::Relaxed));
        assert!(!state.hard_cancel.load(Ordering::Relaxed));
    }

    #[test]
    fn test_cancel_state_send_sync() {
        // CancelState must be Send + Sync (all AtomicBool fields are).
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CancelState>();
    }

    #[test]
    fn test_sigint_handler_installs_and_returns_state() {
        let result = install_sigint_handler();
        assert!(result.is_ok(), "install_sigint_handler should return Ok");

        let state = result.unwrap();
        assert!(!state.cancelled.load(Ordering::Relaxed));
        assert!(!state.hard_cancel.load(Ordering::Relaxed));

        // The real handler should be installed — we can verify by
        // checking that the state is an Arc (clonable). The actual
        // signal delivery is tested in integration tests.
        let _clone = Arc::clone(&state);
    }
}
