mod args;

use clap::{CommandFactory, Parser, error::ErrorKind};
use progress_copy::{Config, Overwrite, Preserve, SyncMode, cli, run_move};

use args::{Args, Sync};

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
