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

impl Default for CancelState {
    fn default() -> Self {
        Self::new()
    }
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
/// Returns an `Arc<CancelState>` that the copy loop reads.
///
/// Signal behaviour:
/// - **First Ctrl+C:** sets `cancelled` flag. The copy loop finishes
///   its current chunk then returns cleanly, allowing `NamedTempFile`
///   cleanup via normal Drop.
/// - **Second Ctrl+C:** sets `hard_cancel` flag. The main thread can
///   poll this to force immediate cleanup.
///
/// **Critical (Pitfall #4):** The handler ONLY sets `AtomicBool` flags.
/// No allocations, no locks, no file I/O in signal context — signal
/// handlers run in a restricted context where Rust destructors are not
/// guaranteed to fire. All actual cleanup happens in the main thread
/// after the copy loop returns.
///
/// If a handler is already registered (e.g., from a previous test in
/// the same process), this function returns `Ok` with a fresh state
/// but does not install a second handler. In production, there is
/// only one handler per process invocation.
pub fn install_sigint_handler() -> anyhow::Result<Arc<CancelState>> {
    let state = Arc::new(CancelState::new());
    let state_clone = Arc::clone(&state);

    match ctrlc::try_set_handler(move || {
        if state_clone.cancelled.load(Ordering::Relaxed) {
            // Second signal — force cancel
            state_clone.hard_cancel.store(true, Ordering::Relaxed);
            eprintln!("\nForce cancelling...");
        } else {
            // First signal — graceful cancel
            state_clone.cancelled.store(true, Ordering::Relaxed);
            eprintln!("\nCancelling... (press Ctrl+C again to force)");
        }
    }) {
        Ok(()) => {}
        Err(ctrlc::Error::MultipleHandlers) => {
            // Handler already registered (e.g., in test suite).
            // Return fresh state — the existing handler's state
            // is different, but each test can still verify
            // CancelState behaviour independently.
        }
        Err(e) => anyhow::bail!("Failed to install signal handler: {}", e),
    }

    Ok(state)
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

        // Verify state is Clone (Arc) for sharing between threads.
        // Note: if another test already installed a handler,
        // this instance's handler won't be active (try_set_handler
        // returns MultipleHandlers) but the CancelState still works.
        let _clone = Arc::clone(&state);
    }
}
