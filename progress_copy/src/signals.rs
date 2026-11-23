//! SIGINT / SIGTERM handling.
//!
//! `ctrlc` runs our closure on a dedicated thread (not in signal context),
//! so it may take locks and touch the filesystem.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once, Weak};

/// Cancellation state of one run.
///
/// - `cancelled`: "stop as soon as possible". Set by Ctrl+C **and** by a
///   `--strict` failure. Copy loops poll it between chunks.
/// - `interrupted`: set only by a signal, so a strict failure is never
///   reported as a user interruption.
#[derive(Debug, Default)]
pub struct CancelState {
    pub cancelled: AtomicBool,
    pub interrupted: AtomicBool,
}

impl CancelState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Stop the run because of an error (not a user interruption).
    pub fn abort(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn is_interrupted(&self) -> bool {
        self.interrupted.load(Ordering::Relaxed)
    }
}

static INSTALL: Once = Once::new();
static ACTIVE: Mutex<Vec<Weak<CancelState>>> = Mutex::new(Vec::new());
static SIGNALLED: AtomicBool = AtomicBool::new(false);

fn on_signal() {
    if SIGNALLED.swap(true, Ordering::SeqCst) {
        // Second signal: remove our temp files, then leave immediately.
        let removed = crate::atomic::cleanup_registered_temps();
        eprintln!("\nForce quit ({removed} temporary file(s) removed).");
        std::process::exit(130);
    }
    eprintln!("\nCancelling… finishing current chunk (press Ctrl+C again to force)");
    if let Ok(mut active) = ACTIVE.lock() {
        active.retain(|w| {
            if let Some(state) = w.upgrade() {
                state.interrupted.store(true, Ordering::SeqCst);
                state.cancelled.store(true, Ordering::SeqCst);
                true
            } else {
                false
            }
        });
    }
}

/// Install the process-wide SIGINT/SIGTERM/SIGHUP handler (once) and return
/// a fresh per-run [`CancelState`] wired to it.
///
/// # Errors
/// Fails if another library already owns the signal handler.
pub fn install_sigint_handler() -> anyhow::Result<Arc<CancelState>> {
    let mut install_err = None;
    INSTALL.call_once(|| match ctrlc::try_set_handler(on_signal) {
        Ok(()) | Err(ctrlc::Error::MultipleHandlers) => {}
        Err(e) => install_err = Some(e),
    });
    if let Some(e) = install_err {
        anyhow::bail!("failed to install signal handler: {e}");
    }

    let state = Arc::new(CancelState::new());
    if let Ok(mut active) = ACTIVE.lock() {
        active.retain(|w| w.strong_count() > 0);
        active.push(Arc::downgrade(&state));
    }
    if SIGNALLED.load(Ordering::SeqCst) {
        state.interrupted.store(true, Ordering::SeqCst);
        state.cancelled.store(true, Ordering::SeqCst);
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_clear() {
        let s = CancelState::new();
        assert!(!s.is_cancelled());
        assert!(!s.is_interrupted());
    }

    #[test]
    fn abort_is_not_an_interruption() {
        let s = CancelState::new();
        s.abort();
        assert!(s.is_cancelled());
        assert!(!s.is_interrupted(), "strict abort must not look like Ctrl+C");
    }

    #[test]
    fn states_are_independent_per_run() {
        let a = install_sigint_handler().unwrap();
        let b = install_sigint_handler().unwrap();
        a.abort();
        assert!(!b.is_cancelled(), "aborting one run must not cancel another");
    }

    #[test]
    fn send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CancelState>();
    }
}
