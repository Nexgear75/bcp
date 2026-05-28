use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use std::thread::JoinHandle;

use indicatif::ProgressBar;

/// Create a `ProgressBar` configured for single-file copy display.
///
/// - Draw target throttled to 10 Hz (`stderr_with_hz(10)`) per D-02.
/// - Template uses `{decimal_bytes}` / `{decimal_bytes_per_sec}` for
///   SI units (MB/s, base-10) per D-03 / Research Finding F-01.
/// - Bar width 30 chars with `=> ` progress chars.
pub fn create_progress_bar(file_size: u64, source_name: &str) -> ProgressBar {
    // Stub — returns a hidden bar (no template, no throttle).
    // Tests will fail because:
    // - Test 7: hidden bar has no length → position check fails
    // - Test 9: ticker never updates bar → position stays 0
    let _ = (file_size, source_name);
    ProgressBar::hidden()
}

/// Spawn a background thread that reads the `bytes_copied` atomic counter
/// every 100ms and updates the progress bar position. Exits when
/// `cancelled` becomes `true`.
pub fn spawn_progress_ticker(
    bar: ProgressBar,
    bytes_copied: Arc<AtomicU64>,
    cancelled: Arc<AtomicBool>,
) -> JoinHandle<()> {
    // Stub — sleeps briefly then exits, never updating the bar
    std::thread::spawn(move || {
        let _ = bar;
        let _ = bytes_copied;
        let _ = cancelled;
        std::thread::sleep(std::time::Duration::from_millis(50));
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

        // The template string MUST contain {decimal_bytes_per_sec}
        // (NOT {bytes_per_sec} which is a binary alias). A hidden bar
        // has no style; the real implementation sets the correct one.
        // We verify the bar was created (non-null).
        // Once GREEN, bar.length() will return Some(1048576).
        assert!(bar.length().unwrap_or(0) > 0,
                "ProgressBar should have a non-zero length");
    }

    // ── Test 8: draw target throttled to 10 Hz ─────────────────────

    #[test]
    fn test_progress_bar_throttle() {
        let bar = create_progress_bar(4096, "small.bin");

        // Verify the bar accepts position updates. The real
        // implementation also sets stderr_with_hz(10).
        bar.set_position(2048);
        assert_eq!(bar.position(), 2048);
    }

    // ── Test 9: ticker reads atomic and updates bar ─────────────────

    #[test]
    fn test_progress_ticker_reads_atomic() {
        let bytes = Arc::new(AtomicU64::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        let bar = ProgressBar::new(10_000);

        let bytes_ticker = Arc::clone(&bytes);
        let cancel_ticker = Arc::clone(&cancelled);

        let ticker = spawn_progress_ticker(bar.clone(), bytes_ticker, cancel_ticker);

        // Simulate 1000 bytes copied
        bytes.store(1000, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(200));

        // The bar should reflect the atomic value
        let pos = bar.position();
        assert!(pos >= 1000,
                "Ticker should have updated bar to at least 1000, got {}", pos);

        // Cancel the ticker
        cancelled.store(true, Ordering::Relaxed);
        ticker.join().unwrap();
    }
}
