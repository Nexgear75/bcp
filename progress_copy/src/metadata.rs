//! Metadata transfer: xattrs → ownership → mode → timestamps.
//!
//! The order matters:
//! - xattrs first, while the file is still writable by us (a read-only mode
//!   would make `setxattr` fail on Linux);
//! - ownership before mode, because `chown` clears setuid/setgid bits;
//! - timestamps last, because the other steps may touch them.
//!
//! Regular files are handled through their open descriptor (no path races);
//! directories and symlinks by path.

use std::fs::{File, Metadata};
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use filetime::FileTime;

use crate::config::Preserve;

/// What to apply the metadata to.
#[derive(Clone, Copy)]
pub enum Target<'a> {
    /// An open regular file (the temp file before it is renamed).
    File(&'a File),
    /// A directory, by path.
    Path(&'a Path),
    /// A named pipe, by path. Extended attributes are skipped: touching them
    /// can open the FIFO, which blocks until a writer appears.
    Fifo(&'a Path),
    /// A symlink itself (never its target), by path.
    Symlink(&'a Path),
}

/// Mode bits the destination gets when the mode is *not* preserved:
/// the source permissions masked by the umask, without setuid/setgid/sticky
/// (exactly what `cp` does).
#[must_use]
pub fn default_mode(src_mode: u32) -> u32 {
    src_mode & 0o777 & !crate::atomic::process_umask()
}

/// Apply metadata from `src` (path + scan-time `meta`) onto `target`.
///
/// Always sets the permission bits (preserved or umask-masked). Returns
/// the list of problems; an empty list means full success. Failing to change
/// ownership without privileges is not an error (same as `cp -p`).
#[must_use]
pub fn apply(src: &Path, meta: &Metadata, target: Target<'_>, preserve: Preserve) -> Vec<String> {
    let mut problems = Vec::new();

    if preserve.xattrs
        && matches!(target, Target::File(_) | Target::Path(_))
        && let Err(e) = copy_xattrs(src, target)
    {
        problems.push(format!("xattrs: {e}"));
    }

    let mut owner_ok = true;
    if preserve.ownership {
        match chown(target, meta.uid(), meta.gid()) {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(libc::EPERM) => {
                owner_ok = false;
                // Not root: at least try to keep the group.
                let _ = chown_group(target, meta.gid());
            }
            Err(e) => {
                owner_ok = false;
                problems.push(format!("ownership: {e}"));
            }
        }
    }

    if !matches!(target, Target::Symlink(_)) {
        let mode = if preserve.mode {
            let mut m = meta.mode() & 0o7777;
            let same_owner = meta.uid() == unsafe { libc::geteuid() };
            if !owner_ok || (!preserve.ownership && !same_owner) {
                // Never hand setuid/setgid to a different owner.
                m &= !0o6000;
            }
            m
        } else {
            default_mode(meta.mode())
        };
        if let Err(e) = chmod(target, mode) {
            problems.push(format!("mode: {e}"));
        }
    }

    if preserve.timestamps {
        let atime = FileTime::from_last_access_time(meta);
        let mtime = FileTime::from_last_modification_time(meta);
        let r = match target {
            Target::File(f) => filetime::set_file_handle_times(f, Some(atime), Some(mtime)),
            Target::Path(p) | Target::Fifo(p) => set_times_at(p, meta, false),
            Target::Symlink(p) => set_times_at(p, meta, true),
        };
        if let Err(e) = r {
            problems.push(format!("timestamps: {e}"));
        }
    }

    problems
}

/// `utimensat` by path, without ever opening the entry (`filetime` opens it
/// on macOS, which blocks forever on a FIFO).
fn set_times_at(p: &Path, meta: &Metadata, nofollow: bool) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
    #[allow(clippy::useless_conversion, clippy::cast_possible_truncation)]
    let ts = [
        libc::timespec { tv_sec: meta.atime() as libc::time_t, tv_nsec: meta.atime_nsec() as _ },
        libc::timespec { tv_sec: meta.mtime() as libc::time_t, tv_nsec: meta.mtime_nsec() as _ },
    ];
    let flags = if nofollow { libc::AT_SYMLINK_NOFOLLOW } else { 0 };
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), ts.as_ptr(), flags) };
    if rc == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

fn chmod(target: Target<'_>, mode: u32) -> io::Result<()> {
    let perms = std::fs::Permissions::from_mode(mode);
    match target {
        Target::File(f) => f.set_permissions(perms),
        Target::Path(p) | Target::Fifo(p) => std::fs::set_permissions(p, perms),
        Target::Symlink(_) => Ok(()),
    }
}

fn chown(target: Target<'_>, uid: u32, gid: u32) -> io::Result<()> {
    match target {
        Target::File(f) => std::os::unix::fs::fchown(f, Some(uid), Some(gid)),
        Target::Path(p) | Target::Fifo(p) => std::os::unix::fs::chown(p, Some(uid), Some(gid)),
        Target::Symlink(p) => std::os::unix::fs::lchown(p, Some(uid), Some(gid)),
    }
}

fn chown_group(target: Target<'_>, gid: u32) -> io::Result<()> {
    match target {
        Target::File(f) => std::os::unix::fs::fchown(f, None, Some(gid)),
        Target::Path(p) | Target::Fifo(p) => std::os::unix::fs::chown(p, None, Some(gid)),
        Target::Symlink(p) => std::os::unix::fs::lchown(p, None, Some(gid)),
    }
}

/// Attributes an unprivileged process can't set; skipping them is normal.
fn ignorable(name: &std::ffi::OsStr, e: &io::Error) -> bool {
    let n = name.to_string_lossy();
    let privileged_ns = n.starts_with("security.") || n.starts_with("system.") || n.starts_with("trusted.");
    let denied = matches!(e.raw_os_error(), Some(libc::EPERM | libc::EACCES));
    let unsupported = matches!(e.raw_os_error(), Some(libc::ENOTSUP)) || e.raw_os_error() == Some(libc::EOPNOTSUPP);
    (privileged_ns && (denied || unsupported)) || n == "com.apple.rootless" || n == "com.apple.provenance"
}

fn copy_xattrs(src: &Path, target: Target<'_>) -> io::Result<()> {
    use xattr::FileExt;
    let names = match xattr::list(src) {
        Ok(n) => n,
        // Source filesystem without xattrs: nothing to copy.
        Err(e) if matches!(e.raw_os_error(), Some(libc::ENOTSUP)) || e.raw_os_error() == Some(libc::EOPNOTSUPP) => {
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    let mut failed = Vec::new();
    for name in names {
        let Some(value) = xattr::get(src, &name)? else { continue };
        let r = match target {
            Target::File(f) => f.set_xattr(&name, &value),
            Target::Path(p) => xattr::set(p, &name, &value),
            Target::Symlink(_) | Target::Fifo(_) => Ok(()),
        };
        match r {
            Ok(()) => {}
            Err(e) if ignorable(&name, &e) => {}
            // Destination filesystem has no xattr support at all.
            Err(e) if matches!(e.raw_os_error(), Some(libc::ENOTSUP)) || e.raw_os_error() == Some(libc::EOPNOTSUPP) => {
                return Err(io::Error::new(e.kind(), format!("not supported by destination filesystem ({e})")));
            }
            Err(e) => failed.push(format!("{}: {e}", name.to_string_lossy())),
        }
    }
    if failed.is_empty() { Ok(()) } else { Err(io::Error::other(failed.join(", "))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mode_masks_umask_and_special_bits() {
        let umask = crate::atomic::process_umask();
        assert_eq!(default_mode(0o104_755), 0o755 & !umask);
        assert_eq!(default_mode(0o100_600), 0o600 & !umask);
    }

    #[test]
    fn mode_applied_even_without_preserve() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("s");
        std::fs::write(&src, b"x").unwrap();
        std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o755)).unwrap();
        let dst = dir.path().join("d");
        std::fs::write(&dst, b"x").unwrap();
        std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o600)).unwrap();
        let f = File::open(&dst).unwrap();
        let meta = std::fs::metadata(&src).unwrap();
        assert!(apply(&src, &meta, Target::File(&f), Preserve::default()).is_empty());
        let mode = std::fs::metadata(&dst).unwrap().mode() & 0o777;
        assert_eq!(mode, default_mode(0o755));
    }

    #[test]
    fn xattrs_copied_before_readonly_mode() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("s");
        std::fs::write(&src, b"x").unwrap();
        if xattr::set(&src, "user.bcp.test", b"v").is_err() {
            return; // filesystem without user xattrs
        }
        std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o444)).unwrap();
        let dst = dir.path().join("d");
        std::fs::write(&dst, b"x").unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(&dst).unwrap();
        let meta = std::fs::metadata(&src).unwrap();
        let problems = apply(&src, &meta, Target::File(&f), Preserve::all());
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(xattr::get(&dst, "user.bcp.test").unwrap().as_deref(), Some(&b"v"[..]));
        assert_eq!(std::fs::metadata(&dst).unwrap().mode() & 0o777, 0o444);
    }

    #[test]
    fn fifo_metadata_never_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("p");
        let dst = dir.path().join("q");
        let m = nix::sys::stat::Mode::from_bits_truncate(0o640);
        nix::unistd::mkfifo(&src, m).unwrap();
        nix::unistd::mkfifo(&dst, m).unwrap();
        let old = FileTime::from_unix_time(1_000_000_000, 0);
        filetime::set_symlink_file_times(&src, old, old).unwrap();
        let meta = std::fs::symlink_metadata(&src).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let (s2, d2) = (src.clone(), dst.clone());
        std::thread::spawn(move || {
            let _ = tx.send(apply(&s2, &meta, Target::Fifo(&d2), Preserve::all()));
        });
        let problems = rx.recv_timeout(std::time::Duration::from_secs(5)).expect("metadata on a FIFO blocked");
        assert!(problems.is_empty(), "{problems:?}");
        let got = FileTime::from_last_modification_time(&std::fs::symlink_metadata(&dst).unwrap());
        assert_eq!(got, old);
    }

    #[test]
    fn symlink_timestamps_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("t");
        std::fs::write(&target, b"x").unwrap();
        let src = dir.path().join("s");
        let dst = dir.path().join("d");
        std::os::unix::fs::symlink(&target, &src).unwrap();
        std::os::unix::fs::symlink(&target, &dst).unwrap();
        let old = FileTime::from_unix_time(1_000_000_000, 0);
        filetime::set_symlink_file_times(&src, old, old).unwrap();
        let before = std::fs::metadata(&target).unwrap().mtime();
        let meta = std::fs::symlink_metadata(&src).unwrap();
        assert!(apply(&src, &meta, Target::Symlink(&dst), Preserve::all()).is_empty());
        assert_eq!(std::fs::symlink_metadata(&dst).unwrap().mtime(), 1_000_000_000);
        assert_eq!(std::fs::metadata(&target).unwrap().mtime(), before, "target must not be touched");
    }

    #[test]
    fn directory_timestamps_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("s");
        let dst = dir.path().join("d");
        std::fs::create_dir(&src).unwrap();
        std::fs::create_dir(&dst).unwrap();
        let old = FileTime::from_unix_time(1_000_000_000, 0);
        filetime::set_file_times(&src, old, old).unwrap();
        let meta = std::fs::metadata(&src).unwrap();
        assert!(apply(&src, &meta, Target::Path(&dst), Preserve::all()).is_empty());
        let got = FileTime::from_last_modification_time(&std::fs::metadata(&dst).unwrap());
        assert_eq!(got, old);
    }
}
