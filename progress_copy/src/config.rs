//! Options shared by `bcp` and `bmv`.

/// Which metadata attributes to preserve on copy.
///
/// Even when `mode` is false, the destination gets the source permission
/// bits masked by the process umask (exactly like `cp` without `-p`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct Preserve {
    pub mode: bool,
    pub ownership: bool,
    pub timestamps: bool,
    pub xattrs: bool,
}

impl Preserve {
    /// Preserve everything.
    #[must_use]
    pub fn all() -> Self {
        Self { mode: true, ownership: true, timestamps: true, xattrs: true }
    }

    /// What `cp -p` preserves: mode, ownership, timestamps.
    #[must_use]
    pub fn cp_p() -> Self {
        Self { mode: true, ownership: true, timestamps: true, xattrs: false }
    }

    /// True if at least one attribute should be preserved.
    #[must_use]
    pub fn any(&self) -> bool {
        self.mode || self.ownership || self.timestamps || self.xattrs
    }
}

/// How progress should be displayed.
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

/// What to do when a destination entry already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Overwrite {
    /// Fail with an error (default — never lose data silently).
    #[default]
    Refuse,
    /// Replace the existing entry (`-f`).
    Force,
    /// Silently skip the entry (`-n`, like `cp -n`).
    Skip,
    /// Ask on the terminal for each conflict (`-i`).
    Prompt,
}

/// Durability policy (`--sync`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SyncMode {
    /// Never flush explicitly (like `cp`). The rename is still atomic.
    None,
    /// Flush everything once at the end of the run (default): fast, and the
    /// data is durable when the command returns.
    #[default]
    Batch,
    /// Flush every file to stable storage before renaming it into place
    /// (`F_FULLFSYNC` on macOS). Slowest, strongest guarantee mid-run.
    Full,
}

/// Symlink handling for sources (`-P` / `-H` / `-L`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dereference {
    /// Never follow symlinks; copy them as links (`-P`, default).
    #[default]
    Never,
    /// Follow symlinks given on the command line only (`-H`).
    CommandLine,
    /// Follow every symlink (`-L`).
    Always,
}

/// Configuration for a copy (or move) operation.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct Config {
    /// Conflict policy for existing destinations.
    pub overwrite: Overwrite,
    /// Only replace an existing destination when the source is newer (`-u`).
    pub update: bool,
    /// Rename replaced destinations to `<name><suffix>` first (`-b`).
    pub backup_suffix: Option<String>,
    /// Treat the destination as a plain name, never as a directory to copy
    /// into (`-T`).
    pub no_target_directory: bool,
    /// Enable verbose output.
    pub verbose: bool,
    /// Size of the read/write buffer in bytes. Defaults to 1 MiB.
    pub buffer_size: usize,
    /// Recurse into directories (`-r` / `-R`).
    pub recursive: bool,
    /// Number of worker threads (`-j`). `None` = auto (CPU count).
    pub threads: Option<usize>,
    /// Disable macOS `clonefile()` copy-on-write (`--no-clone`).
    pub no_clone: bool,
    /// Disable Linux `copy_file_range()` kernel copy (`--no-kernel-copy`).
    pub no_kernel_copy: bool,
    /// Scan + plan only, perform no write I/O (`--dry-run`).
    pub dry_run: bool,
    /// Abort on the first per-file error instead of skipping (`--strict`).
    pub strict: bool,
    /// Symlink handling for sources.
    pub dereference: Dereference,
    /// Durability policy.
    pub sync: SyncMode,
    /// Metadata attributes to preserve (`-p` / `--preserve`).
    pub preserve: Preserve,
    /// Progress display mode.
    pub progress: ProgressMode,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            overwrite: Overwrite::default(),
            update: false,
            backup_suffix: None,
            no_target_directory: false,
            verbose: false,
            buffer_size: 1024 * 1024,
            recursive: false,
            threads: None,
            no_clone: false,
            no_kernel_copy: false,
            dry_run: false,
            strict: false,
            dereference: Dereference::default(),
            sync: SyncMode::default(),
            preserve: Preserve::default(),
            progress: ProgressMode::default(),
        }
    }
}

impl Config {
    /// Resolve the effective worker-thread count.
    #[must_use]
    pub fn thread_count(&self) -> usize {
        self.threads.unwrap_or_else(|| {
            std::thread::available_parallelism().map_or(4, std::num::NonZero::get)
        })
    }
}
