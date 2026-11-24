//! Progress display: preparation bar, total bytes, file count.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};

use crate::config::ProgressMode;
use crate::types::CopyCounters;

/// Filled / edge / empty glyphs for all block bars.
const BAR_CHARS: &str = "█▓░";

fn style(template: &str) -> ProgressStyle {
    ProgressStyle::with_template(template).expect("static template").progress_chars(BAR_CHARS)
}

/// Indeterminate spinner for a silent preparation phase. Hidden unless the
/// display is `Rich`.
#[must_use]
pub fn phase_spinner(message: &str, mode: ProgressMode) -> ProgressBar {
    if mode != ProgressMode::Rich {
        return ProgressBar::hidden();
    }
    let pb = ProgressBar::new_spinner();
    pb.set_draw_target(ProgressDrawTarget::stderr_with_hz(10));
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} {msg} [{elapsed_precise}]")
            .expect("static template")
            .tick_chars("◐◓◑◒ "),
    );
    pb.set_message(message.to_string());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

/// Handle to a multi-bar display.
pub struct MultiDisplay {
    pub multi: MultiProgress,
    /// Transient bar while destination directories are created.
    pub dirs_bar: Option<ProgressBar>,
    pub total_bar: ProgressBar,
    pub files_bar: ProgressBar,
    done: Arc<AtomicBool>,
}

/// Build the display for a transfer.
///
/// - `Rich`: bytes bar + `N/M files` bar (+ transient directory bar).
/// - `Quiet`: one compact line.
/// - `Silent`: hidden.
#[must_use]
pub fn create_multi_progress(total_bytes: u64, total_files: u64, total_dirs: u64, mode: ProgressMode) -> MultiDisplay {
    let multi = MultiProgress::new();
    if mode == ProgressMode::Silent {
        multi.set_draw_target(ProgressDrawTarget::hidden());
    } else {
        multi.set_draw_target(ProgressDrawTarget::stderr_with_hz(10));
    }

    let dirs_bar = (mode == ProgressMode::Rich && total_dirs > 0).then(|| {
        let bar = multi.add(ProgressBar::new(total_dirs));
        bar.set_style(style("Dirs  ┃{bar:30.cyan}┃ {pos}/{len} directories"));
        bar
    });

    let total_bar = multi.add(ProgressBar::new(total_bytes));
    let files_bar = multi.add(ProgressBar::new(total_files));

    if mode == ProgressMode::Rich {
        total_bar.set_style(style(
            "Bytes ┃{bar:30.green}┃ {percent:>3}% │ {decimal_bytes}/{decimal_total_bytes} │ \
             {decimal_bytes_per_sec} │ ETA {eta}",
        ));
        files_bar.set_style(style("Files ┃{bar:30.green}┃ {pos}/{len}"));
    } else {
        total_bar.set_style(style(
            "{percent:>3}% │ {decimal_bytes}/{decimal_total_bytes} │ {decimal_bytes_per_sec} │ ETA {eta}",
        ));
        files_bar.set_draw_target(ProgressDrawTarget::hidden());
    }

    MultiDisplay { multi, dirs_bar, total_bar, files_bar, done: Arc::new(AtomicBool::new(false)) }
}

impl MultiDisplay {
    /// Spawn the thread that mirrors `counters` onto the bars every 100 ms.
    ///
    /// It runs until [`MultiDisplay::finish`] is called — never until the
    /// totals are reached, because failed or skipped files mean the totals
    /// may never be reached.
    #[must_use]
    pub fn spawn_ticker(&self, counters: CopyCounters) -> JoinHandle<()> {
        let total_bar = self.total_bar.clone();
        let files_bar = self.files_bar.clone();
        let dirs_bar = self.dirs_bar.clone();
        let done = Arc::clone(&self.done);
        std::thread::spawn(move || {
            let mut dirs_cleared = false;
            loop {
                // Check first: never redraw a bar that `finish` cleared.
                if done.load(Ordering::Acquire) {
                    break;
                }
                total_bar.set_position(counters.bytes_copied.load(Ordering::Relaxed));
                let files = counters.files_done.load(Ordering::Relaxed) + counters.files_failed.load(Ordering::Relaxed);
                files_bar.set_position(files);
                if let Some(bar) = dirs_bar.as_ref()
                    && !dirs_cleared
                {
                    let dirs = counters.dirs_done.load(Ordering::Relaxed);
                    bar.set_position(dirs);
                    if bar.length().is_some_and(|l| dirs >= l) {
                        bar.finish_and_clear();
                        dirs_cleared = true;
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })
    }

    /// Stop the ticker and clear the bars.
    pub fn finish(&self) {
        self.done.store(true, Ordering::Release);
        if let Some(bar) = &self.dirs_bar {
            bar.finish_and_clear();
        }
        self.total_bar.finish_and_clear();
        self.files_bar.finish_and_clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn ticker_stops_even_when_totals_are_never_reached() {
        // Regression: a failed file kept files_done < total forever and the
        // ticker (and the whole program) hung.
        let d = create_multi_progress(1000, 10, 0, ProgressMode::Silent);
        let counters = CopyCounters::new();
        counters.files_failed.store(1, Ordering::Relaxed);
        let h = d.spawn_ticker(counters);
        std::thread::sleep(Duration::from_millis(150));
        let t = Instant::now();
        d.finish();
        h.join().unwrap();
        assert!(t.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn ticker_mirrors_counters() {
        let d = create_multi_progress(1000, 10, 0, ProgressMode::Silent);
        let counters = CopyCounters::new();
        counters.bytes_copied.store(500, Ordering::Relaxed);
        counters.files_done.store(3, Ordering::Relaxed);
        counters.files_failed.store(1, Ordering::Relaxed);
        let h = d.spawn_ticker(counters);
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(d.total_bar.position(), 500);
        assert_eq!(d.files_bar.position(), 4, "failed files count as processed");
        d.finish();
        h.join().unwrap();
    }
}
