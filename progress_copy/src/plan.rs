//! Phase 2 of the pipeline: turn scanned entries into an executable
//! [`CopyPlan`], applying self-copy detection, overwrite policy, clone
//! eligibility, and a disk-space pre-flight check.

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use anyhow::Context;

use crate::config::Config;
use crate::error::CopyError;
use crate::scan::ScanResult;
use crate::types::{CopyPlan, FileKind};

/// Build a [`CopyPlan`] from a scan result for the given destination.
pub fn build_plan(scan: ScanResult, dest: &Path, config: &Config) -> anyhow::Result<CopyPlan> {
    let mut plan = CopyPlan {
        scan_warnings: scan.warnings,
        ..Default::default()
    };

    // Device id of the destination root, used for clone eligibility.
    let dest_device = dest_device_id(dest);

    let clone_ok = !config.no_clone && crate::platform::clone_supported();

    for scanned in scan.entries {
        let entry = scanned.entry;
        let dest_path = scanned.dest_path;

        // Single `lstat` of the destination entry, reused for both the
        // self-copy guard and the overwrite policy. This avoids the extra
        // source+dest stats `same_file::is_same_file` performed per entry:
        // in the dominant case (a move onto a fresh destination) it's one
        // syscall that fails fast with ENOENT instead of three.
        let dest_meta = std::fs::symlink_metadata(&dest_path).ok();

        // Self-copy guard (same device + inode) — a hard error, matching
        // `cp` ("'x' and 'x' are the same file"). The source device/inode were
        // already captured during the scan, so no extra source stat is needed.
        if let Some(dm) = &dest_meta
            && dm.dev() == entry.device_id
            && dm.ino() == entry.inode
        {
            return Err(CopyError::SelfCopy(entry.source_path.clone()).into());
        }

        match &entry.kind {
            FileKind::Directory => {
                plan.dirs_to_create.push(dest_path);
            }
            FileKind::Symlink { .. } => {
                // Overwrite policy applies to existing destinations.
                check_overwrite_meta(&dest_path, dest_meta.as_ref(), config)?;
                plan.symlinks.push((entry, dest_path));
            }
            FileKind::File => {
                check_overwrite_meta(&dest_path, dest_meta.as_ref(), config)?;
                plan.total_bytes += entry.size;
                plan.total_files += 1;

                let cloneable = clone_ok
                    && dest_device.is_some()
                    && Some(entry.device_id) == dest_device;
                if cloneable {
                    plan.files_to_clone.push((entry, dest_path));
                } else {
                    plan.files_to_copy.push((entry, dest_path));
                }
            }
            FileKind::Other => {}
        }
    }

    // Disk-space pre-flight: clones share blocks (CoW) so only the
    // truly-copied bytes need free space.
    let needed: u64 = plan.files_to_copy.iter().map(|(e, _)| e.size).sum();
    check_disk_space(dest, needed)
        .with_context(|| "Insufficient disk space for this transfer")?;

    Ok(plan)
}

/// Device id of `dest` (or its nearest existing ancestor).
fn dest_device_id(dest: &Path) -> Option<u64> {
    std::fs::metadata(nearest_existing(dest)).ok().map(|m| m.dev())
}

/// Apply the overwrite policy from an already-fetched destination `lstat`.
///
/// An existing destination is an error unless `-f` (force) is set and `-n`
/// (no-clobber) is not. `validate_flags` already rejects `-f` + `-n`. The
/// metadata comes from `symlink_metadata`, so a symlink at the destination is
/// treated as the entry itself, not its target.
fn check_overwrite_meta(
    dest: &Path,
    dest_meta: Option<&std::fs::Metadata>,
    config: &Config,
) -> anyhow::Result<()> {
    if dest_meta.is_some() && (config.no_clobber || !config.force) {
        return Err(CopyError::DestinationExists(dest.to_path_buf()).into());
    }
    Ok(())
}

/// Verify the destination filesystem has at least `needed` free bytes.
///
/// Uses `statvfs` via `nix`; failures to query are treated as "unknown"
/// and allowed through rather than blocking a copy.
pub fn check_disk_space(dest: &Path, needed: u64) -> anyhow::Result<()> {
    if needed == 0 {
        return Ok(());
    }
    let probe = nearest_existing(dest);
    if let Ok(stat) = nix::sys::statvfs::statvfs(&probe) {
        let avail = stat.blocks_available() as u64 * stat.fragment_size();
        if avail < needed {
            anyhow::bail!(
                "need {} but only {} available on destination filesystem",
                bytesize::ByteSize(needed),
                bytesize::ByteSize(avail),
            );
        }
    }
    Ok(())
}

fn nearest_existing(dest: &Path) -> std::path::PathBuf {
    let mut p = dest;
    loop {
        if p.exists() {
            return p.to_path_buf();
        }
        match p.parent() {
            // An empty parent ("") means the current directory.
            Some(parent) if parent.as_os_str().is_empty() => return std::path::PathBuf::from("."),
            Some(parent) => p = parent,
            None => return std::path::PathBuf::from("."),
        }
    }
}
