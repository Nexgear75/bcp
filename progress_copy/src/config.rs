use std::path::PathBuf;

/// Which metadata attributes to preserve on copy (Phase 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Preserve {
    pub mode: bool,
    pub timestamps: bool,
    pub xattrs: bool,
}

impl Preserve {
    /// Preserve everything.
    pub fn all() -> Self {
        Self { mode: true, timestamps: true, xattrs: true }
    }

    /// True if at least one attribute should be preserved.
    pub fn any(&self) -> bool {
        self.mode || self.timestamps || self.xattrs
    }
}

/// How progress should be displayed (Phase 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProgressMode {
    /// Rich multi-bar display (default on a TTY).
    #[default]
    Rich,
    /// Single compact line (`--quiet`, or auto on non-TTY).
    Quiet,
    /// No progress output at all (`--no-progress`).
    Silent,
}

/// Configuration for a copy (or move) operation.
///
/// `source`/`destination` carry the single-file Phase-1 path used by
/// [`crate::run`]; the multi-source recursive engine receives its
/// sources and destination explicitly and reads the option fields here.
#[derive(Debug, Clone)]
pub struct Config {
    /// Path to the source file (single-file `run()` path).
    pub source: PathBuf,
    /// Path to the destination file (single-file `run()` path).
    pub destination: PathBuf,
    /// Force overwrite of destination if it exists.
    pub force: bool,
    /// Do not overwrite an existing destination (no-clobber).
    pub no_clobber: bool,
    /// Enable verbose output.
    pub verbose: bool,
    /// Size of the read/write buffer in bytes. Defaults to 1 MB (1048576).
    pub buffer_size: usize,

    // ── Phase 2 ──────────────────────────────────────────────────────
    /// Recurse into directories (`-r`).
    pub recursive: bool,
    /// Number of worker threads (`--threads`). `None` = auto (CPU count).
    pub threads: Option<usize>,
    /// Disable macOS `clonefile()` copy-on-write (`--no-clone`).
    pub no_clone: bool,
    /// Disable Linux `copy_file_range()` kernel copy (`--no-kernel-copy`).
    pub no_kernel_copy: bool,
    /// Scan + plan only, perform no write I/O (`--dry-run`).
    pub dry_run: bool,
    /// Abort on the first per-file error instead of skipping (`--strict`).
    pub strict: bool,

    // ── Phase 3 ──────────────────────────────────────────────────────
    /// Metadata attributes to preserve (`--preserve`).
    pub preserve: Preserve,
    /// Progress display mode.
    pub progress: ProgressMode,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            source: PathBuf::new(),
            destination: PathBuf::new(),
            force: false,
            no_clobber: false,
            verbose: false,
            buffer_size: 1024 * 1024, // 1 MB per D-04
            recursive: false,
            threads: None,
            no_clone: false,
            no_kernel_copy: false,
            dry_run: false,
            strict: false,
            preserve: Preserve::default(),
            progress: ProgressMode::default(),
        }
    }
}

impl Config {
    /// Resolve the effective worker-thread count.
    pub fn thread_count(&self) -> usize {
        self.threads.unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        })
    }
}
