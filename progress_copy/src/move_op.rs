//! Move logic for `bmv`: atomic rename when possible, otherwise a verified
//! copy followed by deletion of **exactly** what was transferred.
//!
//! Safety rules for the cross-filesystem path:
//! - the source is deleted only after the copy is complete and flushed to
//!   disk (`--sync` is raised to at least `batch`);
//! - if anything in a source tree failed, was skipped (sockets, devices,
//!   unreadable entries) or changed since the scan, **nothing** of that
//!   source is deleted;
//! - deletion walks the scanned entries (files first, then directories
//!   deepest first with `rmdir`), so files created in the source during the
//!   copy are never deleted.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::config::{Config, ProgressMode, SyncMode};
use crate::engine::ExecOutcome;
use crate::error::CopyError;
use crate::plan::Decision;
use crate::signals::CancelState;
use crate::types::{CopyCounters, CopyPlan, CopyReport, FileEntry, FileKind};
use crate::{engine, plan, progress, scan, signals, validate};

const TOOL: &str = "bmv";

/// Move one or more sources to `dest`.
///
/// Per-source failures are collected in the report (like `mv`, the other
/// sources are still processed).
///
/// # Errors
/// Invalid options or an invalid target operand.
pub fn run_move(sources: &[PathBuf], dest: &Path, config: &Config) -> anyhow::Result<CopyReport> {
    validate::validate_flags(config)?;
    if sources.is_empty() {
        anyhow::bail!("missing file operand");
    }
    let cancel = signals::install_sigint_handler()?;
    let dest_dir = scan::resolve_dest_is_dir(dest, sources.len(), config)?;
    let mut report = CopyReport::default();

    for source in sources {
        if cancel.is_cancelled() {
            break;
        }
        let name = source.file_name().unwrap_or(source.as_os_str());
        let dest_path = if dest_dir { dest.join(name) } else { dest.to_path_buf() };
        match move_one(source, &dest_path, config, &cancel) {
            Ok(r) => report.absorb(r),
            Err(e) => {
                report.errors.push(format!("cannot move '{}' to '{}': {e:#}", source.display(), dest_path.display()))
            }
        }
    }
    report.cancelled |= cancel.is_interrupted();
    Ok(report)
}

fn move_one(source: &Path, dest_path: &Path, config: &Config, cancel: &CancelState) -> anyhow::Result<CopyReport> {
    let meta = std::fs::symlink_metadata(source).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::Error::new(CopyError::SourceNotFound(source.to_path_buf()))
        } else {
            anyhow::Error::new(e)
        }
    })?;
    let kind = if meta.is_dir() { FileKind::Directory } else { FileKind::File };
    let entry = FileEntry { source_path: source.to_path_buf(), relative_path: PathBuf::new(), meta, kind };

    // lstat comparison: never opens anything (opening a FIFO would block).
    let same =
        std::fs::symlink_metadata(dest_path).is_ok_and(|d| d.dev() == entry.device_id() && d.ino() == entry.inode());
    if same {
        anyhow::bail!("'{}' and '{}' are the same file", source.display(), dest_path.display());
    }
    if entry.kind == FileKind::Directory {
        scan::check_not_into_itself(source, dest_path)?;
    }

    let dest_meta = std::fs::symlink_metadata(dest_path).ok();
    if entry.kind == FileKind::Directory
        && let Some(dm) = &dest_meta
        && !dm.is_dir()
    {
        anyhow::bail!("cannot overwrite non-directory '{}' with directory", dest_path.display());
    }
    let replace = match plan::decide(&entry, dest_path, dest_meta.as_ref(), config, TOOL)? {
        Decision::Skip => return Ok(CopyReport { files_skipped: 1, ..Default::default() }),
        Decision::Proceed { replace } => replace,
    };

    if config.dry_run {
        print_dry_run(source, dest_path, &entry);
        return Ok(CopyReport::default());
    }

    // 1. Same filesystem: one atomic rename (also for whole trees).
    match crate::atomic::install(source, dest_path, replace, config.backup_suffix.as_deref()) {
        Ok(()) => {
            if config.verbose && config.progress != ProgressMode::Silent {
                eprintln!("renamed '{}' -> '{}'", source.display(), dest_path.display());
            }
            return Ok(CopyReport { renamed: 1, ..Default::default() });
        }
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(CopyError::DestinationExists(dest_path.to_path_buf()).into());
        }
        Err(e) => return Err(anyhow::Error::new(e).context("rename")),
    }

    // 2. Different filesystems: copy, verify, delete.
    if entry.kind == FileKind::Directory
        && dest_meta.is_some()
        && std::fs::read_dir(dest_path).map(|mut d| d.next().is_some()).unwrap_or(true)
    {
        anyhow::bail!("'{}': Directory not empty", dest_path.display());
    }
    cross_device_move(source, dest_path, config, cancel)
}

fn print_dry_run(source: &Path, dest_path: &Path, entry: &FileEntry) {
    let dest_dev =
        std::fs::metadata(plan::nearest_existing(dest_path.parent().unwrap_or(Path::new(".")))).map(|m| m.dev());
    let how = if dest_dev.ok() == Some(entry.device_id()) { "rename" } else { "copy + delete (different filesystem)" };
    eprintln!("would move '{}' -> '{}' [{how}]", source.display(), dest_path.display());
}

fn cross_device_move(
    source: &Path,
    dest_path: &Path,
    config: &Config,
    cancel: &CancelState,
) -> anyhow::Result<CopyReport> {
    let mut cfg = config.clone();
    cfg.recursive = true;
    if cfg.sync == SyncMode::None {
        cfg.sync = SyncMode::Batch; // the source is about to disappear
    }
    // Replacing a top-level entry was already approved by `decide`.
    if cfg.overwrite == crate::config::Overwrite::Refuse && std::fs::symlink_metadata(dest_path).is_ok() {
        cfg.overwrite = crate::config::Overwrite::Force;
    }

    let scanned = scan::scan_one(source, dest_path, &cfg)?;
    let spinner = progress::phase_spinner("Planning…", cfg.progress);
    let plan = plan::build_plan(scanned, dest_path, &cfg, TOOL);
    spinner.finish_and_clear();
    let plan = plan?;

    let counters = CopyCounters::new();
    let display =
        progress::create_multi_progress(plan.total_bytes, plan.total_files, plan.dirs.len() as u64, cfg.progress);
    let ticker = display.spawn_ticker(counters.clone());
    let result = engine::execute_plan(&plan, &cfg, &counters, cancel);
    display.finish();
    let _ = ticker.join();
    let ExecOutcome { mut report, transferred } = result?;

    if cancel.is_cancelled() {
        report.warnings.push(format!("move interrupted: source '{}' left intact", source.display()));
        return Ok(report);
    }
    if !report.is_success() || !plan.skipped.is_empty() {
        report.errors.push(format!("some entries were not transferred; source '{}' left intact", source.display()));
        return Ok(report);
    }
    if let Some(problem) = verify(&plan, &transferred) {
        report.errors.push(format!("{problem}; source '{}' left intact", source.display()));
        return Ok(report);
    }

    delete_sources(&plan, &mut report);
    if config.verbose && config.progress != ProgressMode::Silent {
        eprintln!("moved '{}' -> '{}' (copied across filesystems)", source.display(), dest_path.display());
    }
    Ok(report)
}

/// Every scanned entry must have been transferred, and no source file may
/// have changed since the scan (otherwise the copy could be stale).
fn verify(plan: &CopyPlan, transferred: &std::collections::HashSet<PathBuf>) -> Option<String> {
    let entries = plan
        .files
        .iter()
        .map(|j| &j.entry)
        .chain(plan.hardlinks.iter().map(|j| &j.entry))
        .chain(plan.symlinks.iter().map(|j| &j.entry))
        .chain(plan.fifos.iter().map(|j| &j.entry))
        .chain(plan.dirs.iter().map(|j| &j.entry));
    for e in entries {
        if !transferred.contains(&e.source_path) {
            return Some(format!("'{}' was not transferred", e.source_path.display()));
        }
        if e.kind == FileKind::File {
            match std::fs::symlink_metadata(&e.source_path) {
                Ok(now)
                    if now.len() == e.meta.len()
                        && now.mtime() == e.meta.mtime()
                        && now.mtime_nsec() == e.meta.mtime_nsec() => {}
                _ => return Some(format!("'{}' changed during the move", e.source_path.display())),
            }
        }
    }
    None
}

fn delete_sources(plan: &CopyPlan, report: &mut CopyReport) {
    let non_dirs = plan
        .files
        .iter()
        .map(|j| &j.entry.source_path)
        .chain(plan.hardlinks.iter().map(|j| &j.entry.source_path))
        .chain(plan.symlinks.iter().map(|j| &j.entry.source_path))
        .chain(plan.fifos.iter().map(|j| &j.entry.source_path));
    for p in non_dirs {
        if let Err(e) = std::fs::remove_file(p) {
            report.errors.push(format!("copied but cannot remove source '{}': {e}", p.display()));
        }
    }
    // Directories deepest first; `remove_dir` refuses non-empty ones, so
    // anything that appeared during the copy survives.
    let mut dirs: Vec<&Path> = plan.dirs.iter().map(|j| j.entry.source_path.as_path()).collect();
    dirs.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for d in dirs {
        if let Err(e) = std::fs::remove_dir(d) {
            let hint = if e.raw_os_error() == Some(libc::ENOTEMPTY) {
                " (new entries appeared during the move — left in place)"
            } else {
                ""
            };
            report.errors.push(format!("cannot remove source directory '{}': {e}{hint}", d.display()));
        }
    }
}
