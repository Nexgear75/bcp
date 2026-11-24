//! Phase 1 of the pipeline: scan source paths into [`FileEntry`] records,
//! resolving each one's destination path with `cp`-like semantics.

use std::fs::Metadata;
use std::path::{Path, PathBuf};

use anyhow::Context;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

use crate::config::{Config, Dereference, ProgressMode};
use crate::error::CopyError;
use crate::types::{FileEntry, FileKind, ScanWarning};

/// A scanned entry together with its already-resolved destination path.
#[derive(Debug)]
pub struct ScannedEntry {
    pub entry: FileEntry,
    pub dest_path: PathBuf,
}

/// Result of scanning all sources.
#[derive(Debug, Default)]
pub struct ScanResult {
    pub entries: Vec<ScannedEntry>,
    pub warnings: Vec<ScanWarning>,
    pub total_bytes: u64,
}

fn classify(path: &Path, meta: &Metadata) -> FileKind {
    use std::os::unix::fs::FileTypeExt;
    let ft = meta.file_type();
    if ft.is_symlink() {
        match std::fs::read_link(path) {
            Ok(target) => FileKind::Symlink { target },
            Err(_) => FileKind::Other,
        }
    } else if ft.is_dir() {
        FileKind::Directory
    } else if ft.is_file() {
        FileKind::File
    } else if ft.is_fifo() {
        FileKind::Fifo
    } else {
        FileKind::Other
    }
}

/// Whether `dest` is a directory we copy *into*.
///
/// # Errors
/// Several sources need an existing directory; `-T` forbids several sources.
pub fn resolve_dest_is_dir(dest: &Path, sources: usize, config: &Config) -> anyhow::Result<bool> {
    let is_dir = std::fs::metadata(dest).map(|m| m.is_dir()).unwrap_or(false);
    if config.no_target_directory {
        if sources > 1 {
            anyhow::bail!("extra operand: -T allows a single source");
        }
        return Ok(false);
    }
    if sources > 1 && !is_dir {
        anyhow::bail!("target '{}' is not a directory", dest.display());
    }
    Ok(is_dir)
}

fn top_level_meta(source: &Path, config: &Config) -> anyhow::Result<Metadata> {
    let follow = matches!(config.dereference, Dereference::Always | Dereference::CommandLine);
    let r = if follow { std::fs::metadata(source) } else { std::fs::symlink_metadata(source) };
    r.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::Error::new(CopyError::SourceNotFound(source.to_path_buf()))
        } else {
            anyhow::anyhow!("cannot stat '{}': {e}", source.display())
        }
    })
}

/// Canonical form of `p`, even when its tail doesn't exist yet.
fn canonical_lenient(p: &Path) -> PathBuf {
    let mut existing = p.to_path_buf();
    let mut tail = Vec::new();
    loop {
        if let Ok(c) = std::fs::canonicalize(&existing) {
            let mut out = c;
            for t in tail.iter().rev() {
                out.push(t);
            }
            return out;
        }
        match (existing.file_name().map(std::ffi::OsStr::to_os_string), existing.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                existing = if parent.as_os_str().is_empty() { PathBuf::from(".") } else { parent.to_path_buf() };
            }
            _ => return p.to_path_buf(),
        }
    }
}

/// Refuse `cp -r dir dir/sub` (the copy would end up inside itself).
///
/// # Errors
/// When `dest` lies inside `source`.
pub fn check_not_into_itself(source: &Path, dest: &Path) -> anyhow::Result<()> {
    let (Ok(src), dst) = (std::fs::canonicalize(source), canonical_lenient(dest)) else { return Ok(()) };
    if dst.starts_with(&src) {
        anyhow::bail!("cannot copy a directory, '{}', into itself, '{}'", source.display(), dest.display());
    }
    Ok(())
}

/// Scan all `sources` against `dest`, producing resolved entries.
///
/// # Errors
/// Missing source, directory without `-r`, invalid target, copy into itself.
pub fn scan_sources(sources: &[PathBuf], dest: &Path, config: &Config) -> anyhow::Result<ScanResult> {
    if sources.is_empty() {
        anyhow::bail!("missing source operand");
    }
    let dest_dir = resolve_dest_is_dir(dest, sources.len(), config)?;
    let pb = scan_spinner(config);
    let mut out = ScanResult::default();

    for source in sources {
        let name = source.file_name().unwrap_or(source.as_os_str());
        let dest_path = if dest_dir { dest.join(name) } else { dest.to_path_buf() };
        scan_into(source, &dest_path, config, &pb, &mut out)?;
    }

    pb.finish_and_clear();
    Ok(out)
}

/// Scan exactly one source onto an explicit destination root (moves).
///
/// # Errors
/// See [`scan_sources`].
pub fn scan_one(source: &Path, dest_root: &Path, config: &Config) -> anyhow::Result<ScanResult> {
    let pb = scan_spinner(config);
    let mut out = ScanResult::default();
    scan_into(source, dest_root, config, &pb, &mut out)?;
    pb.finish_and_clear();
    Ok(out)
}

fn scan_into(
    source: &Path,
    dest_path: &Path,
    config: &Config,
    pb: &ProgressBar,
    out: &mut ScanResult,
) -> anyhow::Result<()> {
    let meta = top_level_meta(source, config).with_context(|| format!("cannot copy '{}'", source.display()))?;
    let kind = classify(source, &meta);
    match kind {
        FileKind::Directory => {
            if !config.recursive {
                anyhow::bail!("-r not specified; omitting directory '{}'", source.display());
            }
            check_not_into_itself(source, dest_path)?;
            let root = FileEntry { source_path: source.to_path_buf(), relative_path: PathBuf::new(), meta, kind };
            out.entries.push(ScannedEntry { entry: root, dest_path: dest_path.to_path_buf() });
            scan_dir(source, dest_path, config, pb, out);
        }
        FileKind::Other => out.warnings.push(ScanWarning::Unsupported { path: source.to_path_buf() }),
        _ => {
            let entry = FileEntry {
                source_path: source.to_path_buf(),
                relative_path: source.file_name().map(PathBuf::from).unwrap_or_default(),
                meta,
                kind,
            };
            out.total_bytes += entry.size();
            tick(pb, out.entries.len() + 1, out.total_bytes, source);
            out.entries.push(ScannedEntry { entry, dest_path: dest_path.to_path_buf() });
        }
    }
    Ok(())
}

type MetaState = ((), Option<std::io::Result<Metadata>>);

/// Walk a directory's descendants with jwalk. `lstat` calls run inside
/// jwalk's parallel `read_dir` workers, not on the consuming thread.
fn scan_dir(root: &Path, dest_base: &Path, config: &Config, pb: &ProgressBar, out: &mut ScanResult) {
    let follow = config.dereference == Dereference::Always;
    let walker = jwalk::WalkDirGeneric::<MetaState>::new(root)
        .skip_hidden(false)
        .follow_links(follow)
        .min_depth(1)
        .process_read_dir(move |_, _, (), children| {
            for child in children.iter_mut().flatten() {
                let p = child.path();
                let m = if follow { std::fs::metadata(&p) } else { std::fs::symlink_metadata(&p) };
                child.client_state = Some(m);
            }
        });

    for dent in walker {
        let mut dent = match dent {
            Ok(d) => d,
            Err(e) => {
                let path = e.path().map_or_else(|| root.to_path_buf(), Path::to_path_buf);
                out.warnings.push(ScanWarning::Unreadable { path, reason: e.to_string() });
                continue;
            }
        };
        let path = dent.path();
        if let Some(e) = &dent.read_children_error {
            out.warnings.push(ScanWarning::Unreadable { path: path.clone(), reason: e.to_string() });
        }
        let meta = match dent.client_state.take() {
            Some(Ok(m)) => m,
            Some(Err(e)) => {
                out.warnings.push(ScanWarning::Unreadable { path, reason: e.to_string() });
                continue;
            }
            None => match dent.metadata() {
                Ok(m) => m,
                Err(e) => {
                    out.warnings.push(ScanWarning::Unreadable { path, reason: e.to_string() });
                    continue;
                }
            },
        };
        let Ok(rel) = path.strip_prefix(root).map(Path::to_path_buf) else { continue };
        let kind = classify(&path, &meta);
        if kind == FileKind::Other {
            out.warnings.push(ScanWarning::Unsupported { path });
            continue;
        }
        let entry = FileEntry { source_path: path.clone(), relative_path: rel.clone(), meta, kind };
        out.total_bytes += entry.size();
        if out.entries.len().is_multiple_of(64) {
            tick(pb, out.entries.len() + 1, out.total_bytes, &path);
        }
        out.entries.push(ScannedEntry { entry, dest_path: dest_base.join(&rel) });
    }
}

fn scan_spinner(config: &Config) -> ProgressBar {
    if config.progress != ProgressMode::Rich {
        return ProgressBar::hidden();
    }
    let pb = ProgressBar::new_spinner();
    pb.set_draw_target(ProgressDrawTarget::stderr_with_hz(10));
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} {msg} [{elapsed_precise}]")
            .expect("static template")
            .tick_chars("◐◓◑◒ "),
    );
    // Keep spinning even when a directory read is slow (USB disk…).
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb
}

fn tick(pb: &ProgressBar, files: usize, bytes: u64, current: &Path) {
    if pb.is_hidden() {
        return;
    }
    pb.set_message(format!("Scanning… {files} entries ({}) — {}", bytesize::ByteSize(bytes), current.display()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn into_itself_detected() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a");
        std::fs::create_dir_all(a.join("sub")).unwrap();
        assert!(check_not_into_itself(&a, &a.join("sub/a")).is_err());
        assert!(check_not_into_itself(&a, &d.path().join("b")).is_ok());
        // Sibling with a common prefix is fine.
        assert!(check_not_into_itself(&a, &d.path().join("ab")).is_ok());
    }

    #[test]
    fn several_sources_need_existing_dir() {
        let d = tempfile::tempdir().unwrap();
        let cfg = Config::default();
        assert!(resolve_dest_is_dir(&d.path().join("nope"), 2, &cfg).is_err());
        assert!(resolve_dest_is_dir(d.path(), 2, &cfg).unwrap());
        let t = Config { no_target_directory: true, ..Default::default() };
        assert!(!resolve_dest_is_dir(d.path(), 1, &t).unwrap());
    }

    #[test]
    fn walk_reports_metadata_and_fifo() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("r");
        std::fs::create_dir_all(root.join("x/y")).unwrap();
        std::fs::write(root.join("x/y/f"), b"12345").unwrap();
        nix::unistd::mkfifo(&root.join("pipe"), nix::sys::stat::Mode::from_bits_truncate(0o644)).unwrap();
        let cfg = Config { recursive: true, progress: ProgressMode::Silent, ..Default::default() };
        let r = scan_sources(&[root], &d.path().join("out"), &cfg).unwrap();
        assert_eq!(r.total_bytes, 5);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert!(r.entries.iter().any(|e| e.entry.kind == FileKind::Fifo));
        assert_eq!(r.entries.iter().filter(|e| e.entry.kind == FileKind::Directory).count(), 3);
    }
}
