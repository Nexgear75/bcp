//! Atomic-write primitives: registered temp files, no-clobber install,
//! durability helpers.
//!
//! Every byte is written to a uniquely-named temp entry **in the destination
//! directory** (same filesystem → `rename` is atomic), which is then renamed
//! over the final name. Temp entries are tracked in a process-wide registry so
//! a forced quit (second Ctrl+C) can remove them, and a guard removes them on
//! every error path.

use std::collections::HashSet;
use std::ffi::{CString, OsString};
use std::fs::File;
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

// ── Temp registry ──────────────────────────────────────────────────

static REGISTRY: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);
static COUNTER: AtomicU64 = AtomicU64::new(0);

fn register(path: &Path) {
    if let Ok(mut r) = REGISTRY.lock() {
        r.get_or_insert_with(HashSet::new).insert(path.to_path_buf());
    }
}

fn unregister(path: &Path) {
    if let Ok(mut r) = REGISTRY.lock()
        && let Some(set) = r.as_mut()
    {
        set.remove(path);
    }
}

/// Remove every temp entry still registered. Returns how many were removed.
/// Called by the signal handler on a forced quit.
pub fn cleanup_registered_temps() -> usize {
    cleanup_matching(|_| true)
}

fn cleanup_matching(pred: impl Fn(&Path) -> bool) -> usize {
    let paths: Vec<PathBuf> = match REGISTRY.lock() {
        Ok(mut r) => match r.as_mut() {
            Some(set) => {
                let taken: Vec<PathBuf> = set.iter().filter(|p| pred(p)).cloned().collect();
                for p in &taken {
                    set.remove(p);
                }
                taken
            }
            None => Vec::new(),
        },
        Err(_) => return 0,
    };
    paths.iter().filter(|p| std::fs::remove_file(p).is_ok()).count()
}

/// Number of temp entries currently registered (for tests and diagnostics).
pub fn registered_temp_count() -> usize {
    REGISTRY.lock().ok().and_then(|r| r.as_ref().map(HashSet::len)).unwrap_or(0)
}

/// Build a unique temp name next to `dest`: `.<name>.<pid><n>.bcptmp`.
/// The name part is truncated so the result always fits in `NAME_MAX`.
fn temp_name_for(dest: &Path) -> PathBuf {
    let dir = dest.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = dest.file_name().map(OsStrExt::as_bytes).unwrap_or(b"bcp");
    let stem = &name[..name.len().min(180)];
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut bytes = Vec::with_capacity(stem.len() + 32);
    bytes.push(b'.');
    bytes.extend_from_slice(stem);
    bytes.extend_from_slice(format!(".{:x}{n:x}.bcptmp", std::process::id()).as_bytes());
    dir.join(OsString::from_vec(bytes))
}

/// A reserved temp path next to a destination. Removed on drop unless
/// [`TempPath::install`] succeeded.
#[derive(Debug)]
pub struct TempPath {
    path: PathBuf,
    armed: bool,
}

impl TempPath {
    /// Reserve a fresh name (nothing is created). Use for entries created by
    /// other syscalls (`clonefile`, `symlink`, `mkfifo`, `link`).
    #[must_use]
    pub fn reserve(dest: &Path) -> Self {
        let path = temp_name_for(dest);
        register(&path);
        Self { path, armed: true }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Atomically move the temp entry to `dest`.
    ///
    /// # Errors
    /// `AlreadyExists` when `dest` exists and `replace` is false; any rename
    /// error otherwise. The temp entry is removed on failure.
    pub fn install(mut self, dest: &Path, replace: bool, backup: Option<&str>) -> io::Result<()> {
        install(&self.path, dest, replace, backup)?;
        self.armed = false;
        unregister(&self.path);
        Ok(())
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
            unregister(&self.path);
        }
    }
}

/// A freshly created temp file (mode 0600, `O_EXCL`) next to a destination.
#[derive(Debug)]
pub struct TempFile {
    pub file: File,
    path: TempPath,
}

impl TempFile {
    /// Create the temp file in `dest`'s directory.
    ///
    /// # Errors
    /// Any `open` error (missing directory, permission…).
    pub fn create_for(dest: &Path) -> io::Result<Self> {
        let mut last = None;
        for _ in 0..8 {
            let path = TempPath::reserve(dest);
            match std::fs::OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(path.path()) {
                Ok(file) => return Ok(Self { file, path }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
                Err(e) => {
                    // Nothing was created: disarm so Drop doesn't remove a
                    // file that isn't ours.
                    let mut path = path;
                    path.armed = false;
                    unregister(&path.path);
                    return Err(e);
                }
            }
        }
        Err(last.unwrap_or_else(|| io::Error::other("could not create temp file")))
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        self.path.path()
    }

    /// Close the file and atomically rename it to `dest`.
    ///
    /// # Errors
    /// See [`TempPath::install`].
    pub fn install(self, dest: &Path, replace: bool, backup: Option<&str>) -> io::Result<()> {
        let Self { file, path } = self;
        drop(file);
        path.install(dest, replace, backup)
    }
}

// ── Rename primitives ──────────────────────────────────────────────

fn cstr(p: &Path) -> io::Result<CString> {
    CString::new(p.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))
}

/// Path of the backup for `dest` (`dest` + suffix).
#[must_use]
pub fn backup_path(dest: &Path, suffix: &str) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

/// Install `from` at `to`.
///
/// - `replace == false`: fails with `AlreadyExists` if `to` exists, with no
///   race window (`renameat2(RENAME_NOREPLACE)` / `renamex_np(RENAME_EXCL)`).
/// - `replace == true`: atomic replace; with `backup`, the old entry is first
///   renamed to `to<suffix>`.
///
/// # Errors
/// Any rename error.
pub fn install(from: &Path, to: &Path, replace: bool, backup: Option<&str>) -> io::Result<()> {
    if replace {
        if let Some(sfx) = backup
            && std::fs::symlink_metadata(to).is_ok()
        {
            std::fs::rename(to, backup_path(to, sfx))?;
        }
        return std::fs::rename(from, to);
    }
    rename_noreplace(from, to)
}

/// Rename that never replaces an existing `to`.
///
/// # Errors
/// `AlreadyExists` if `to` exists; any other rename error.
pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    let (f, t) = (cstr(from)?, cstr(to)?);
    match sys_rename_noreplace(&f, &t) {
        Ok(()) => return Ok(()),
        Err(e) if !is_unsupported(&e) => return Err(e),
        Err(_) => {}
    }
    // Filesystem without native support. `link` + `unlink` is still atomic
    // with respect to an existing `to` (link fails with EEXIST). macOS `link`
    // follows symlinks, so symlinks use the (racy) check-then-rename.
    let is_symlink = std::fs::symlink_metadata(from).map(|m| m.file_type().is_symlink()).unwrap_or(false);
    if !is_symlink {
        match std::fs::hard_link(from, to) {
            Ok(()) => {
                let _ = std::fs::remove_file(from);
                return Ok(());
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(e),
            Err(_) => {}
        }
    }
    if std::fs::symlink_metadata(to).is_ok() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "destination exists"));
    }
    std::fs::rename(from, to)
}

fn is_unsupported(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(libc::EINVAL | libc::ENOSYS | libc::ENOTSUP | libc::ENOTTY))
        || e.raw_os_error() == Some(libc::EOPNOTSUPP)
}

#[cfg(target_os = "linux")]
fn sys_rename_noreplace(from: &CString, to: &CString) -> io::Result<()> {
    const RENAME_NOREPLACE: libc::c_uint = 1;
    // Raw syscall: works with both glibc and musl.
    let rc = unsafe {
        libc::syscall(libc::SYS_renameat2, libc::AT_FDCWD, from.as_ptr(), libc::AT_FDCWD, to.as_ptr(), RENAME_NOREPLACE)
    };
    if rc == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

#[cfg(target_os = "macos")]
fn sys_rename_noreplace(from: &CString, to: &CString) -> io::Result<()> {
    let rc = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    if rc == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn sys_rename_noreplace(_from: &CString, _to: &CString) -> io::Result<()> {
    Err(io::Error::from_raw_os_error(libc::ENOSYS))
}

// ── Durability ─────────────────────────────────────────────────────

/// Flush a file's data to stable storage.
///
/// macOS: `fcntl(F_FULLFSYNC)` — plain `fsync` only reaches the drive cache.
/// Linux: `fsync`.
///
/// # Errors
/// Any flush error.
#[cfg(target_os = "macos")]
pub fn sync_file(file: &File) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) };
    if rc == -1 {
        // Some filesystems (SMB, FAT) don't support F_FULLFSYNC.
        return file.sync_all();
    }
    Ok(())
}

/// See the macOS variant.
///
/// # Errors
/// Any flush error.
#[cfg(not(target_os = "macos"))]
pub fn sync_file(file: &File) -> io::Result<()> {
    file.sync_all()
}

/// `fsync` a directory so the names created in it are durable.
///
/// # Errors
/// Any open/fsync error.
pub fn fsync_dir(dir: &Path) -> io::Result<()> {
    let d = File::open(dir)?;
    match d.sync_all() {
        // Some filesystems refuse fsync on directories; nothing more to do.
        Err(e) if matches!(e.raw_os_error(), Some(libc::EINVAL | libc::ENOTSUP | libc::EBADF)) => Ok(()),
        r => r,
    }
}

/// Make everything written under `dir`'s filesystem durable (batch mode).
///
/// Linux: one `syncfs`. macOS: `fsync` every written file in parallel at
/// the end (by then the kernel has already written most pages, so this is
/// cheap — measured faster than fsyncing inside the workers), then a single
/// `F_FULLFSYNC` to flush the drive cache.
///
/// # Errors
/// The first flush error.
pub fn sync_filesystem(dir: &Path, written: &[PathBuf]) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        let _ = written;
        use std::os::unix::io::AsRawFd;
        let d = File::open(dir)?;
        let rc = unsafe { libc::syncfs(d.as_raw_fd()) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        use rayon::prelude::*;
        let unopenable = AtomicBool::new(false);
        written.par_iter().try_for_each(|p| match File::open(p) {
            Ok(f) => f.sync_all(),
            Err(_) => {
                // e.g. mode 000: can't reopen it to fsync.
                unopenable.store(true, Ordering::Relaxed);
                Ok(())
            }
        })?;
        if unopenable.load(Ordering::Relaxed) {
            unsafe { libc::sync() };
        }
        let handle = written.iter().rev().find_map(|p| File::open(p).ok());
        match handle {
            Some(f) => sync_file(&f),
            None => sync_file(&File::open(dir)?),
        }
    }
}

// ── Umask ──────────────────────────────────────────────────────────

/// The process umask, read once.
pub fn process_umask() -> u32 {
    static UMASK: OnceLock<u32> = OnceLock::new();
    *UMASK.get_or_init(|| {
        #[cfg(target_os = "linux")]
        if let Ok(s) = std::fs::read_to_string("/proc/self/status")
            && let Some(line) = s.lines().find(|l| l.starts_with("Umask:"))
            && let Ok(m) = u32::from_str_radix(line[6..].trim(), 8)
        {
            return m;
        }
        // umask() can only be read by setting it; restore immediately.
        let m = unsafe { libc::umask(0o022) };
        unsafe { libc::umask(m) };
        #[allow(clippy::useless_conversion)] // mode_t is u16 on macOS, u32 on Linux
        u32::from(m)
    })
}

// ── Single-file convenience API ────────────────────────────────────

/// Copy one regular file atomically (temp → copy → fsync → rename).
///
/// Refuses to replace an existing `dest_path`. The destination mode is the
/// source mode masked by the umask, like `cp`.
///
/// # Errors
/// Open/read/write errors, `Cancelled`, or `AlreadyExists`.
pub fn atomic_copy(
    src_path: &Path,
    dest_path: &Path,
    buffer_size: usize,
    bytes_copied: &Arc<AtomicU64>,
    cancelled: &AtomicBool,
) -> anyhow::Result<u64> {
    let config = crate::Config { buffer_size, sync: crate::config::SyncMode::Full, ..Default::default() };
    crate::engine::copy_single_file(src_path, dest_path, &config, bytes_copied, cancelled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_names_are_unique_and_bounded() {
        let long = "x".repeat(250);
        let dest = Path::new("/tmp").join(&long);
        let a = temp_name_for(&dest);
        let b = temp_name_for(&dest);
        assert_ne!(a, b);
        assert!(a.file_name().unwrap().len() <= 255, "temp name must fit NAME_MAX");
    }

    #[test]
    fn temp_file_removed_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("f");
        let t = TempFile::create_for(&dest).unwrap();
        let p = t.path().to_path_buf();
        assert!(p.exists());
        drop(t);
        assert!(!p.exists());
    }

    #[test]
    fn noreplace_refuses_existing() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::write(&a, b"new").unwrap();
        std::fs::write(&b, b"old").unwrap();
        let e = rename_noreplace(&a, &b).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&b).unwrap(), b"old");
        assert!(a.exists());
    }

    #[test]
    fn install_with_backup_keeps_old_version() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::write(&a, b"new").unwrap();
        std::fs::write(&b, b"old").unwrap();
        install(&a, &b, true, Some("~")).unwrap();
        assert_eq!(std::fs::read(&b).unwrap(), b"new");
        assert_eq!(std::fs::read(dir.path().join("b~")).unwrap(), b"old");
    }

    #[test]
    fn cleanup_registry_removes_temps() {
        let dir = tempfile::tempdir().unwrap();
        let t = TempFile::create_for(&dir.path().join("x")).unwrap();
        let p = t.path().to_path_buf();
        std::mem::forget(t); // simulate a worker killed mid-copy
        assert!(p.exists());
        assert_eq!(cleanup_matching(|q| q.starts_with(dir.path())), 1);
        assert!(!p.exists());
    }

    #[test]
    fn umask_is_sane() {
        assert!(process_umask() <= 0o777);
    }
}
