use std::path::PathBuf;

use clap::{CommandFactory, Parser, ValueEnum, error::ErrorKind};
use progress_copy::{Config, Overwrite, Preserve, SyncMode, cli, run_move};

#[derive(Clone, Copy, ValueEnum)]
enum Sync {
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
#[command(name = "bmv", version, about, after_help = "Exit status: 0 success, 1 at least one error, 2 usage error, 130 interrupted.")]
#[allow(clippy::struct_excessive_bools)]
struct Args {
    /// Replace existing destinations
    #[arg(short = 'f', long, overrides_with_all = ["no_clobber", "interactive"])]
    force: bool,
    /// Never replace existing destinations (skip them silently)
    #[arg(short = 'n', long, overrides_with_all = ["force", "interactive"])]
    no_clobber: bool,
    /// Ask before replacing an existing destination
    #[arg(short = 'i', long, overrides_with_all = ["force", "no_clobber"])]
    interactive: bool,
    /// Move only when the source is newer than the destination
    #[arg(short = 'u', long)]
    update: bool,
    /// Keep a backup of each replaced destination (NAME~)
    #[arg(short = 'b', long)]
    backup: bool,
    /// Backup suffix (implies --backup)
    #[arg(short = 'S', long, value_name = "SUFFIX")]
    suffix: Option<String>,

    /// Move all sources into DIRECTORY
    #[arg(short = 't', long, value_name = "DIRECTORY", conflicts_with = "no_target_directory")]
    target_directory: Option<PathBuf>,
    /// Treat DEST as a normal file, never as a directory to move into
    #[arg(short = 'T', long)]
    no_target_directory: bool,

    /// Worker threads for cross-filesystem copies (default: number of CPUs)
    #[arg(short = 'j', long, value_name = "N")]
    threads: Option<usize>,
    /// Read/write buffer size, e.g. 256K, 1M (default: 1M)
    #[arg(long, value_name = "SIZE", value_parser = cli::parse_size)]
    buffer_size: Option<usize>,
    /// Durability for cross-filesystem moves: batch (default) or full
    #[arg(long, value_enum, value_name = "MODE")]
    sync: Option<Sync>,

    /// Stop at the first error instead of continuing
    #[arg(long)]
    strict: bool,
    /// Show what would be done without changing anything
    #[arg(long)]
    dry_run: bool,
    /// Explain what is being done
    #[arg(short = 'v', long)]
    verbose: bool,
    /// Single-line progress
    #[arg(short = 'q', long)]
    quiet: bool,
    /// No progress output at all
    #[arg(long)]
    no_progress: bool,

    /// SOURCE... DEST, or SOURCE... with -t DIRECTORY
    #[arg(required = true, value_name = "PATHS")]
    paths: Vec<PathBuf>,
}

fn main() {
    std::process::exit(real_main());
}

fn real_main() -> i32 {
    let args = Args::parse();

    let mut paths = args.paths;
    let dest = if let Some(dir) = args.target_directory {
        if !dir.is_dir() {
            eprintln!("bmv: target '{}' is not a directory", dir.display());
            return 1;
        }
        dir
    } else {
        if paths.len() < 2 {
            Args::command().error(ErrorKind::MissingRequiredArgument, "missing destination operand").exit();
        }
        paths.pop().expect("len >= 2")
    };

    let config = Config {
        overwrite: if args.force {
            Overwrite::Force
        } else if args.no_clobber {
            Overwrite::Skip
        } else if args.interactive {
            Overwrite::Prompt
        } else {
            Overwrite::Refuse
        },
        update: args.update,
        backup_suffix: if args.backup || args.suffix.is_some() {
            Some(args.suffix.unwrap_or_else(|| "~".into()))
        } else {
            None
        },
        no_target_directory: args.no_target_directory,
        verbose: args.verbose,
        buffer_size: args.buffer_size.unwrap_or(1 << 20),
        recursive: true,
        threads: args.threads,
        dry_run: args.dry_run,
        strict: args.strict,
        sync: match args.sync {
            Some(Sync::Full) => SyncMode::Full,
            _ => SyncMode::Batch,
        },
        preserve: Preserve::all(),
        progress: cli::progress_mode(args.quiet, args.no_progress),
        ..Default::default()
    };

    match run_move(&paths, &dest, &config) {
        Ok(report) => cli::finish("bmv", &report, config.verbose && !config.dry_run),
        Err(e) => {
            eprintln!("bmv: {e:#}");
            1
        }
    }
}
