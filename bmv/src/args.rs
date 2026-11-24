//! Command-line definition of `bmv` (shared with `xtask` to generate
//! shell completions and the man page).

use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use progress_copy::cli;

#[derive(Clone, Copy, ValueEnum)]
pub enum Sync {
    /// Same as batch for moves (a source is never deleted before its copy is on disk)
    None,
    /// Flush everything once before deleting sources (default)
    Batch,
    /// Flush each file before installing it
    Full,
}

/// Move (rename) files and directories with progress bars — a safer `mv`.
///
/// On the same filesystem this is an atomic rename. Across filesystems the
/// data is copied, flushed to disk and verified before the source is deleted;
/// if anything goes wrong, the source is left untouched.
#[derive(Parser)]
#[command(
    name = "bmv",
    version,
    after_help = "Exit status: 0 success, 1 at least one error, 2 usage error, 130 interrupted."
)]
#[allow(clippy::struct_excessive_bools)]
pub struct Args {
    /// Replace existing destinations
    #[arg(short = 'f', long, overrides_with_all = ["no_clobber", "interactive"])]
    pub force: bool,
    /// Never replace existing destinations (skip them silently)
    #[arg(short = 'n', long, overrides_with_all = ["force", "interactive"])]
    pub no_clobber: bool,
    /// Ask before replacing an existing destination
    #[arg(short = 'i', long, overrides_with_all = ["force", "no_clobber"])]
    pub interactive: bool,
    /// Move only when the source is newer than the destination
    #[arg(short = 'u', long)]
    pub update: bool,
    /// Keep a backup of each replaced destination (NAME~)
    #[arg(short = 'b', long)]
    pub backup: bool,
    /// Backup suffix (implies --backup)
    #[arg(short = 'S', long, value_name = "SUFFIX")]
    pub suffix: Option<String>,

    /// Move all sources into DIRECTORY
    #[arg(short = 't', long, value_name = "DIRECTORY", conflicts_with = "no_target_directory")]
    pub target_directory: Option<PathBuf>,
    /// Treat DEST as a normal file, never as a directory to move into
    #[arg(short = 'T', long)]
    pub no_target_directory: bool,

    /// Worker threads for cross-filesystem copies (default: number of CPUs)
    #[arg(short = 'j', long, value_name = "N")]
    pub threads: Option<usize>,
    /// Read/write buffer size, e.g. 256K, 1M (default: 1M)
    #[arg(long, value_name = "SIZE", value_parser = cli::parse_size)]
    pub buffer_size: Option<usize>,
    /// Durability for cross-filesystem moves: batch (default) or full
    #[arg(long, value_enum, value_name = "MODE")]
    pub sync: Option<Sync>,

    /// Stop at the first error instead of continuing
    #[arg(long)]
    pub strict: bool,
    /// Show what would be done without changing anything
    #[arg(long)]
    pub dry_run: bool,
    /// Explain what is being done
    #[arg(short = 'v', long)]
    pub verbose: bool,
    /// Single-line progress
    #[arg(short = 'q', long)]
    pub quiet: bool,
    /// No progress output at all
    #[arg(long)]
    pub no_progress: bool,

    /// SOURCE... DEST, or SOURCE... with -t DIRECTORY
    #[arg(required = true, value_name = "PATHS")]
    pub paths: Vec<PathBuf>,
}
