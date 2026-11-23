use clap::Parser;
use progress_copy::{run_move, Config, ProgressMode};
use std::path::PathBuf;

/// Move files with progress bars — a modern replacement for `mv`.
#[derive(Parser)]
#[command(name = "bmv", version, about = "Move files with progress bars")]
struct Args {
    /// Force overwrite of destination file
    #[arg(short = 'f', long, help = "Force overwrite of destination file")]
    force: bool,

    /// Do not overwrite an existing file
    #[arg(short = 'n', long, help = "Do not overwrite an existing file")]
    no_clobber: bool,

    /// Verbose output
    #[arg(short = 'v', long, help = "Verbose output")]
    verbose: bool,

    /// Number of worker threads for cross-device copy (default: auto)
    #[arg(short = 't', long, value_name = "N", help = "Worker threads (default: auto)")]
    threads: Option<usize>,

    /// Read/write buffer size for cross-device copy, e.g. 1M, 256K
    #[arg(short = 'b', long, value_name = "SIZE", help = "Buffer size, e.g. 1M, 256K")]
    buffer: Option<String>,

    /// Abort on the first error instead of skipping
    #[arg(long, help = "Abort on first error instead of skipping")]
    strict: bool,

    /// Scan and plan only; perform no changes
    #[arg(long, help = "Scan and plan only; perform no changes")]
    dry_run: bool,

    /// Minimal single-line progress
    #[arg(short = 'q', long, help = "Minimal single-line progress")]
    quiet: bool,

    /// Disable progress output entirely
    #[arg(long, help = "Disable progress output entirely")]
    no_progress: bool,

    /// Source paths followed by the destination (last argument)
    #[arg(required = true, num_args = 2.., value_name = "SOURCES... DEST")]
    paths: Vec<String>,
}

fn parse_size(s: &str) -> Result<usize, String> {
    let t = s.trim();
    let (num, mult) = match t.chars().last() {
        Some('K') | Some('k') => (&t[..t.len() - 1], 1024usize),
        Some('M') | Some('m') => (&t[..t.len() - 1], 1024 * 1024),
        Some('G') | Some('g') => (&t[..t.len() - 1], 1024 * 1024 * 1024),
        Some(c) if c.is_ascii_digit() => (t, 1usize),
        _ => return Err(format!("invalid size: {s}")),
    };
    let base: usize = num.trim().parse().map_err(|_| format!("invalid size: {s}"))?;
    Ok(base.saturating_mul(mult).max(1))
}

fn main() {
    let args = Args::parse();

    let mut paths = args.paths;
    let destination = PathBuf::from(paths.pop().expect("clap guarantees >= 2 paths"));
    let sources: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();

    let buffer_size = match args.buffer.as_deref().map(parse_size) {
        Some(Ok(n)) => n,
        Some(Err(e)) => {
            eprintln!("bmv: {e}");
            std::process::exit(2);
        }
        None => 1024 * 1024,
    };

    let progress = if args.no_progress {
        ProgressMode::Silent
    } else if args.quiet || !console::user_attended_stderr() {
        // Explicit --quiet, or auto-degrade when stderr is not a TTY.
        ProgressMode::Quiet
    } else {
        ProgressMode::Rich
    };

    let config = Config {
        source: sources.first().cloned().unwrap_or_default(),
        destination: destination.clone(),
        force: args.force,
        no_clobber: args.no_clobber,
        verbose: args.verbose,
        buffer_size,
        // Moving a directory tree is implicitly recursive (rename or deep copy).
        recursive: true,
        threads: args.threads,
        no_clone: false,
        no_kernel_copy: false,
        dry_run: args.dry_run,
        strict: args.strict,
        preserve: progress_copy::Preserve::all(),
        progress,
    };

    match run_move(&sources, &destination, &config) {
        Ok(_) => std::process::exit(0),
        Err(e) => {
            eprintln!("bmv: {e:#}");
            std::process::exit(1);
        }
    }
}
