//! Build helpers, run with `cargo xtask <command>`.
//!
//! - `assets <DIR>`: write shell completions (`DIR/completions`) and man
//!   pages (`DIR/man`) for `bcp` and `bmv`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::CommandFactory;
use clap_complete::Shell;

#[allow(dead_code)]
#[path = "../../bcp/src/args.rs"]
mod bcp_args;
#[allow(dead_code)]
#[path = "../../bmv/src/args.rs"]
mod bmv_args;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [cmd, dir] if cmd == "assets" => match assets(Path::new(dir)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("xtask: {e}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("usage: cargo xtask assets <DIR>");
            ExitCode::from(2)
        }
    }
}

fn assets(out: &Path) -> std::io::Result<()> {
    let completions = out.join("completions");
    let man = out.join("man");
    std::fs::create_dir_all(&completions)?;
    std::fs::create_dir_all(&man)?;
    for mut cmd in [bcp_args::Args::command(), bmv_args::Args::command()] {
        let name = cmd.get_name().to_string();
        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
            clap_complete::generate_to(shell, &mut cmd, &name, &completions)?;
        }
        let mut page = Vec::new();
        clap_mangen::Man::new(cmd.clone()).render(&mut page)?;
        let path: PathBuf = man.join(format!("{name}.1"));
        std::fs::write(&path, page)?;
    }
    println!("assets written to {}", out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_definitions_are_valid() {
        bcp_args::Args::command().debug_assert();
        bmv_args::Args::command().debug_assert();
    }

    #[test]
    fn assets_are_generated() {
        let d = tempfile::tempdir().unwrap();
        assets(d.path()).unwrap();
        for f in [
            "completions/_bcp",
            "completions/bcp.bash",
            "completions/bcp.fish",
            "completions/_bmv",
            "completions/bmv.bash",
            "completions/bmv.fish",
            "man/bcp.1",
            "man/bmv.1",
        ] {
            let p = d.path().join(f);
            assert!(std::fs::metadata(&p).unwrap().len() > 0, "{f} is empty");
        }
        let man = std::fs::read_to_string(d.path().join("man/bmv.1")).unwrap();
        assert!(man.contains("Move (rename) files"), "man page uses the bmv description");
    }
}
