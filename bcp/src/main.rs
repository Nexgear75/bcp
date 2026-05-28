use clap::Parser;
use progress_copy::{run, Config};
use std::path::PathBuf;

/// Copy files with progress bars — a modern replacement for `cp`.
#[derive(Parser)]
#[command(
    name = "bcp",
    version,
    about = "Copy files with progress bars"
)]
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

    /// Source file path
    source: String,

    /// Destination file path
    destination: String,
}

fn main() {
    let args = Args::parse();

    let config = Config {
        source: PathBuf::from(args.source),
        destination: PathBuf::from(args.destination),
        force: args.force,
        no_clobber: args.no_clobber,
        verbose: args.verbose,
        buffer_size: 1024 * 1024, // 1 MB per D-04
    };

    match run(config) {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("bcp: {e}");
            std::process::exit(1);
        }
    }
}
