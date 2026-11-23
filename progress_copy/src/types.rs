use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

/// What kind of filesystem entry a [`FileEntry`] describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileKind {
    /// A regular file (has bytes to copy).
    File,
    /// A directory (created at the destination, no bytes).
    Directory,
    /// A symlink — `target` is its link target, recreated, not followed.
    Symlink { target: PathBuf },
    /// A named pipe — recreated with `mkfifo`.
    Fifo,
    /// Anything else (socket, device) — cannot be copied.
    Other,
}

/// One entry discovered during the scan phase.
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Source path.
    pub source_path: PathBuf,
    /// Path relative to the source root.
    pub relative_path: PathBuf,
    /// Full source metadata captured at scan time.
    pub meta: std::fs::Metadata,
    /// Entry kind.
    pub kind: FileKind,
}

impl FileEntry {
    /// Size in bytes (0 for anything but regular files).
    #[must_use]
    pub fn size(&self) -> u64 {
        if self.kind == FileKind::File { self.meta.len() } else { 0 }
    }

    /// Device id of the source.
    #[must_use]
    pub fn device_id(&self) -> u64 {
        std::os::unix::fs::MetadataExt::dev(&self.meta)
    }

    /// Inode of the source.
    #[must_use]
    pub fn inode(&self) -> u64 {
        std::os::unix::fs::MetadataExt::ino(&self.meta)
    }

    /// Bytes actually allocated on disk (smaller than `size` for sparse files).
    #[must_use]
    pub fn allocated(&self) -> u64 {
        std::os::unix::fs::MetadataExt::blocks(&self.meta).saturating_mul(512)
    }

    /// True when the file has holes worth preserving.
    #[must_use]
    pub fn is_sparse(&self) -> bool {
        self.kind == FileKind::File && self.size() > 64 * 1024 && self.allocated() < self.size()
    }
}

/// A non-fatal problem encountered during scan or plan.
#[derive(Debug, Clone)]
pub enum ScanWarning {
    /// An unsupported entry kind (socket, device…) — not copied.
    Unsupported { path: PathBuf },
    /// Could not read an entry during the walk — not copied.
    Unreadable { path: PathBuf, reason: String },
}

impl fmt::Display for ScanWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { path } => {
                write!(f, "cannot copy special file '{}' (socket or device)", path.display())
            }
            Self::Unreadable { path, reason } => {
                write!(f, "cannot read '{}': {reason}", path.display())
            }
        }
    }
}

/// A regular file to transfer.
#[derive(Debug, Clone)]
pub struct FileJob {
    pub entry: FileEntry,
    pub dest: PathBuf,
    /// Eligible for a copy-on-write clone (same volume, platform support).
    pub clone: bool,
    /// The destination exists and the overwrite policy allows replacing it.
    pub replace: bool,
}

/// A non-file entry to recreate (dir, symlink, fifo, hardlink).
#[derive(Debug, Clone)]
pub struct EntryJob {
    pub entry: FileEntry,
    pub dest: PathBuf,
    pub replace: bool,
}

/// A hardlink to an already-copied file.
#[derive(Debug, Clone)]
pub struct LinkJob {
    pub entry: FileEntry,
    pub dest: PathBuf,
    /// Destination of the first copy of this inode.
    pub target: PathBuf,
    pub replace: bool,
}

/// The complete plan produced after scanning and analysing sources.
#[derive(Debug, Default)]
pub struct CopyPlan {
    /// Directories to create at the destination (parents before children).
    pub dirs: Vec<EntryJob>,
    /// Symlinks to recreate.
    pub symlinks: Vec<EntryJob>,
    /// Named pipes to recreate.
    pub fifos: Vec<EntryJob>,
    /// Regular files to transfer (largest first).
    pub files: Vec<FileJob>,
    /// Extra names for files already in `files` (hardlinks).
    pub hardlinks: Vec<LinkJob>,
    /// Entries skipped because of the overwrite policy (`-n`, `-u`, `-i`).
    pub skipped: Vec<PathBuf>,
    /// Total bytes across all files to transfer.
    pub total_bytes: u64,
    /// Total number of files (regular files only).
    pub total_files: u64,
    /// Non-fatal warnings gathered during scan/plan.
    pub scan_warnings: Vec<ScanWarning>,
}

/// Atomic counters shared between worker threads and the progress display.
#[derive(Debug, Clone, Default)]
pub struct CopyCounters {
    /// Bytes copied so far (all strategies).
    pub bytes_copied: Arc<AtomicU64>,
    /// Files completed so far (all strategies).
    pub files_done: Arc<AtomicU64>,
    /// Files that failed.
    pub files_failed: Arc<AtomicU64>,
    /// Directories created so far (preparation phase).
    pub dirs_done: Arc<AtomicU64>,
}

impl CopyCounters {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Final outcome of an executed plan.
#[derive(Debug, Default, Clone)]
pub struct CopyReport {
    pub files_copied: u64,
    pub files_failed: u64,
    pub files_skipped: u64,
    pub bytes_copied: u64,
    pub dirs_created: u64,
    pub symlinks_created: u64,
    pub hardlinks_created: u64,
    pub fifos_created: u64,
    /// Sources renamed in place (`bmv` on the same filesystem).
    pub renamed: u64,
    /// True if the operation was interrupted by the user (SIGINT/SIGTERM).
    pub cancelled: bool,
    /// Per-entry errors (each one makes the run fail with exit code 1).
    pub errors: Vec<String>,
    /// Informational warnings (do not change the exit code).
    pub warnings: Vec<String>,
}

impl CopyReport {
    /// True when everything requested was done.
    #[must_use]
    pub fn is_success(&self) -> bool {
        !self.cancelled && self.files_failed == 0 && self.errors.is_empty()
    }

    /// Process exit code for this report: 0 ok, 1 failure, 130 interrupted.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        if self.cancelled {
            130
        } else if self.is_success() {
            0
        } else {
            1
        }
    }

    /// Merge another report into this one.
    pub fn absorb(&mut self, other: CopyReport) {
        self.files_copied += other.files_copied;
        self.files_failed += other.files_failed;
        self.files_skipped += other.files_skipped;
        self.bytes_copied += other.bytes_copied;
        self.dirs_created += other.dirs_created;
        self.symlinks_created += other.symlinks_created;
        self.hardlinks_created += other.hardlinks_created;
        self.fifos_created += other.fifos_created;
        self.renamed += other.renamed;
        self.cancelled |= other.cancelled;
        self.errors.extend(other.errors);
        self.warnings.extend(other.warnings);
    }
}
