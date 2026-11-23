//! Phase 3 of the pipeline: execute a [`CopyPlan`] in parallel, dispatching
//! each file to the fastest available strategy and updating shared counters.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use rayon::prelude::*;
use tempfile::Builder;

use crate::config::Config;
#[allow(unused_imports)]
use crate::error::CopyError;
use crate::signals::CancelState;
use crate::types::{CopyCounters, CopyPlan, CopyReport, FileEntry, FileKind};

/// Execute a plan, returning a [`CopyReport`].
///
/// `counters` is shared with the progress display thread; `cancel` is the
/// SIGINT state polled between files and chunks.
pub fn execute_plan(
    plan: &CopyPlan,
    config: &Config,
    counters: &CopyCounters,
    cancel: &Arc<CancelState>,
) -> anyhow::Result<CopyReport> {
    let mut report = CopyReport::default();
    let errors: Mutex<Vec<String>> = Mutex::new(Vec::new());

    // 1. Create destination directories first (sequential, ordered).
    // `dirs_to_create` is in walk order (parents before children), so a plain
    // `create_dir` succeeds in the common case; only fall back to the costlier
    // `create_dir_all` (which re-stats every ancestor) when a parent is missing.
    for dir in &plan.dirs_to_create {
        let created = match std::fs::create_dir(dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir_all(dir),
            Err(e) => Err(e),
        };
        if let Err(e) = created {
            if config.strict {
                return Err(anyhow::anyhow!("create dir {}: {e}", dir.display()));
            }
            errors.lock().unwrap().push(format!("create dir {}: {e}", dir.display()));
        } else {
            report.dirs_created += 1;
            counters.dirs_done.fetch_add(1, Ordering::Relaxed);
        }
    }

    // 2. Recreate symlinks.
    for (entry, dest) in &plan.symlinks {
        if let FileKind::Symlink { target } = &entry.kind {
            let _ = std::fs::remove_file(dest); // overwrite if present
            if let Err(e) = std::os::unix::fs::symlink(target, dest) {
                if config.strict {
                    return Err(anyhow::anyhow!("symlink {}: {e}", dest.display()));
                }
                errors.lock().unwrap().push(format!("symlink {}: {e}", dest.display()));
            } else {
                report.symlinks_created += 1;
            }
        }
    }

    // 3. Clones (macOS APFS) — near-instant, run sequentially.
    #[cfg(target_os = "macos")]
    for (entry, dest) in &plan.files_to_clone {
        if cancel.cancelled.load(Ordering::Relaxed) {
            break;
        }
        match clone_file_atomic(entry, dest, config) {
            Ok(()) => {
                counters.files_done.fetch_add(1, Ordering::Relaxed);
                counters.bytes_copied.fetch_add(entry.size, Ordering::Relaxed);
            }
            Err(e) => {
                // Fall back to a regular copy on clone failure.
                match copy_file_atomic(entry, dest, config, &counters.bytes_copied, &cancel.cancelled) {
                    Ok(()) => {
                        counters.files_done.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e2) => {
                        counters.files_failed.fetch_add(1, Ordering::Relaxed);
                        handle_file_error(&errors, config, cancel, dest, &format!("{e} / fallback: {e2}"))?;
                    }
                }
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    debug_assert!(plan.files_to_clone.is_empty());

    // 4. Parallel copies.
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(config.thread_count())
        .build()
        .context("Failed to build thread pool")?;

    pool.install(|| {
        plan.files_to_copy.par_iter().for_each(|(entry, dest)| {
            if cancel.cancelled.load(Ordering::Relaxed) {
                return;
            }
            match copy_file_atomic(entry, dest, config, &counters.bytes_copied, &cancel.cancelled) {
                Ok(()) => {
                    counters.files_done.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    counters.files_failed.fetch_add(1, Ordering::Relaxed);
                    let _ = handle_file_error(&errors, config, cancel, dest, &e.to_string());
                }
            }
        });
    });

    report.cancelled = cancel.cancelled.load(Ordering::Relaxed);
    report.files_copied = counters.files_done.load(Ordering::Relaxed);
    report.files_failed = counters.files_failed.load(Ordering::Relaxed);
    report.bytes_copied = counters.bytes_copied.load(Ordering::Relaxed);
    report.errors = errors.into_inner().unwrap();

    Ok(report)
}

/// Record a per-file error; in strict mode signal cancellation so other
/// workers stop, and propagate the error.
fn handle_file_error(
    errors: &Mutex<Vec<String>>,
    config: &Config,
    cancel: &Arc<CancelState>,
    dest: &Path,
    msg: &str,
) -> anyhow::Result<()> {
    let full = format!("{}: {msg}", dest.display());
    errors.lock().unwrap().push(full.clone());
    if config.strict {
        cancel.cancelled.store(true, Ordering::Relaxed);
        return Err(anyhow::anyhow!(full));
    }
    Ok(())
}

/// Copy a single regular file atomically: temp → strategy copy → fsync →
/// metadata → rename.
pub fn copy_file_atomic(
    entry: &FileEntry,
    dest: &Path,
    config: &Config,
    bytes_copied: &Arc<AtomicU64>,
    cancelled: &AtomicBool,
) -> anyhow::Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create parent dir for {}", dest.display()))?;
    }

    let dest_dir = dest.parent().unwrap_or(Path::new("."));
    let prefix = format!(
        ".{}.",
        dest.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
    );
    let tmp = Builder::new()
        .prefix(&prefix)
        .tempfile_in(dest_dir)
        .context("Failed to create temporary file")?;

    let tmp_path = tmp.path().to_path_buf();
    let mut tmp_file = fs_err::OpenOptions::new()
        .write(true)
        .open(&tmp_path)
        .context("Failed to open temp file for writing")?;

    let mut src_file = fs_err::File::open(&entry.source_path)
        .with_context(|| format!("open source {}", entry.source_path.display()))?;

    copy_data(&mut src_file, &mut tmp_file, entry.size, config, bytes_copied, cancelled)?;

    drop(tmp_file);
    crate::atomic::sync_file(tmp.as_file()).context("fsync temp file")?;

    // Apply metadata onto the temp file before the rename (atomic visibility).
    if config.preserve.any()
        && let Err(e) = crate::metadata::apply(&entry.source_path, &tmp_path, config.preserve)
            && config.strict {
                anyhow::bail!("metadata: {e}");
            }

    tmp.persist(dest).map_err(|e| anyhow::anyhow!("rename into place: {}", e))?;
    Ok(())
}

/// Copy file bytes using the best available strategy.
#[allow(unused_variables)]
fn copy_data(
    src: &mut fs_err::File,
    dst: &mut fs_err::File,
    size: u64,
    config: &Config,
    bytes_copied: &Arc<AtomicU64>,
    cancelled: &AtomicBool,
) -> anyhow::Result<u64> {
    #[cfg(target_os = "linux")]
    if !config.no_kernel_copy {
        match crate::platform::linux::copy_file_range(&*src, &*dst, size, bytes_copied, cancelled) {
            Ok(Some(total)) => return Ok(total),
            Ok(None) => { /* unsupported → fall back to chunked */ }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                return Err(CopyError::Cancelled.into());
            }
            Err(e) => return Err(e.into()),
        }
    }

    let total = crate::copy::chunked_copy_with_progress(
        src, dst, size, config.buffer_size, bytes_copied, cancelled,
    )
    .map_err(anyhow::Error::new)?;
    Ok(total)
}

/// Clone a single file atomically (macOS): clonefile → temp → rename.
#[cfg(target_os = "macos")]
fn clone_file_atomic(entry: &FileEntry, dest: &Path, _config: &Config) -> anyhow::Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create parent dir for {}", dest.display()))?;
    }
    let dest_dir = dest.parent().unwrap_or(Path::new("."));
    // clonefile requires the target not to exist; use a unique temp name.
    let tmp_name = format!(
        ".{}.clone.{}",
        dest.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
        std::process::id()
    );
    let tmp_path = dest_dir.join(tmp_name);
    let _ = std::fs::remove_file(&tmp_path);

    crate::platform::macos::clonefile(&entry.source_path, &tmp_path)
        .with_context(|| format!("clonefile {}", entry.source_path.display()))?;

    // Apply metadata + atomic rename into place.
    if _config.preserve.any() {
        let _ = crate::metadata::apply(&entry.source_path, &tmp_path, _config.preserve);
    }
    std::fs::rename(&tmp_path, dest)
        .with_context(|| format!("rename clone into {}", dest.display()))?;
    Ok(())
}
