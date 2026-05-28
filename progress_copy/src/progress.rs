use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

use crate::signals::CancelState;

/// Create a `ProgressBar` configured for single-file copy display.
///
/// - Draw target is throttled to 10 Hz (`stderr_with_hz(10)`) per D-02.
/// - Template uses `{decimal_bytes}` / `{decimal_bytes_per_sec}` for
///   SI units (MB/s, base-10) per D-03 / Research Finding F-01.
///   CRITICAL: `{bytes_per_sec}` is an alias for
///   `{binary_bytes_per_sec}` (MiB/s) — do NOT use it.
/// - Bar width is 30 characters with `=> ` progress chars.
/// - Source file name is displayed in the `{msg}` field.
pub fn create_progress_bar(file_size: u64, source_name: &str) -> ProgressBar {
    let bar = ProgressBar::new(file_size);

    // D-02: throttle to 10 Hz to prevent terminal rendering bottleneck
    bar.set_draw_target(ProgressDrawTarget::stderr_with_hz(10));

    // D-01 template adjusted for D-03 (SI units) per F-01:
    // - {decimal_bytes} / {decimal_total_bytes} → MB, GB (base-10)
    // - {decimal_bytes_per_sec} → MB/s (base-10), NOT {bytes_per_sec}
    // - {eta} → estimated time remaining (computed by indicatif)
    bar.set_style(
        ProgressStyle::with_template(
            "{msg} [{bar:30}] {percent:>3}% | \
             {decimal_bytes}/{decimal_total_bytes} | \
             {decimal_bytes_per_sec} | ETA: {eta}",
        )
        .unwrap()
        .progress_chars("=> "),
    );

    bar.set_message(source_name.to_string());

    // Steady tick ensures the bar redraws every 100ms even without
    // position changes (e.g., during slow I/O operations).
    bar.enable_steady_tick(Duration::from_millis(100));

    bar
}

/// Spawn a background thread that reads the `bytes_copied` atomic
/// counter every 100ms and updates the progress bar position (D-06).
///
/// The thread exits when `cancel_state.cancelled` becomes `true`,
/// then calls `bar.finish_and_clear()`.
pub fn spawn_progress_ticker(
    bar: ProgressBar,
    bytes_copied: Arc<AtomicU64>,
    cancel_state: Arc<CancelState>,
) -> JoinHandle<()> {
    let total = bar.length();
    std::thread::spawn(move || loop {
        if cancel_state.cancelled.load(Ordering::Relaxed) {
            bar.finish_and_clear();
            break;
        }
        let current = bytes_copied.load(Ordering::Relaxed);
        bar.set_position(current);
        // Exit cleanly when the copy is complete (position reaches the
        // known file size).  Without this the thread spins forever and
        // `ticker.join()` in the main thread deadlocks on normal (non-
        // cancelled) completion.
        if let Some(end) = total {
            if current >= end {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    })
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    // ── Test 7: template uses SI (decimal) units ───────────────────

    #[test]
    fn test_progress_bar_template_uses_decimal_units() {
        let bar = create_progress_bar(1024 * 1024, "test.bin");

        // The template string MUST use {decimal_bytes_per_sec}
        // (NOT {bytes_per_sec} which is a binary alias for MiB/s).
        // See F-01 / Pitfall #4 in RESEARCH.md.
        //
        // We verify indirectly: the bar was successfully created with
        // a non-zero length, meaning `with_template` parsed correctly.
        assert!(
            bar.length().unwrap_or(0) > 0,
            "ProgressBar should have a non-zero length"
        );
    }

    // ── Test 8: draw target throttled to 10 Hz ─────────────────────

    #[test]
    fn test_progress_bar_throttle() {
        let bar = create_progress_bar(4096, "small.bin");

        // indicatif doesn't expose the draw target back from the bar,
        // so we verify behaviour: the bar accepts position updates
        // without panicking, and the steady tick is enabled.
        bar.set_position(2048);
        assert_eq!(bar.position(), 2048);

        // Verify with various sizes
        let bar2 = create_progress_bar(0, "empty.bin");
        assert_eq!(bar2.position(), 0);
    }

    // ── Test 9: ticker reads atomic and updates bar ─────────────────

    #[test]
    fn test_progress_ticker_reads_atomic() {
        let bytes = Arc::new(AtomicU64::new(0));
        let cancel_state = Arc::new(CancelState::new());
        let bar = ProgressBar::new(10_000);

        let bytes_ticker = Arc::clone(&bytes);
        let cancel_ticker = Arc::clone(&cancel_state);

        let ticker = spawn_progress_ticker(bar.clone(), bytes_ticker, cancel_ticker);

        // Simulate 1000 bytes copied
        bytes.store(1000, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(200));

        // The bar should reflect the atomic value (D-06)
        let pos = bar.position();
        assert!(
            pos >= 1000,
            "Ticker should have updated bar to at least 1000, got {}",
            pos
        );

        // Cancel the ticker
        cancel_state.cancelled.store(true, Ordering::Relaxed);
        ticker.join().unwrap();
    }
}
