//! Shared fixtures for integration tests.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use progress_copy::{Config, ProgressMode};

pub fn write(path: &Path, content: &[u8]) {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(path, content).unwrap();
}

pub fn silent() -> Config {
    Config { progress: ProgressMode::Silent, ..Default::default() }
}

pub fn silent_recursive() -> Config {
    Config { recursive: true, progress: ProgressMode::Silent, ..Default::default() }
}

pub fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

pub fn mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::symlink_metadata(p).unwrap().permissions().mode() & 0o7777
}

pub fn umask() -> u32 {
    progress_copy::atomic::process_umask()
}

/// All leftover temp entries (`*.bcptmp`) under `root`.
pub fn leftover_temps(root: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".bcptmp"))
        .map(|e| e.path().to_path_buf())
        .collect()
}

/// Run `f` on another thread and fail the test if it takes longer than
/// `secs` (regression guard for hangs).
pub fn within<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(std::time::Duration::from_secs(secs)).expect("operation hung (timeout)")
}

/// A directory on a *different filesystem* than the system temp dir, so
/// moves have to copy. `None` when no such filesystem can be provided
/// (the test is then skipped).
///
/// - `BCP_XDEV_DIR` env var: use that directory.
/// - macOS: a small APFS disk image mounted for the duration of the test.
/// - Linux: `/dev/shm` when it is a different device.
pub struct OtherFs {
    pub dir: PathBuf,
    image: Option<(PathBuf, tempfile::TempDir)>,
}

impl OtherFs {
    /// Like [`OtherFs::try_new`], but panics when `BCP_REQUIRE_XDEV=1` and no
    /// second filesystem is available (so CI never skips silently).
    pub fn new() -> Option<Self> {
        let r = Self::try_new();
        if r.is_none() && std::env::var_os("BCP_REQUIRE_XDEV").is_some() {
            panic!("BCP_REQUIRE_XDEV is set but no second filesystem is available");
        }
        if r.is_none() {
            eprintln!("note: no second filesystem available, cross-filesystem test skipped");
        }
        r
    }

    fn try_new() -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        let tmp_dev = fs::metadata(std::env::temp_dir()).ok()?.dev();
        if let Ok(d) = std::env::var("BCP_XDEV_DIR") {
            let dir = tempfile::Builder::new().prefix("bcp-xdev").tempdir_in(d).ok()?.keep();
            return Some(Self { dir, image: None });
        }
        if cfg!(target_os = "linux") {
            let shm = Path::new("/dev/shm");
            if fs::metadata(shm).ok()?.dev() != tmp_dev {
                let dir = tempfile::Builder::new().prefix("bcp-xdev").tempdir_in(shm).ok()?.keep();
                return Some(Self { dir, image: None });
            }
            return None;
        }
        if cfg!(target_os = "macos") {
            let work = tempfile::tempdir().ok()?;
            let img = work.path().join("vol.dmg");
            let mnt = work.path().join("mnt");
            let ok = Command::new("hdiutil")
                .args(["create", "-quiet", "-size", "48m", "-fs", "APFS", "-volname", "bcptest"])
                .arg(&img)
                .status()
                .ok()?
                .success()
                && Command::new("hdiutil")
                    .args(["attach", "-quiet", "-nobrowse", "-mountpoint"])
                    .arg(&mnt)
                    .arg(&img)
                    .status()
                    .ok()?
                    .success();
            if !ok {
                return None;
            }
            return Some(Self { dir: mnt.clone(), image: Some((mnt, work)) });
        }
        None
    }
}

impl Drop for OtherFs {
    fn drop(&mut self) {
        if let Some((mnt, _work)) = &self.image {
            let _ = Command::new("hdiutil").args(["detach", "-quiet", "-force"]).arg(mnt).status();
        } else {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}
