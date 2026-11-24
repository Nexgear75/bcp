//! Command-line definition of `bcp` (shared with `xtask` to generate
//! shell completions and the man page).

use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use progress_copy::{Preserve, cli};

#[derive(Clone, Copy, ValueEnum)]
pub enum Sync {
    /// No explicit flush (like cp); renames stay atomic
    None,
    /// Flush everything once at the end (default)
    Batch,
    /// Flush each file before installing it (slowest, safest)
    Full,
}

/// Copy files and directories with progress bars — a safer, faster `cp`.
#[derive(Parser)]
#[command(
    name = "bcp",
    version,
    after_help = "Exit status: 0 success, 1 at least one error, 2 usage error, 130 interrupted."
)]
#[allow(clippy::struct_excessive_bools)]
pub struct Args {
    /// Replace existing destination files
    #[arg(short = 'f', long, overrides_with_all = ["no_clobber", "interactive"])]
    pub force: bool,
    /// Never replace existing files (skip them silently)
    #[arg(short = 'n', long, overrides_with_all = ["force", "interactive"])]
    pub no_clobber: bool,
    /// Ask before replacing an existing file
    #[arg(short = 'i', long, overrides_with_all = ["force", "no_clobber"])]
    pub interactive: bool,
    /// Replace only when the source is newer than the destination
    #[arg(short = 'u', long)]
    pub update: bool,
    /// Keep a backup of each replaced file (NAME~)
    #[arg(short = 'b', long)]
    pub backup: bool,
    /// Backup suffix (implies --backup)
    #[arg(short = 'S', long, value_name = "SUFFIX")]
    pub suffix: Option<String>,

    /// Copy directories recursively
    #[arg(short = 'r', short_alias = 'R', long)]
    pub recursive: bool,
    /// Same as -R -P --preserve=all
    #[arg(short = 'a', long)]
    pub archive: bool,
    /// Follow every symlink in the sources
    #[arg(short = 'L', long, overrides_with_all = ["no_dereference", "cmdline_links"])]
    pub dereference: bool,
    /// Never follow symlinks; copy them as links (default)
    #[arg(short = 'P', long, overrides_with_all = ["dereference", "cmdline_links"])]
    pub no_dereference: bool,
    /// Follow symlinks given on the command line only
    #[arg(short = 'H', overrides_with_all = ["dereference", "no_dereference"])]
    pub cmdline_links: bool,

    /// Preserve mode, ownership and timestamps
    #[arg(short = 'p')]
    pub preserve_default: bool,
    /// Preserve the listed attributes: mode,ownership,timestamps,xattrs,all
    #[arg(long, value_name = "ATTRS", num_args = 0..=1, require_equals = true,
          default_missing_value = "mode,ownership,timestamps", value_parser = cli::parse_preserve)]
    pub preserve: Option<Preserve>,

    /// Copy all sources into DIRECTORY
    #[arg(short = 't', long, value_name = "DIRECTORY", conflicts_with = "no_target_directory")]
    pub target_directory: Option<PathBuf>,
    /// Treat DEST as a normal file, never as a directory to copy into
    #[arg(short = 'T', long)]
    pub no_target_directory: bool,

    /// Worker threads (default: number of CPUs)
    #[arg(short = 'j', long, value_name = "N")]
    pub threads: Option<usize>,
    /// Read/write buffer size, e.g. 256K, 1M (default: 1M)
    #[arg(long, value_name = "SIZE", value_parser = cli::parse_size)]
    pub buffer_size: Option<usize>,
    /// Durability: none, batch (default), full
    #[arg(long, value_enum, value_name = "MODE")]
    pub sync: Option<Sync>,
    /// Disable APFS copy-on-write clones (macOS)
    #[arg(long)]
    pub no_clone: bool,
    /// Disable copy_file_range kernel copies (Linux)
    #[arg(long)]
    pub no_kernel_copy: bool,

    /// Stop at the first error instead of continuing
    #[arg(long)]
    pub strict: bool,
    /// Show what would be done without writing anything
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
