use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Instant;

/// Shared progress state used by the single-file copy engine and display.
///
/// All counters use `AtomicU64` for lock-free access from multiple threads.
/// The copy worker increments `bytes_copied` and `files_done` with relaxed
/// ordering; the progress display thread reads them every 100ms.
pub struct CopyProgress {
    /// Total bytes copied so far.
    pub bytes_copied: AtomicU64,
    /// Number of files completed (always 1 in single-file Phase 1).
    pub files_done: AtomicU64,
    /// When the copy started.
    pub start_time: Instant,
    /// Display name of the source file.
    pub source_name: String,
}

impl CopyProgress {
    /// Create a new `CopyProgress` with all counters at zero and `start_time`
    /// set to the current instant.
    pub fn new(source_name: String) -> Self {
        Self {
            bytes_copied: AtomicU64::new(0),
            files_done: AtomicU64::new(0),
            start_time: Instant::now(),
            source_name,
        }
    }
}

/// What kind of filesystem entry a [`FileEntry`] describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileKind {
    /// A regular file (has bytes to copy).
    File,
    /// A directory (created at the destination, no bytes).
    Directory,
    /// A symlink — `target` is its link target, recreated, not followed.
    Symlink { target: PathBuf },
    /// Anything else (fifo, socket, device) — skipped with a warning.
    Other,
}

/// One entry discovered during the scan phase.
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Absolute source path.
    pub source_path: PathBuf,
    /// Path relative to the source root, used to rebuild the dest path.
    pub relative_path: PathBuf,
    /// Size in bytes (0 for dirs/symlinks).
    pub size: u64,
    /// Device id (for same-device / clone detection).
    pub device_id: u64,
    /// Inode (for self-copy detection).
    pub inode: u64,
    /// Entry kind.
    pub kind: FileKind,
}

/// A non-fatal problem encountered during scan or plan.
#[derive(Debug, Clone)]
pub enum ScanWarning {
    /// Source and destination resolve to the same file — skipped.
    SameFile { path: PathBuf },
    /// An unsupported entry kind (socket, fifo…) — skipped.
    Unsupported { path: PathBuf },
    /// Could not read an entry during the walk — skipped.
    Unreadable { path: PathBuf, reason: String },
}

/// The complete plan produced after scanning and analysing sources.
#[derive(Debug, Default)]
pub struct CopyPlan {
    /// Directories to create at the destination (in walk order).
    pub dirs_to_create: Vec<PathBuf>,
    /// Symlinks to recreate: (source entry, destination path).
    pub symlinks: Vec<(FileEntry, PathBuf)>,
    /// Regular files to copy: (source entry, destination path).
    pub files_to_copy: Vec<(FileEntry, PathBuf)>,
    /// Files eligible for macOS `clonefile()`: (source entry, dest path).
    pub files_to_clone: Vec<(FileEntry, PathBuf)>,
    /// Total bytes across all files to transfer.
    pub total_bytes: u64,
    /// Total number of files (copy + clone).
    pub total_files: u64,
    /// Non-fatal warnings gathered during scan/plan.
    pub scan_warnings: Vec<ScanWarning>,
}

/// Atomic counters shared between worker threads and the progress display.
///
/// Each field is an `Arc<AtomicU64>` so it can be handed directly to the
/// copy helpers (which take `&Arc<AtomicU64>`) while the display ticker
/// reads the same storage.
#[derive(Debug, Clone, Default)]
pub struct CopyCounters {
    /// Bytes copied so far (all strategies).
    pub bytes_copied: Arc<AtomicU64>,
    /// Files completed so far (all strategies).
    pub files_done: Arc<AtomicU64>,
    /// Files skipped due to non-fatal errors.
    pub files_failed: Arc<AtomicU64>,
    /// Directories created so far (preparation phase).
    pub dirs_done: Arc<AtomicU64>,
}

impl CopyCounters {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Final outcome of an executed plan.
#[derive(Debug, Default, Clone)]
pub struct CopyReport {
    pub files_copied: u64,
    pub files_failed: u64,
    pub bytes_copied: u64,
    pub dirs_created: u64,
    pub symlinks_created: u64,
    /// True if the operation was interrupted (SIGINT).
    pub cancelled: bool,
    /// Per-file error messages collected when not in strict mode.
    pub errors: Vec<String>,
}
