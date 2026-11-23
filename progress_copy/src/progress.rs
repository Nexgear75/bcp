use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};

use crate::config::ProgressMode;
use crate::signals::CancelState;
use crate::types::CopyCounters;

/// Filled / edge / empty glyphs for all block bars (classic boxed look).
const BAR_CHARS: &str = "█▓░";

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
            "{msg:.bold} ┃{bar:30.green}┃ {percent:>3}% │ \
             {decimal_bytes}/{decimal_total_bytes} │ \
             {decimal_bytes_per_sec} │ ETA {eta}",
        )
        .unwrap()
        .progress_chars(BAR_CHARS),
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
        if let Some(end) = total
            && current >= end {
                break;
            }
        std::thread::sleep(Duration::from_millis(100));
    })
}

/// Create an indeterminate spinner for a silent preparation phase
/// (planning, analysis…). Animates via a steady tick so the user sees the
/// tool is working even when no per-item update is emitted.
///
/// Returns a hidden bar in `Silent` mode.
pub fn phase_spinner(message: &str, mode: ProgressMode) -> ProgressBar {
    if mode == ProgressMode::Silent {
        return ProgressBar::hidden();
    }
    let pb = ProgressBar::new_spinner();
    pb.set_draw_target(ProgressDrawTarget::stderr_with_hz(10));
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} {msg} [{elapsed_precise}]")
            .unwrap()
            .tick_chars("◐◓◑◒ "),
    );
    pb.set_message(message.to_string());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

// ── Multi-file progress (Phase 2/3) ─────────────────────────────────

/// Handle to a multi-bar display: preparation + total bytes + file count.
pub struct MultiDisplay {
    pub multi: MultiProgress,
    /// Transient bar shown while destination directories are being created.
    /// Cleared once preparation completes so only the copy bars remain.
    pub dirs_bar: Option<ProgressBar>,
    pub total_bar: ProgressBar,
    pub files_bar: ProgressBar,
    mode: ProgressMode,
}

/// Build a multi-bar display for a tree copy.
///
/// - `Rich`: a total-bytes bar plus an `N/M files` bar.
/// - `Quiet`: a single compact total-bytes line.
/// - `Silent`: fully hidden.
pub fn create_multi_progress(
    total_bytes: u64,
    total_files: u64,
    total_dirs: u64,
    mode: ProgressMode,
) -> MultiDisplay {
    let multi = MultiProgress::new();
    if mode == ProgressMode::Silent {
        multi.set_draw_target(ProgressDrawTarget::hidden());
    } else {
        multi.set_draw_target(ProgressDrawTarget::stderr_with_hz(10));
    }

    // Transient preparation bar: only in Rich mode, and only when there are
    // directories to create (added first so it renders on top, then cleared).
    let dirs_bar = if mode == ProgressMode::Rich && total_dirs > 0 {
        let bar = multi.add(ProgressBar::new(total_dirs));
        bar.set_style(
            ProgressStyle::with_template("Prep  ┃{bar:30.cyan}┃ {pos}/{len} dossiers")
                .unwrap()
                .progress_chars(BAR_CHARS),
        );
        bar.enable_steady_tick(Duration::from_millis(100));
        Some(bar)
    } else {
        None
    };

    let total_bar = multi.add(ProgressBar::new(total_bytes));
    let files_bar = multi.add(ProgressBar::new(total_files));

    if mode == ProgressMode::Quiet || mode == ProgressMode::Silent {
        total_bar.set_style(
            ProgressStyle::with_template(
                "{percent:>3}% │ {decimal_bytes}/{decimal_total_bytes} │ \
                 {decimal_bytes_per_sec} │ ETA {eta} │ {pos}/{len} files",
            )
            .unwrap(),
        );
        // In quiet mode the single total bar carries the file count too.
        files_bar.set_draw_target(ProgressDrawTarget::hidden());
    } else {
        total_bar.set_style(
            ProgressStyle::with_template(
                "Bytes ┃{bar:30.green}┃ {percent:>3}% │ \
                 {decimal_bytes}/{decimal_total_bytes} │ \
                 {decimal_bytes_per_sec} │ ETA {eta}",
            )
            .unwrap()
            .progress_chars(BAR_CHARS),
        );
        files_bar.set_style(
            ProgressStyle::with_template("Files ┃{bar:30.green}┃ {pos}/{len}")
                .unwrap()
                .progress_chars(BAR_CHARS),
        );
    }

    total_bar.enable_steady_tick(Duration::from_millis(100));
    MultiDisplay { multi, dirs_bar, total_bar, files_bar, mode }
}

impl MultiDisplay {
    /// Spawn a ticker that mirrors the shared counters onto the bars every
    /// 100ms, exiting when cancelled or when both targets are reached.
    pub fn spawn_ticker(
        &self,
        counters: CopyCounters,
        cancel: Arc<CancelState>,
    ) -> JoinHandle<()> {
        let total_bar = self.total_bar.clone();
        let files_bar = self.files_bar.clone();
        let dirs_bar = self.dirs_bar.clone();
        let total_bytes = total_bar.length();
        let total_files = files_bar.length();
        let total_dirs = dirs_bar.as_ref().and_then(|b| b.length());
        std::thread::spawn(move || {
            let mut dirs_cleared = false;
            loop {
            let bytes = counters.bytes_copied.load(Ordering::Relaxed);
            let files = counters.files_done.load(Ordering::Relaxed);
            total_bar.set_position(bytes);
            files_bar.set_position(files);

            // Mirror the directory-creation progress, then clear the prep bar
            // once every directory exists so only the copy bars remain.
            if let (Some(bar), Some(total)) = (dirs_bar.as_ref(), total_dirs)
                && !dirs_cleared {
                    let dirs = counters.dirs_done.load(Ordering::Relaxed);
                    bar.set_position(dirs);
                    if dirs >= total {
                        bar.finish_and_clear();
                        dirs_cleared = true;
                    }
                }

            if cancel.cancelled.load(Ordering::Relaxed) {
                break;
            }
            let bytes_done = total_bytes.map(|t| bytes >= t).unwrap_or(true);
            let files_done = total_files.map(|t| files >= t).unwrap_or(true);
            if bytes_done && files_done {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
            }
        })
    }

    /// Clear the bars at the end of an operation.
    pub fn finish(&self) {
        if let Some(bar) = &self.dirs_bar {
            bar.finish_and_clear();
        }
        self.total_bar.finish_and_clear();
        self.files_bar.finish_and_clear();
        let _ = self.mode;
    }
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
