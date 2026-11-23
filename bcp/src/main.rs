use clap::Parser;
use progress_copy::{run_copy, Config, Preserve, ProgressMode};
use std::path::PathBuf;

/// Copy files with progress bars — a modern replacement for `cp`.
#[derive(Parser)]
#[command(name = "bcp", version, about = "Copy files with progress bars")]
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

    /// Copy directories recursively
    #[arg(short = 'r', long, help = "Copy directories recursively")]
    recursive: bool,

    /// Number of worker threads (default: auto)
    #[arg(short = 't', long, value_name = "N", help = "Worker threads (default: auto)")]
    threads: Option<usize>,

    /// Read/write buffer size, e.g. 1M, 256K (default: 1M)
    #[arg(short = 'b', long, value_name = "SIZE", help = "Buffer size, e.g. 1M, 256K")]
    buffer: Option<String>,

    /// Preserve metadata: mode,timestamps,xattrs,all
    #[arg(short = 'p', long, value_name = "ATTRS", num_args = 0..=1, default_missing_value = "all",
          help = "Preserve metadata (mode,timestamps,xattrs,all)")]
    preserve: Option<String>,

    /// Disable macOS clonefile (copy-on-write)
    #[arg(long, help = "Disable macOS clonefile (copy-on-write)")]
    no_clone: bool,

    /// Disable Linux copy_file_range (kernel copy)
    #[arg(long, help = "Disable Linux copy_file_range (kernel copy)")]
    no_kernel_copy: bool,

    /// Abort on the first error instead of skipping
    #[arg(long, help = "Abort on first error instead of skipping")]
    strict: bool,

    /// Scan and plan only; perform no writes
    #[arg(long, help = "Scan and plan only; perform no writes")]
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

fn parse_preserve(spec: &str) -> Preserve {
    if spec.eq_ignore_ascii_case("all") {
        return Preserve::all();
    }
    let mut p = Preserve::default();
    for part in spec.split(',') {
        match part.trim().to_lowercase().as_str() {
            "mode" => p.mode = true,
            "timestamps" | "time" => p.timestamps = true,
            "xattrs" | "xattr" => p.xattrs = true,
            "all" => p = Preserve::all(),
            _ => {}
        }
    }
    p
}

fn main() {
    let args = Args::parse();

    // Split positionals: all but the last are sources, the last is dest.
    let mut paths = args.paths;
    let destination = PathBuf::from(paths.pop().expect("clap guarantees >= 2 paths"));
    let sources: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();

    let buffer_size = match args.buffer.as_deref().map(parse_size) {
        Some(Ok(n)) => n,
        Some(Err(e)) => {
            eprintln!("bcp: {e}");
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
        destination,
        force: args.force,
        no_clobber: args.no_clobber,
        verbose: args.verbose,
        buffer_size,
        recursive: args.recursive,
        threads: args.threads,
        no_clone: args.no_clone,
        no_kernel_copy: args.no_kernel_copy,
        dry_run: args.dry_run,
        strict: args.strict,
        preserve: args.preserve.as_deref().map(parse_preserve).unwrap_or_default(),
        progress,
    };

    match run_copy(&sources, &config.destination, &config) {
        Ok(_) => std::process::exit(0),
        Err(e) => {
            eprintln!("bcp: {e:#}");
            std::process::exit(1);
        }
    }
}
