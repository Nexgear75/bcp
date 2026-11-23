//! Phase 2 of the pipeline: turn scanned entries into an executable
//! [`CopyPlan`]: self-copy detection, overwrite policy, hardlink grouping,
//! clone eligibility, and a disk-space check.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::config::{Config, Overwrite};
use crate::error::CopyError;
use crate::scan::ScanResult;
use crate::types::{CopyPlan, EntryJob, FileEntry, FileJob, FileKind, LinkJob};

/// Outcome of the overwrite policy for one entry.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Go ahead; `replace` = an existing destination will be replaced.
    Proceed { replace: bool },
    /// Leave the destination alone.
    Skip,
}

/// Apply the overwrite policy (`-f`, `-n`, `-i`, `-u`) to an existing
/// destination described by `dest_meta`.
///
/// # Errors
/// `DestinationExists` when refusing, or a type mismatch (dir vs non-dir).
pub fn decide(entry: &FileEntry, dest: &Path, dest_meta: Option<&std::fs::Metadata>, config: &Config, tool: &str) -> anyhow::Result<Decision> {
    let Some(dm) = dest_meta else { return Ok(Decision::Proceed { replace: false }) };

    if dm.is_dir() && entry.kind != FileKind::Directory {
        anyhow::bail!("cannot overwrite directory '{}' with non-directory", dest.display());
    }
    if config.overwrite == Overwrite::Skip {
        return Ok(Decision::Skip);
    }
    if config.update {
        let src_m = (entry.meta.mtime(), entry.meta.mtime_nsec());
        let dst_m = (dm.mtime(), dm.mtime_nsec());
        if dst_m >= src_m {
            return Ok(Decision::Skip);
        }
    }
    match config.overwrite {
        Overwrite::Force => Ok(Decision::Proceed { replace: true }),
        Overwrite::Prompt => {
            if prompt(&format!("{tool}: overwrite '{}'? ", dest.display())) {
                Ok(Decision::Proceed { replace: true })
            } else {
                Ok(Decision::Skip)
            }
        }
        // `-u` alone means "replace older destinations".
        Overwrite::Refuse if config.update => Ok(Decision::Proceed { replace: true }),
        Overwrite::Refuse => Err(CopyError::DestinationExists(dest.to_path_buf()).into()),
        Overwrite::Skip => Ok(Decision::Skip),
    }
}

/// Ask a yes/no question on stderr, read the answer from stdin.
#[must_use]
pub fn prompt(question: &str) -> bool {
    let mut err = std::io::stderr();
    let _ = write!(err, "{question}");
    let _ = err.flush();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim_start().chars().next(), Some('y' | 'Y'))
}

/// Build a [`CopyPlan`] from a scan result. `dest` is the destination
/// operand (used for clone eligibility and the disk-space check).
///
/// # Errors
/// Self-copy, refused overwrite, type mismatch, or not enough disk space.
pub fn build_plan(scan: ScanResult, dest: &Path, config: &Config, tool: &str) -> anyhow::Result<CopyPlan> {
    let mut plan = CopyPlan { scan_warnings: scan.warnings, ..Default::default() };

    let dest_device = std::fs::metadata(nearest_existing(dest)).ok().map(|m| m.dev());
    let clone_ok = !config.no_clone && crate::platform::clone_supported();
    // (dev, ino) → destination of the first copy, for hardlinks.
    let mut first_copy: HashMap<(u64, u64), PathBuf> = HashMap::new();

    for scanned in scan.entries {
        let entry = scanned.entry;
        let dest_path = scanned.dest_path;
        let dest_meta = std::fs::symlink_metadata(&dest_path).ok();

        // Self-copy guard (same device + inode), like cp's "are the same file".
        if let Some(dm) = &dest_meta
            && dm.dev() == entry.device_id()
            && dm.ino() == entry.inode()
        {
            return Err(CopyError::SelfCopy(entry.source_path.clone()).into());
        }

        if entry.kind == FileKind::Directory {
            if let Some(dm) = &dest_meta {
                let is_dir = dm.is_dir() || std::fs::metadata(&dest_path).map(|m| m.is_dir()).unwrap_or(false);
                if !is_dir {
                    anyhow::bail!(
                        "cannot overwrite non-directory '{}' with directory '{}'",
                        dest_path.display(),
                        entry.source_path.display()
                    );
                }
            }
            plan.dirs.push(EntryJob { entry, dest: dest_path, replace: dest_meta.is_some() });
            continue;
        }

        let replace = match decide(&entry, &dest_path, dest_meta.as_ref(), config, tool)? {
            Decision::Skip => {
                plan.skipped.push(dest_path);
                continue;
            }
            Decision::Proceed { replace } => replace,
        };

        match entry.kind {
            FileKind::Symlink { .. } => plan.symlinks.push(EntryJob { entry, dest: dest_path, replace }),
            FileKind::Fifo => plan.fifos.push(EntryJob { entry, dest: dest_path, replace }),
            FileKind::File => {
                if entry.meta.nlink() > 1 {
                    let key = (entry.device_id(), entry.inode());
                    if let Some(target) = first_copy.get(&key) {
                        plan.hardlinks.push(LinkJob { entry, dest: dest_path, target: target.clone(), replace });
                        continue;
                    }
                    first_copy.insert(key, dest_path.clone());
                }
                plan.total_bytes += entry.size();
                plan.total_files += 1;
                let clone = clone_ok && dest_device == Some(entry.device_id());
                plan.files.push(FileJob { entry, dest: dest_path, clone, replace });
            }
            FileKind::Directory | FileKind::Other => {}
        }
    }

    // Largest files first: the long transfers start early and the small ones
    // fill the gaps, so all workers finish at about the same time.
    plan.files.sort_by_key(|j| std::cmp::Reverse(j.entry.size()));

    // Clones share blocks (CoW); sparse files only need their allocated bytes.
    let needed: u64 = plan
        .files
        .iter()
        .filter(|j| !j.clone)
        .map(|j| if j.entry.is_sparse() { j.entry.allocated() } else { j.entry.size() })
        .sum();
    check_disk_space(dest, needed)?;

    Ok(plan)
}

/// Verify the destination filesystem has at least `needed` free bytes.
///
/// # Errors
/// When the space is known to be insufficient.
pub fn check_disk_space(dest: &Path, needed: u64) -> anyhow::Result<()> {
    if needed == 0 {
        return Ok(());
    }
    if let Ok(stat) = nix::sys::statvfs::statvfs(&nearest_existing(dest)) {
        #[allow(clippy::useless_conversion)]
        let avail = u64::from(stat.blocks_available()).saturating_mul(u64::from(stat.fragment_size()));
        if avail < needed {
            anyhow::bail!(
                "not enough space on destination: need {} but only {} available",
                bytesize::ByteSize(needed),
                bytesize::ByteSize(avail),
            );
        }
    }
    Ok(())
}

/// `dest` or its nearest existing ancestor.
#[must_use]
pub fn nearest_existing(dest: &Path) -> PathBuf {
    let mut p = dest;
    loop {
        if p.exists() {
            return p.to_path_buf();
        }
        match p.parent() {
            Some(parent) if parent.as_os_str().is_empty() => return PathBuf::from("."),
            Some(parent) => p = parent,
            None => return PathBuf::from("."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_for(path: &Path) -> FileEntry {
        FileEntry {
            source_path: path.to_path_buf(),
            relative_path: PathBuf::new(),
            meta: std::fs::symlink_metadata(path).unwrap(),
            kind: FileKind::File,
        }
    }

    #[test]
    fn policy_matrix() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("s");
        let dst = d.path().join("d");
        std::fs::write(&src, b"new").unwrap();
        std::fs::write(&dst, b"old").unwrap();
        let e = entry_for(&src);
        let dm = std::fs::metadata(&dst).unwrap();
        let cfg = |o| Config { overwrite: o, ..Default::default() };

        assert_eq!(decide(&e, &dst, None, &cfg(Overwrite::Refuse), "t").unwrap(), Decision::Proceed { replace: false });
        assert!(decide(&e, &dst, Some(&dm), &cfg(Overwrite::Refuse), "t").is_err());
        assert_eq!(decide(&e, &dst, Some(&dm), &cfg(Overwrite::Force), "t").unwrap(), Decision::Proceed { replace: true });
        assert_eq!(decide(&e, &dst, Some(&dm), &cfg(Overwrite::Skip), "t").unwrap(), Decision::Skip);
    }

    #[test]
    fn update_compares_mtimes() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("s");
        let dst = d.path().join("d");
        std::fs::write(&src, b"new").unwrap();
        std::fs::write(&dst, b"old").unwrap();
        let old = filetime::FileTime::from_unix_time(1_000_000_000, 0);
        let new = filetime::FileTime::from_unix_time(1_500_000_000, 0);
        let up = Config { update: true, ..Default::default() };

        filetime::set_file_times(&src, new, new).unwrap();
        filetime::set_file_times(&dst, old, old).unwrap();
        let dm = std::fs::metadata(&dst).unwrap();
        assert_eq!(decide(&entry_for(&src), &dst, Some(&dm), &up, "t").unwrap(), Decision::Proceed { replace: true });

        filetime::set_file_times(&src, old, old).unwrap();
        filetime::set_file_times(&dst, new, new).unwrap();
        let dm = std::fs::metadata(&dst).unwrap();
        assert_eq!(decide(&entry_for(&src), &dst, Some(&dm), &up, "t").unwrap(), Decision::Skip);
    }

    #[test]
    fn file_over_directory_refused() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("s");
        std::fs::write(&src, b"x").unwrap();
        let dm = std::fs::metadata(d.path()).unwrap();
        let force = Config { overwrite: Overwrite::Force, ..Default::default() };
        assert!(decide(&entry_for(&src), d.path(), Some(&dm), &force, "t").is_err());
    }

    #[test]
    fn disk_space_check() {
        let d = tempfile::tempdir().unwrap();
        assert!(check_disk_space(d.path(), 1).is_ok());
        assert!(check_disk_space(d.path(), u64::MAX).is_err());
    }
}
