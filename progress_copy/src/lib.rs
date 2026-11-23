//! Engine behind `bcp` and `bmv`: scan → plan → execute, with atomic
//! installs, platform fast paths and a live progress display.

pub mod atomic;
pub mod cli;
pub mod config;
pub mod copy;
pub mod engine;
pub mod error;
pub mod metadata;
pub mod move_op;
pub mod plan;
pub mod platform;
pub mod progress;
pub mod scan;
pub mod signals;
pub mod types;
pub mod validate;

use std::path::{Path, PathBuf};

pub use config::{Config, Dereference, Overwrite, Preserve, ProgressMode, SyncMode};
pub use error::CopyError;
pub use types::CopyReport;

/// Copy one or more sources into `dest` (scan → plan → execute).
///
/// Per-file failures are collected in the returned report (see
/// [`CopyReport::exit_code`]); the library never exits the process.
///
/// # Errors
/// Invalid options, missing sources, refused overwrites, self-copy, not
/// enough space, or the first failure in `--strict` mode.
pub fn run_copy(sources: &[PathBuf], dest: &Path, config: &Config) -> anyhow::Result<CopyReport> {
    validate::validate_flags(config)?;
    let cancel = signals::install_sigint_handler()?;
    // Read the umask once, before any worker thread exists.
    let _ = atomic::process_umask();

    let scanned = scan::scan_sources(sources, dest, config)?;
    let spinner_mode = if config.overwrite == Overwrite::Prompt { ProgressMode::Silent } else { config.progress };
    let spinner = progress::phase_spinner("Planning…", spinner_mode);
    let plan = plan::build_plan(scanned, dest, config, "bcp");
    spinner.finish_and_clear();
    let plan = plan?;

    if config.dry_run {
        print_dry_run(&plan, config);
        return Ok(CopyReport { files_skipped: plan.skipped.len() as u64, ..Default::default() });
    }

    let counters = types::CopyCounters::new();
    let display =
        progress::create_multi_progress(plan.total_bytes, plan.total_files, plan.dirs.len() as u64, config.progress);
    let ticker = display.spawn_ticker(counters.clone());
    let result = engine::execute_plan(&plan, config, &counters, &cancel);
    display.finish();
    let _ = ticker.join();
    Ok(result?.report)
}

/// Move one or more sources to `dest`. See [`move_op`].
///
/// # Errors
/// Invalid options or target operand.
pub fn run_move(sources: &[PathBuf], dest: &Path, config: &Config) -> anyhow::Result<CopyReport> {
    move_op::run_move(sources, dest, config)
}

fn print_dry_run(plan: &types::CopyPlan, config: &Config) {
    let clones = plan.files.iter().filter(|j| j.clone).count();
    eprintln!("Dry run — nothing will be written:");
    eprintln!("  directories : {}", plan.dirs.iter().filter(|d| !d.replace).count());
    eprintln!("  files       : {} ({} as copy-on-write clones)", plan.files.len(), clones);
    eprintln!("  symlinks    : {}", plan.symlinks.len());
    eprintln!("  hardlinks   : {}", plan.hardlinks.len());
    eprintln!("  fifos       : {}", plan.fifos.len());
    eprintln!("  skipped     : {}", plan.skipped.len());
    eprintln!("  total       : {}", bytesize::ByteSize(plan.total_bytes));
    if config.verbose {
        for j in &plan.files {
            eprintln!("  '{}' -> '{}'", j.entry.source_path.display(), j.dest.display());
        }
    }
    for w in &plan.scan_warnings {
        eprintln!("  warning: {w}");
    }
}
