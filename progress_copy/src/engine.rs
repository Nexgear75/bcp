//! Phase 3 of the pipeline: execute a [`CopyPlan`].
//!
//! Every entry is first materialised under a temp name next to its
//! destination, then installed with an atomic rename (never replacing an
//! existing entry unless the plan allows it). Regular files run in parallel
//! on a rayon pool, largest first.

use std::collections::{BTreeSet, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use rayon::prelude::*;

use crate::atomic::{TempFile, TempPath};
use crate::config::{Config, SyncMode};
use crate::error::CopyError;
use crate::metadata::{self, Target};
use crate::signals::CancelState;
use crate::types::{CopyCounters, CopyPlan, CopyReport, FileEntry, FileJob, FileKind};

/// Result of [`execute_plan`].
#[derive(Debug, Default)]
pub struct ExecOutcome {
    pub report: CopyReport,
    /// Source paths whose entry was fully transferred (used by `bmv` to know
    /// exactly what it may delete).
    pub transferred: HashSet<PathBuf>,
}

/// Shared mutable state of one execution.
struct Shared<'a> {
    config: &'a Config,
    counters: &'a CopyCounters,
    cancel: &'a CancelState,
    clone_disabled: AtomicBool,
    errors: Mutex<Vec<String>>,
    warnings: Mutex<Vec<String>>,
    transferred: Mutex<HashSet<PathBuf>>,
    /// Regular files installed (for batch sync).
    written: Mutex<Vec<PathBuf>>,
    /// Every installed destination path (for directory fsync).
    installed: Mutex<Vec<PathBuf>>,
    fatal: Mutex<Option<String>>,
}

impl Shared<'_> {
    fn stopped(&self) -> bool {
        self.cancel.is_cancelled()
    }

    fn success(&self, entry: &FileEntry, dest: &Path, regular: bool) {
        self.transferred.lock().expect("lock").insert(entry.source_path.clone());
        self.installed.lock().expect("lock").push(dest.to_path_buf());
        if regular {
            self.written.lock().expect("lock").push(dest.to_path_buf());
        }
    }

    fn metadata_problems(&self, dest: &Path, problems: Vec<String>) {
        if problems.is_empty() {
            return;
        }
        let msg = format!("{}: cannot preserve {}", dest.display(), problems.join("; "));
        if self.config.strict {
            self.fail(msg);
        } else {
            self.warnings.lock().expect("lock").push(msg);
        }
    }

    fn fail(&self, msg: String) {
        if self.config.strict {
            let mut f = self.fatal.lock().expect("lock");
            if f.is_none() {
                *f = Some(msg.clone());
            }
            self.cancel.abort();
        }
        self.errors.lock().expect("lock").push(msg);
    }
}

/// Execute a plan.
///
/// `counters` is shared with the progress display; `cancel` is polled
/// between files and chunks.
///
/// # Errors
/// Only in `--strict` mode (first failure) or when the thread pool can't be
/// built. Otherwise failures are collected in the report.
pub fn execute_plan(plan: &CopyPlan, config: &Config, counters: &CopyCounters, cancel: &CancelState) -> anyhow::Result<ExecOutcome> {
    let sh = Shared {
        config,
        counters,
        cancel,
        clone_disabled: AtomicBool::new(false),
        errors: Mutex::new(Vec::new()),
        warnings: Mutex::new(Vec::new()),
        transferred: Mutex::new(HashSet::new()),
        written: Mutex::new(Vec::new()),
        installed: Mutex::new(Vec::new()),
        fatal: Mutex::new(None),
    };

    // Unsupported / unreadable entries are failures, not silent skips.
    for w in &plan.scan_warnings {
        counters.files_failed.fetch_add(1, Ordering::Relaxed);
        sh.fail(w.to_string());
    }

    // 1. Directories (parents first). Created writable; final mode later.
    for job in &plan.dirs {
        if sh.stopped() {
            break;
        }
        match create_dir(&job.dest, job.replace) {
            Ok(()) => {
                counters.dirs_done.fetch_add(1, Ordering::Relaxed);
                sh.success(&job.entry, &job.dest, false);
            }
            Err(e) => sh.fail(format!("cannot create directory '{}': {e}", job.dest.display())),
        }
    }
    let dirs_created = plan.dirs.iter().filter(|j| !j.replace).count() as u64;

    // 2. Regular files, in parallel.
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(config.thread_count())
        .build()
        .context("failed to build thread pool")?;
    pool.install(|| {
        plan.files.par_iter().with_max_len(1).for_each(|job| {
            if sh.stopped() {
                return;
            }
            run_file_job(&sh, job);
        });
    });

    // 3. Symlinks, FIFOs, hardlinks (hardlinks need their target copied).
    let mut symlinks = 0;
    let mut fifos = 0;
    let mut hardlinks = 0;
    for job in &plan.symlinks {
        if sh.stopped() {
            break;
        }
        match make_special(&job.entry, &job.dest, job.replace, config) {
            Ok(problems) => {
                symlinks += 1;
                sh.success(&job.entry, &job.dest, false);
                sh.metadata_problems(&job.dest, problems);
            }
            Err(e) => sh.fail(format!("cannot create symlink '{}': {e:#}", job.dest.display())),
        }
    }
    for job in &plan.fifos {
        if sh.stopped() {
            break;
        }
        match make_special(&job.entry, &job.dest, job.replace, config) {
            Ok(problems) => {
                fifos += 1;
                sh.success(&job.entry, &job.dest, false);
                sh.metadata_problems(&job.dest, problems);
            }
            Err(e) => sh.fail(format!("cannot create fifo '{}': {e:#}", job.dest.display())),
        }
    }
    let link_targets: HashSet<PathBuf> = if plan.hardlinks.is_empty() {
        HashSet::new()
    } else {
        sh.installed.lock().expect("lock").iter().cloned().collect()
    };
    for job in &plan.hardlinks {
        if sh.stopped() {
            break;
        }
        let target_ok = link_targets.contains(&job.target);
        let linked = target_ok && link_into_place(&job.target, &job.dest, job.replace, config).is_ok();
        if linked {
            hardlinks += 1;
            sh.success(&job.entry, &job.dest, false);
        } else {
            // Target missing or filesystem without hardlinks: plain copy.
            let fj = FileJob { entry: job.entry.clone(), dest: job.dest.clone(), clone: false, replace: job.replace };
            run_file_job(&sh, &fj);
        }
    }

    // 4. Directory metadata, deepest first (creating files inside a directory
    //    changes its mtime, and a read-only mode would block those writes).
    if !sh.stopped() {
        for job in plan.dirs.iter().rev() {
            if !job.replace || config.preserve.any() {
                let problems = metadata::apply(&job.entry.source_path, &job.entry.meta, Target::Path(&job.dest), config.preserve);
                sh.metadata_problems(&job.dest, problems);
            }
        }
    }

    // 5. Durability.
    let written = std::mem::take(&mut *sh.written.lock().expect("lock"));
    let installed = std::mem::take(&mut *sh.installed.lock().expect("lock"));
    if let Err(e) = flush(config.sync, &written, &installed) {
        sh.fail(format!("cannot flush data to disk: {e}"));
    }

    if let Some(msg) = sh.fatal.lock().expect("lock").take() {
        anyhow::bail!(msg);
    }

    let report = CopyReport {
        files_copied: counters.files_done.load(Ordering::Relaxed),
        files_failed: counters.files_failed.load(Ordering::Relaxed),
        files_skipped: plan.skipped.len() as u64,
        bytes_copied: counters.bytes_copied.load(Ordering::Relaxed),
        dirs_created,
        symlinks_created: symlinks,
        hardlinks_created: hardlinks,
        fifos_created: fifos,
        renamed: 0,
        cancelled: cancel.is_interrupted(),
        errors: sh.errors.into_inner().expect("lock"),
        warnings: sh.warnings.into_inner().expect("lock"),
    };
    Ok(ExecOutcome { report, transferred: sh.transferred.into_inner().expect("lock") })
}

fn run_file_job(sh: &Shared<'_>, job: &FileJob) {
    let r = transfer_file(job, sh.config, &sh.counters.bytes_copied, &sh.cancel.cancelled, &sh.clone_disabled);
    match r {
        Ok((problems, cloned)) => {
            sh.counters.files_done.fetch_add(1, Ordering::Relaxed);
            // A clone wrote no data: nothing to flush but its directory entry.
            sh.success(&job.entry, &job.dest, !cloned);
            sh.metadata_problems(&job.dest, problems);
        }
        Err(e) if is_cancel(&e) && sh.stopped() => {}
        Err(e) => {
            sh.counters.files_failed.fetch_add(1, Ordering::Relaxed);
            sh.fail(format!("cannot copy '{}' to '{}': {e:#}", job.entry.source_path.display(), job.dest.display()));
        }
    }
}

fn is_cancel(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        matches!(c.downcast_ref::<CopyError>(), Some(CopyError::Cancelled))
            || c.downcast_ref::<std::io::Error>().is_some_and(|io| io.kind() == std::io::ErrorKind::Interrupted)
    })
}

fn create_dir(dest: &Path, exists: bool) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    if exists {
        return Ok(());
    }
    // Owner-writable while we fill it; the final mode is applied at the end.
    let mut b = std::fs::DirBuilder::new();
    b.mode(0o700 | (0o777 & !crate::atomic::process_umask()));
    match b.create(dest) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && dest.is_dir() => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            b.recursive(true);
            b.create(dest)
        }
        Err(e) => Err(e),
    }
}

/// Transfer one regular file (clone if possible, else copy). Returns the
/// metadata problems (non-fatal) and whether it was cloned.
fn transfer_file(
    job: &FileJob,
    config: &Config,
    bytes: &Arc<AtomicU64>,
    cancelled: &AtomicBool,
    clone_disabled: &AtomicBool,
) -> anyhow::Result<(Vec<String>, bool)> {
    #[cfg(target_os = "macos")]
    if job.clone && !clone_disabled.load(Ordering::Relaxed) {
        match clone_into_place(job, config) {
            Ok(problems) => {
                bytes.fetch_add(job.entry.size(), Ordering::Relaxed);
                return Ok((problems, true));
            }
            Err(e) => {
                if let Some(io) = e.downcast_ref::<std::io::Error>()
                    && crate::platform::macos::clone_permanently_unsupported(io)
                {
                    clone_disabled.store(true, Ordering::Relaxed);
                }
                if e.downcast_ref::<CopyError>().is_some() {
                    return Err(e); // DestinationExists race: don't retry
                }
            }
        }
    }
    let _ = clone_disabled;
    copy_regular(&job.entry, &job.dest, job.replace, config, bytes, cancelled).map(|(_, p)| (p, false))
}

/// Copy a regular file through a temp file. Returns (bytes, metadata problems).
fn copy_regular(
    entry: &FileEntry,
    dest: &Path,
    replace: bool,
    config: &Config,
    bytes: &Arc<AtomicU64>,
    cancelled: &AtomicBool,
) -> anyhow::Result<(u64, Vec<String>)> {
    let mut src = File::open(&entry.source_path).with_context(|| format!("open '{}'", entry.source_path.display()))?;
    let mut tmp = TempFile::create_for(dest).with_context(|| format!("create temp file next to '{}'", dest.display()))?;

    let total = copy_data(&mut src, &mut tmp.file, entry, config, bytes, cancelled)?;

    let problems = metadata::apply(&entry.source_path, &entry.meta, Target::File(&tmp.file), config.preserve);
    if config.sync == SyncMode::Full {
        crate::atomic::sync_file(&tmp.file).context("fsync")?;
    }
    install(tmp, dest, replace, config)?;
    Ok((total, problems))
}

fn install(tmp: TempFile, dest: &Path, replace: bool, config: &Config) -> anyhow::Result<()> {
    tmp.install(dest, replace, config.backup_suffix.as_deref()).map_err(|e| map_install_err(e, dest))
}

fn map_install_err(e: std::io::Error, dest: &Path) -> anyhow::Error {
    if e.kind() == std::io::ErrorKind::AlreadyExists {
        // Appeared after planning: never overwrite it.
        CopyError::DestinationExists(dest.to_path_buf()).into()
    } else {
        anyhow::Error::new(e).context("rename into place")
    }
}

fn copy_data(
    src: &mut File,
    dst: &mut File,
    entry: &FileEntry,
    config: &Config,
    bytes: &Arc<AtomicU64>,
    cancelled: &AtomicBool,
) -> anyhow::Result<u64> {
    let size = entry.size();
    if entry.is_sparse()
        && let Some(n) = crate::copy::sparse_copy(src, dst, size, config.buffer_size, bytes, cancelled)?
    {
        return Ok(n);
    }

    #[allow(unused_mut)]
    let mut done: u64 = 0;
    #[cfg(target_os = "linux")]
    {
        use crate::platform::linux::{KernelCopy, advise_sequential, copy_file_range};
        if !config.no_kernel_copy {
            match copy_file_range(src, dst, size, bytes, cancelled) {
                Ok(KernelCopy::Done(n)) => return Ok(n),
                Ok(KernelCopy::Partial(n)) => done = n,
                Ok(KernelCopy::Unsupported) => {}
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => return Err(CopyError::Cancelled.into()),
                Err(e) => return Err(anyhow::Error::new(e).context("copy_file_range")),
            }
        }
        advise_sequential(src);
    }
    let rest = crate::copy::chunked_copy_with_progress(src, dst, size.saturating_sub(done), config.buffer_size, bytes, cancelled)?;
    Ok(done + rest)
}

/// macOS: clone to a temp name, fix metadata, rename into place.
#[cfg(target_os = "macos")]
fn clone_into_place(job: &FileJob, config: &Config) -> anyhow::Result<Vec<String>> {
    let tmp = TempPath::reserve(&job.dest);
    let follow = config.dereference != crate::config::Dereference::Never;
    crate::platform::macos::clonefile(&job.entry.source_path, tmp.path(), follow)?;
    let f = File::open(tmp.path())?;
    let problems = metadata::apply(&job.entry.source_path, &job.entry.meta, Target::File(&f), config.preserve);
    if config.sync == SyncMode::Full {
        crate::atomic::sync_file(&f)?;
    }
    drop(f);
    tmp.install(&job.dest, job.replace, config.backup_suffix.as_deref())
        .map_err(|e| map_install_err(e, &job.dest))?;
    Ok(problems)
}

/// Recreate a symlink or FIFO under a temp name, then install it.
fn make_special(entry: &FileEntry, dest: &Path, replace: bool, config: &Config) -> anyhow::Result<Vec<String>> {
    let tmp = TempPath::reserve(dest);
    let problems = match &entry.kind {
        FileKind::Symlink { target } => {
            std::os::unix::fs::symlink(target, tmp.path())?;
            metadata::apply(&entry.source_path, &entry.meta, Target::Symlink(tmp.path()), config.preserve)
        }
        FileKind::Fifo => {
            nix::unistd::mkfifo(tmp.path(), nix::sys::stat::Mode::from_bits_truncate(0o600))?;
            metadata::apply(&entry.source_path, &entry.meta, Target::Fifo(tmp.path()), config.preserve)
        }
        _ => anyhow::bail!("not a special file"),
    };
    tmp.install(dest, replace, config.backup_suffix.as_deref()).map_err(|e| map_install_err(e, dest))?;
    Ok(problems)
}

fn link_into_place(target: &Path, dest: &Path, replace: bool, config: &Config) -> anyhow::Result<()> {
    let tmp = TempPath::reserve(dest);
    std::fs::hard_link(target, tmp.path())?;
    tmp.install(dest, replace, config.backup_suffix.as_deref()).map_err(|e| map_install_err(e, dest))
}

/// Make the run durable according to `mode`.
fn flush(mode: SyncMode, written: &[PathBuf], installed: &[PathBuf]) -> std::io::Result<()> {
    if mode == SyncMode::None || installed.is_empty() {
        return Ok(());
    }
    if mode == SyncMode::Batch
        && let Some(first) = installed.first()
    {
        let dir = first.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        crate::atomic::sync_filesystem(dir, written)?;
    }
    // Make the new names durable: fsync every directory we added entries to.
    let dirs: BTreeSet<PathBuf> = installed
        .iter()
        .filter_map(|p| p.parent())
        .map(|p| if p.as_os_str().is_empty() { PathBuf::from(".") } else { p.to_path_buf() })
        .collect();
    dirs.par_iter().try_for_each(|d| crate::atomic::fsync_dir(d))
}

/// Copy one regular file atomically with full durability; refuses to replace
/// an existing destination. Used by [`crate::atomic::atomic_copy`].
///
/// # Errors
/// Open/read/write errors, `Cancelled`, or `DestinationExists`.
pub fn copy_single_file(
    src: &Path,
    dest: &Path,
    config: &Config,
    bytes: &Arc<AtomicU64>,
    cancelled: &AtomicBool,
) -> anyhow::Result<u64> {
    let meta = std::fs::metadata(src).map_err(|_| CopyError::SourceNotFound(src.to_path_buf()))?;
    let entry = FileEntry { source_path: src.to_path_buf(), relative_path: PathBuf::new(), meta, kind: FileKind::File };
    let (n, _) = copy_regular(&entry, dest, false, config, bytes, cancelled)?;
    if let Some(parent) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
        crate::atomic::fsync_dir(parent)?;
    }
    Ok(n)
}
