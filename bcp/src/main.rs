use std::path::PathBuf;

use clap::{CommandFactory, Parser, ValueEnum, error::ErrorKind};
use progress_copy::{Config, Dereference, Overwrite, Preserve, SyncMode, cli, run_copy};

#[derive(Clone, Copy, ValueEnum)]
enum Sync {
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
    about,
    after_help = "Exit status: 0 success, 1 at least one error, 2 usage error, 130 interrupted."
)]
#[allow(clippy::struct_excessive_bools)]
struct Args {
    /// Replace existing destination files
    #[arg(short = 'f', long, overrides_with_all = ["no_clobber", "interactive"])]
    force: bool,
    /// Never replace existing files (skip them silently)
    #[arg(short = 'n', long, overrides_with_all = ["force", "interactive"])]
    no_clobber: bool,
    /// Ask before replacing an existing file
    #[arg(short = 'i', long, overrides_with_all = ["force", "no_clobber"])]
    interactive: bool,
    /// Replace only when the source is newer than the destination
    #[arg(short = 'u', long)]
    update: bool,
    /// Keep a backup of each replaced file (NAME~)
    #[arg(short = 'b', long)]
    backup: bool,
    /// Backup suffix (implies --backup)
    #[arg(short = 'S', long, value_name = "SUFFIX")]
    suffix: Option<String>,

    /// Copy directories recursively
    #[arg(short = 'r', short_alias = 'R', long)]
    recursive: bool,
    /// Same as -R -P --preserve=all
    #[arg(short = 'a', long)]
    archive: bool,
    /// Follow every symlink in the sources
    #[arg(short = 'L', long, overrides_with_all = ["no_dereference", "cmdline_links"])]
    dereference: bool,
    /// Never follow symlinks; copy them as links (default)
    #[arg(short = 'P', long, overrides_with_all = ["dereference", "cmdline_links"])]
    no_dereference: bool,
    /// Follow symlinks given on the command line only
    #[arg(short = 'H', overrides_with_all = ["dereference", "no_dereference"])]
    cmdline_links: bool,

    /// Preserve mode, ownership and timestamps
    #[arg(short = 'p')]
    preserve_default: bool,
    /// Preserve the listed attributes: mode,ownership,timestamps,xattrs,all
    #[arg(long, value_name = "ATTRS", num_args = 0..=1, require_equals = true,
          default_missing_value = "mode,ownership,timestamps", value_parser = cli::parse_preserve)]
    preserve: Option<Preserve>,

    /// Copy all sources into DIRECTORY
    #[arg(short = 't', long, value_name = "DIRECTORY", conflicts_with = "no_target_directory")]
    target_directory: Option<PathBuf>,
    /// Treat DEST as a normal file, never as a directory to copy into
    #[arg(short = 'T', long)]
    no_target_directory: bool,

    /// Worker threads (default: number of CPUs)
    #[arg(short = 'j', long, value_name = "N")]
    threads: Option<usize>,
    /// Read/write buffer size, e.g. 256K, 1M (default: 1M)
    #[arg(long, value_name = "SIZE", value_parser = cli::parse_size)]
    buffer_size: Option<usize>,
    /// Durability: none, batch (default), full
    #[arg(long, value_enum, value_name = "MODE")]
    sync: Option<Sync>,
    /// Disable APFS copy-on-write clones (macOS)
    #[arg(long)]
    no_clone: bool,
    /// Disable copy_file_range kernel copies (Linux)
    #[arg(long)]
    no_kernel_copy: bool,

    /// Stop at the first error instead of continuing
    #[arg(long)]
    strict: bool,
    /// Show what would be done without writing anything
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
            eprintln!("bcp: target '{}' is not a directory", dir.display());
            return 1;
        }
        dir
    } else {
        if paths.len() < 2 {
            Args::command().error(ErrorKind::MissingRequiredArgument, "missing destination operand").exit();
        }
        paths.pop().expect("len >= 2")
    };
    let mut preserve = args.preserve.unwrap_or_default();
    if args.preserve_default {
        let p = Preserve::cp_p();
        preserve = Preserve {
            mode: preserve.mode || p.mode,
            ownership: preserve.ownership || p.ownership,
            timestamps: preserve.timestamps || p.timestamps,
            xattrs: preserve.xattrs,
        };
    }
    if args.archive {
        preserve = Preserve::all();
    }

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
        recursive: args.recursive || args.archive,
        threads: args.threads,
        no_clone: args.no_clone,
        no_kernel_copy: args.no_kernel_copy,
        dry_run: args.dry_run,
        strict: args.strict,
        dereference: if args.dereference {
            Dereference::Always
        } else if args.cmdline_links {
            Dereference::CommandLine
        } else {
            Dereference::Never
        },
        sync: match args.sync {
            Some(Sync::None) => SyncMode::None,
            Some(Sync::Full) => SyncMode::Full,
            Some(Sync::Batch) | None => SyncMode::Batch,
        },
        preserve,
        progress: cli::progress_mode(args.quiet, args.no_progress),
    };

    match run_copy(&paths, &dest, &config) {
        Ok(report) => cli::finish("bcp", &report, config.verbose && !config.dry_run),
        Err(e) => {
            eprintln!("bcp: {e:#}");
            1
        }
    }
}
