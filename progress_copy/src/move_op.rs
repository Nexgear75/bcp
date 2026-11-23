//! Move logic for `bmv`: instant rename on the same device, safe
//! copy-then-delete across devices.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::Context;

use crate::config::{Config, ProgressMode};
use crate::error::CopyError;
use crate::types::{CopyCounters, CopyReport};
use crate::{engine, plan, progress, scan, signals, validate};

/// Move one or more sources to `dest`.
pub fn run_move(sources: &[PathBuf], dest: &Path, config: &Config) -> anyhow::Result<CopyReport> {
    validate::validate_flags(config)?;
    let cancel = signals::install_sigint_handler().context("Failed to install signal handler")?;

    if sources.is_empty() {
        anyhow::bail!("no source paths given");
    }
    let multiple = sources.len() > 1;
    let dest_dir = match std::fs::metadata(dest) {
        Ok(m) => m.is_dir(),
        Err(_) => multiple,
    };
    if multiple && !dest_dir {
        anyhow::bail!("target '{}' is not a directory", dest.display());
    }

    let dest_device = dest_device_id(dest);
    let mut report = CopyReport::default();

    for source in sources {
        let meta = std::fs::symlink_metadata(source)
            .map_err(|_| CopyError::SourceNotFound(source.clone()))
            .with_context(|| format!("Source not found: {}", source.display()))?;

        let name = source.file_name().unwrap_or(source.as_os_str());
        let dest_path = if dest_dir { dest.join(name) } else { dest.to_path_buf() };

        // Self-move guard.
        if same_file_guarded(source, &dest_path) {
            anyhow::bail!("'{}' and '{}' are the same file", source.display(), dest_path.display());
        }
        // Overwrite policy.
        check_overwrite(&dest_path, config)?;

        if Some(meta.dev()) == dest_device {
            // Same device → atomic, instant rename (files and whole trees).
            std::fs::rename(source, &dest_path)
                .with_context(|| format!("rename {} -> {}", source.display(), dest_path.display()))?;
            report.files_copied += 1;
            if config.verbose && config.progress != ProgressMode::Silent {
                eprintln!("Moved {} (same device)", dest_path.display());
            }
        } else {
            // Cross-device → copy with progress, verify, then delete source.
            cross_device_move(source, &dest_path, config, &cancel, &mut report)?;
        }
    }

    if report.cancelled {
        eprintln!("Interrupted. Move stopped; sources left intact.");
        std::process::exit(130);
    }
    Ok(report)
}

fn cross_device_move(
    source: &Path,
    dest_path: &Path,
    config: &Config,
    cancel: &Arc<signals::CancelState>,
    report: &mut CopyReport,
) -> anyhow::Result<()> {
    let scan = scan::scan_one(source, dest_path, config)?;
    let planning = progress::phase_spinner("Planning… (analyse des entrées)", config.progress);
    let sub = plan::build_plan(scan, dest_path, config)?;
    planning.finish_and_clear();

    let counters = CopyCounters::new();
    let display = progress::create_multi_progress(
        sub.total_bytes,
        sub.total_files,
        sub.dirs_to_create.len() as u64,
        config.progress,
    );
    let ticker = display.spawn_ticker(counters.clone(), Arc::clone(cancel));

    let result = engine::execute_plan(&sub, config, &counters, cancel);

    display.finish();
    let _ = ticker.join();
    let sub_report = result?;

    report.files_copied += sub_report.files_copied;
    report.files_failed += sub_report.files_failed;
    report.bytes_copied += sub_report.bytes_copied;
    report.dirs_created += sub_report.dirs_created;
    report.symlinks_created += sub_report.symlinks_created;
    report.errors.extend(sub_report.errors.iter().cloned());

    if cancel.cancelled.load(Ordering::Relaxed) {
        report.cancelled = true;
        return Ok(()); // leave source intact
    }

    // Only delete the source once the copy fully succeeded (anti data-loss).
    if sub_report.files_failed == 0 {
        if source.is_dir() {
            std::fs::remove_dir_all(source)
                .with_context(|| format!("remove source dir {}", source.display()))?;
        } else {
            std::fs::remove_file(source)
                .with_context(|| format!("remove source file {}", source.display()))?;
        }
        if config.verbose && config.progress != ProgressMode::Silent {
            eprintln!("Moved {} (cross-device)", dest_path.display());
        }
    } else {
        anyhow::bail!(
            "{} file(s) failed to copy; source '{}' left intact",
            sub_report.files_failed,
            source.display()
        );
    }
    Ok(())
}

fn same_file_guarded(src: &Path, dest: &Path) -> bool {
    same_file::is_same_file(src, dest).unwrap_or(false)
}

fn check_overwrite(dest: &Path, config: &Config) -> anyhow::Result<()> {
    // `symlink_metadata` ne suit pas les symlinks : on teste l'entrée de
    // destination, pas la cible d'un lien (cf. plan.rs::check_overwrite_meta).
    if std::fs::symlink_metadata(dest).is_ok() && (config.no_clobber || !config.force) {
        return Err(CopyError::DestinationExists(dest.to_path_buf()).into());
    }
    Ok(())
}

fn dest_device_id(dest: &Path) -> Option<u64> {
    let mut p = dest;
    loop {
        if let Ok(m) = std::fs::metadata(p) {
            return Some(m.dev());
        }
        match p.parent() {
            Some(parent) if parent.as_os_str().is_empty() => {
                return std::fs::metadata(".").ok().map(|m| m.dev());
            }
            Some(parent) => p = parent,
            None => return std::fs::metadata(".").ok().map(|m| m.dev()),
        }
    }
}
