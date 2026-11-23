//! Helpers shared by the `bcp` and `bmv` binaries (no clap dependency).

use crate::config::{Preserve, ProgressMode};
use crate::types::CopyReport;

/// Parse a size such as `4096`, `256K`, `1M`, `1MiB`, `2GB` (binary units).
///
/// # Errors
/// Malformed number, unknown unit, zero, or overflow.
pub fn parse_size(s: &str) -> Result<usize, String> {
    let t = s.trim();
    let split = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let base: usize = num.parse().map_err(|_| format!("invalid size: '{s}'"))?;
    let mult: usize = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1 << 10,
        "m" | "mb" | "mib" => 1 << 20,
        "g" | "gb" | "gib" => 1 << 30,
        _ => return Err(format!("invalid size unit in '{s}' (use K, M or G)")),
    };
    let n = base.checked_mul(mult).ok_or_else(|| format!("size too large: '{s}'"))?;
    if n == 0 {
        return Err("size must be greater than zero".into());
    }
    Ok(n)
}

/// Parse a `--preserve` list: `mode,ownership,timestamps,xattrs,all`.
///
/// # Errors
/// Unknown attribute names (typos are reported, not ignored).
pub fn parse_preserve(spec: &str) -> Result<Preserve, String> {
    let mut p = Preserve::default();
    for part in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "mode" => p.mode = true,
            "ownership" | "owner" => p.ownership = true,
            "timestamps" | "time" | "times" => p.timestamps = true,
            "xattr" | "xattrs" => p.xattrs = true,
            "all" => p = Preserve::all(),
            other => {
                return Err(format!(
                    "invalid --preserve attribute '{other}' (expected mode, ownership, timestamps, xattrs or all)"
                ));
            }
        }
    }
    Ok(p)
}

/// Choose the display: `--no-progress` > `--quiet` > auto (rich on a TTY).
#[must_use]
pub fn progress_mode(quiet: bool, no_progress: bool) -> ProgressMode {
    if no_progress {
        ProgressMode::Silent
    } else if quiet || !console::user_attended_stderr() {
        ProgressMode::Quiet
    } else {
        ProgressMode::Rich
    }
}

/// Maximum number of messages printed per category before summarising.
const MAX_LISTED: usize = 50;

/// Print warnings, errors and (verbose) a summary to stderr. Returns the
/// process exit code (0 ok, 1 failure, 130 interrupted).
#[must_use]
pub fn finish(tool: &str, report: &CopyReport, verbose: bool) -> i32 {
    print_list(tool, "warning", &report.warnings);
    print_list(tool, "error", &report.errors);
    if report.cancelled {
        eprintln!("{tool}: interrupted — {} file(s) completed, temporary files removed", report.files_copied);
    } else if verbose {
        eprintln!("{}", summary(report));
    }
    report.exit_code()
}

fn print_list(tool: &str, label: &str, items: &[String]) {
    for m in items.iter().take(MAX_LISTED) {
        eprintln!("{tool}: {label}: {m}");
    }
    if items.len() > MAX_LISTED {
        eprintln!("{tool}: … and {} more {label}(s)", items.len() - MAX_LISTED);
    }
}

/// One-line summary of a report.
#[must_use]
pub fn summary(r: &CopyReport) -> String {
    let mut parts = vec![format!("{} file(s), {}", r.files_copied, bytesize::ByteSize(r.bytes_copied))];
    for (n, what) in [
        (r.renamed, "renamed"),
        (r.dirs_created, "dir(s) created"),
        (r.symlinks_created, "symlink(s)"),
        (r.hardlinks_created, "hardlink(s)"),
        (r.fifos_created, "fifo(s)"),
        (r.files_skipped, "skipped"),
        (r.files_failed, "failed"),
    ] {
        if n > 0 {
            parts.push(format!("{n} {what}"));
        }
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("4096"), Ok(4096));
        assert_eq!(parse_size("256K"), Ok(256 * 1024));
        assert_eq!(parse_size("1M"), Ok(1 << 20));
        assert_eq!(parse_size("1MiB"), Ok(1 << 20));
        assert_eq!(parse_size(" 2gb "), Ok(2 << 30));
        assert!(parse_size("0").is_err());
        assert!(parse_size("12X").is_err());
        assert!(parse_size("M").is_err());
        assert!(parse_size("é").is_err());
        assert!(parse_size("99999999999999999999G").is_err());
    }

    #[test]
    fn preserve_lists() {
        assert_eq!(parse_preserve("all"), Ok(Preserve::all()));
        let p = parse_preserve("mode,timestamps").unwrap();
        assert!(p.mode && p.timestamps && !p.xattrs && !p.ownership);
        assert!(parse_preserve("mdoe").is_err(), "typos must be rejected");
    }

    #[test]
    fn exit_codes() {
        let ok = CopyReport::default();
        assert_eq!(ok.exit_code(), 0);
        let failed = CopyReport { files_failed: 1, ..Default::default() };
        assert_eq!(failed.exit_code(), 1);
        let err = CopyReport { errors: vec!["x".into()], ..Default::default() };
        assert_eq!(err.exit_code(), 1);
        let warn = CopyReport { warnings: vec!["x".into()], ..Default::default() };
        assert_eq!(warn.exit_code(), 0);
        let int = CopyReport { cancelled: true, ..Default::default() };
        assert_eq!(int.exit_code(), 130);
    }
}
