#![allow(dead_code)]

pub mod config;
pub mod error;
pub mod types;
pub mod copy;
pub mod atomic;
pub mod progress;
pub mod signals;
pub mod validate;
pub mod scan;
pub mod plan;
pub mod engine;
pub mod metadata;
pub mod platform;
pub mod move_op;

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::Context;

pub use config::{Config, Preserve, ProgressMode};
pub use error::CopyError;
pub use types::CopyReport;

/// Top-level entry point for a copy of one or more sources into `dest`.
///
/// Runs the full Scan → Plan → Execute pipeline with a live multi-bar
/// display, SIGINT handling, and (optionally) a dry-run preview.
pub fn run_copy(sources: &[std::path::PathBuf], dest: &Path, config: &Config) -> anyhow::Result<CopyReport> {
    validate::validate_flags(config)?;

    let cancel = signals::install_sigint_handler()
        .context("Failed to install signal handler")?;

    // Phase 1: scan.
    let scan = scan::scan_sources(sources, dest, config)?;
    // Phase 2: plan.
    let planning = progress::phase_spinner("Planning… (analyse des entrées)", config.progress);
    let plan = plan::build_plan(scan, dest, config)?;
    planning.finish_and_clear();

    if config.dry_run {
        print_dry_run(&plan, config);
        return Ok(CopyReport::default());
    }

    // Phase 3: execute with progress display.
    let counters = types::CopyCounters::new();
    let display = progress::create_multi_progress(
        plan.total_bytes,
        plan.total_files,
        plan.dirs_to_create.len() as u64,
        config.progress,
    );
    let ticker = display.spawn_ticker(counters.clone(), Arc::clone(&cancel));

    let report = engine::execute_plan(&plan, config, &counters, &cancel);

    display.finish();
    let _ = ticker.join();

    let report = report?;
    report_outcome(&report, &plan, config);
    Ok(report)
}

/// Top-level entry point for a move (rename or copy+delete). See [`move_op`].
pub fn run_move(sources: &[std::path::PathBuf], dest: &Path, config: &Config) -> anyhow::Result<CopyReport> {
    move_op::run_move(sources, dest, config)
}

fn print_dry_run(plan: &types::CopyPlan, config: &Config) {
    if config.progress == ProgressMode::Silent {
        return;
    }
    eprintln!("Dry run — no files will be written:");
    eprintln!("  directories : {}", plan.dirs_to_create.len());
    eprintln!("  symlinks    : {}", plan.symlinks.len());
    eprintln!("  files (copy): {}", plan.files_to_copy.len());
    eprintln!("  files (clone): {}", plan.files_to_clone.len());
    eprintln!(
        "  total       : {} across {} files",
        bytesize::ByteSize(plan.total_bytes),
        plan.total_files
    );
    for w in &plan.scan_warnings {
        eprintln!("  warning: {w:?}");
    }
}

fn report_outcome(report: &CopyReport, plan: &types::CopyPlan, config: &Config) {
    if report.cancelled {
        eprintln!(
            "Interrupted. {} files copied, temp files cleaned up.",
            report.files_copied
        );
        std::process::exit(130);
    }
    for w in &plan.scan_warnings {
        eprintln!("warning: {w:?}");
    }
    for e in &report.errors {
        eprintln!("error: {e}");
    }
    if config.verbose && config.progress != ProgressMode::Silent {
        eprintln!(
            "Copied {} files ({}), {} dirs, {} symlinks{}.",
            report.files_copied,
            bytesize::ByteSize(report.bytes_copied),
            report.dirs_created,
            report.symlinks_created,
            if report.files_failed > 0 {
                format!(", {} failed", report.files_failed)
            } else {
                String::new()
            }
        );
    }
}

/// Main entry point for a copy operation.
///
/// Orchestrates the full copy pipeline:
/// 1. Pre-flight validation (source exists, not self-copy, dest writable)
/// 2. SIGINT handler installation (two-stage Ctrl+C)
/// 3. Progress bar creation and ticker thread
/// 4. Atomic copy (temp file → chunked copy → F_FULLFSYNC → persist)
/// 5. Cleanup and exit code mapping
pub fn run(config: Config) -> anyhow::Result<()> {
    // 1. Pre-flight validation (PLAN-06/07/08, ATOM-07)
    validate::run_preflight(&config)
        .context("Pre-flight check failed")?;

    // 2. Get source file size for progress bar
    let file_size = fs_err::metadata(&config.source)
        .context("Failed to read source metadata")?
        .len();

    // 3. Install SIGINT handler (ATOM-05/06, D-10/11)
    let cancel = signals::install_sigint_handler()
        .context("Failed to install signal handler")?;

    // 4. Set up shared progress counter (PROG-05, D-06)
    let bytes_copied = Arc::new(AtomicU64::new(0));
    let bytes_copied_for_ticker = Arc::clone(&bytes_copied);

    // 5. Create progress bar (PROG-02/03/04, D-01/02/03)
    let source_name = config
        .source
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| config.source.to_string_lossy().to_string());
    let bar = progress::create_progress_bar(file_size, &source_name);

    // 6. Spawn ticker thread (reads AtomicU64 every 100ms)
    let ticker = progress::spawn_progress_ticker(
        bar.clone(),
        bytes_copied_for_ticker,
        Arc::clone(&cancel),
    );

    // 7. Perform atomic copy (COPY-03, ATOM-01/02/04)
    let result = atomic::atomic_copy(
        &config.source,
        &config.destination,
        config.buffer_size,
        &bytes_copied,
        &cancel.cancelled,
    );

    // 8. Join ticker thread
    let _ = ticker.join();

    // 9. Handle result
    match result {
        Ok(total) => {
            if cancel.cancelled.load(Ordering::Relaxed) {
                bar.finish_with_message("Copy cancelled");
                eprintln!("Copy cancelled — temp files cleaned up.");
                std::process::exit(130); // D-09: SIGINT exit code
            }
            if config.verbose {
                eprintln!("Copied: {} ({} bytes)", source_name, total);
            } else {
                bar.finish_and_clear();
            }
            Ok(())
        }
        Err(e) => {
            bar.finish_and_clear();
            // Check if the error wraps a CopyError::Cancelled
            if let Some(copy_err) = e.downcast_ref::<error::CopyError>()
                && matches!(copy_err, error::CopyError::Cancelled) {
                    eprintln!("Copy cancelled — temp files cleaned up.");
                    std::process::exit(130);
                }
            Err(e.context("Copy failed"))
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_run_config_roundtrip() {
        // Test 12: run() orchestrates correctly — validates
        // that run() accepts a Config, runs preflight, and copies.
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        std::fs::write(&src, b"hello world").unwrap();

        let config = Config {
            source: src.clone(),
            destination: dst.clone(),
            ..Default::default()
        };

        let result = run(config);
        assert!(result.is_ok(), "run() should succeed: {:?}", result);

        // Destination should exist with matching content
        assert!(dst.exists(), "Destination should exist after run()");
        let dst_content = std::fs::read(&dst).unwrap();
        let src_content = std::fs::read(&src).unwrap();
        assert_eq!(dst_content, src_content);
    }

    #[test]
    fn test_run_self_copy_refused() {
        // run() should refuse self-copy
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("file.bin");
        std::fs::write(&file, b"test").unwrap();

        let config = Config {
            source: file.clone(),
            destination: file.clone(),
            ..Default::default()
        };

        let result = run(config);
        assert!(result.is_err(), "run() should refuse self-copy");
    }

    #[test]
    fn test_run_conflicting_flags() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        std::fs::write(&src, b"test").unwrap();

        let config = Config {
            source: src,
            destination: dst,
            force: true,
            no_clobber: true,
            ..Default::default()
        };

        let result = run(config);
        assert!(result.is_err(), "run() should reject conflicting flags");
    }
}
