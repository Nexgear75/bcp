//! Phase 1 of the pipeline: scan source paths into [`FileEntry`] records,
//! resolving each one's destination path with `cp`-like semantics.

use std::fs::Metadata;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::Context;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

use crate::config::Config;
use crate::error::CopyError;
use crate::types::{FileEntry, FileKind, ScanWarning};

/// A scanned entry together with its already-resolved destination path.
pub struct ScannedEntry {
    pub entry: FileEntry,
    pub dest_path: PathBuf,
}

/// Result of scanning all sources.
pub struct ScanResult {
    pub entries: Vec<ScannedEntry>,
    pub warnings: Vec<ScanWarning>,
    pub total_bytes: u64,
}

fn classify(path: &Path, meta: &Metadata) -> FileKind {
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
    } else {
        FileKind::Other
    }
}

fn make_entry(source_path: PathBuf, relative_path: PathBuf, meta: &Metadata, kind: FileKind) -> FileEntry {
    FileEntry {
        size: if matches!(kind, FileKind::File) { meta.len() } else { 0 },
        device_id: meta.dev(),
        inode: meta.ino(),
        kind,
        source_path,
        relative_path,
    }
}

/// Decide whether `dest` should be treated as a directory we copy *into*.
fn dest_is_dir(dest: &Path, multiple_sources: bool) -> bool {
    match std::fs::metadata(dest) {
        Ok(m) => m.is_dir(),
        // Non-existent dest with multiple sources is an error handled by
        // the caller; for a single source it means "dest is the new name".
        Err(_) => multiple_sources,
    }
}

/// Scan all `sources` against `dest`, producing resolved entries.
pub fn scan_sources(sources: &[PathBuf], dest: &Path, config: &Config) -> anyhow::Result<ScanResult> {
    if sources.is_empty() {
        anyhow::bail!("no source paths given");
    }

    let multiple = sources.len() > 1;
    let dest_dir = dest_is_dir(dest, multiple);

    let pb = scan_spinner(config);

    let mut entries: Vec<ScannedEntry> = Vec::new();
    let mut warnings: Vec<ScanWarning> = Vec::new();
    let mut total_bytes: u64 = 0;

    for source in sources {
        let meta = std::fs::symlink_metadata(source)
            .map_err(|_| CopyError::SourceNotFound(source.clone()))
            .with_context(|| format!("Source not found: {}", source.display()))?;
        let kind = classify(source, &meta);

        match kind {
            FileKind::Directory => {
                if !config.recursive {
                    anyhow::bail!(
                        "-r not specified; omitting directory '{}'",
                        source.display()
                    );
                }
                // `cp -r dir dest`: into dest/<dirname> if dest is a dir,
                // else dest itself becomes the copied root.
                let dir_name = source.file_name().unwrap_or(source.as_os_str());
                let dest_base = if dest_dir { dest.join(dir_name) } else { dest.to_path_buf() };
                scan_dir(source, &dest_base, &pb, &mut entries, &mut warnings, &mut total_bytes);
            }
            FileKind::File | FileKind::Symlink { .. } => {
                let file_name = source.file_name().unwrap_or(source.as_os_str());
                let dest_path = if dest_dir { dest.join(file_name) } else { dest.to_path_buf() };
                let rel = PathBuf::from(file_name);
                total_bytes += if matches!(kind, FileKind::File) { meta.len() } else { 0 };
                let entry = make_entry(source.clone(), rel, &meta, kind);
                tick(&pb, entries.len() + 1, total_bytes, source);
                entries.push(ScannedEntry { entry, dest_path });
            }
            FileKind::Other => {
                warnings.push(ScanWarning::Unsupported { path: source.clone() });
            }
        }
    }

    pb.finish_and_clear();
    Ok(ScanResult { entries, warnings, total_bytes })
}

/// Scan exactly one source onto an explicit destination root.
///
/// Unlike [`scan_sources`], the caller has already resolved the final
/// destination path (`dest_root`), so a file maps straight to it and a
/// directory's contents are rooted there. Used by the cross-device move
/// path where the destination name is fixed up front.
pub fn scan_one(source: &Path, dest_root: &Path, config: &Config) -> anyhow::Result<ScanResult> {
    let meta = std::fs::symlink_metadata(source)
        .map_err(|_| CopyError::SourceNotFound(source.to_path_buf()))
        .with_context(|| format!("Source not found: {}", source.display()))?;
    let kind = classify(source, &meta);

    let pb = scan_spinner(config);
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let mut total_bytes = 0u64;

    match kind {
        FileKind::Directory => {
            if !config.recursive {
                anyhow::bail!("-r not specified; omitting directory '{}'", source.display());
            }
            scan_dir(source, dest_root, &pb, &mut entries, &mut warnings, &mut total_bytes);
        }
        FileKind::File | FileKind::Symlink { .. } => {
            total_bytes += if matches!(kind, FileKind::File) { meta.len() } else { 0 };
            let entry = make_entry(source.to_path_buf(), PathBuf::new(), &meta, kind);
            entries.push(ScannedEntry { entry, dest_path: dest_root.to_path_buf() });
        }
        FileKind::Other => warnings.push(ScanWarning::Unsupported { path: source.to_path_buf() }),
    }

    pb.finish_and_clear();
    Ok(ScanResult { entries, warnings, total_bytes })
}

/// Walk a single directory source with jwalk, appending resolved entries.
fn scan_dir(
    root: &Path,
    dest_base: &Path,
    pb: &ProgressBar,
    entries: &mut Vec<ScannedEntry>,
    warnings: &mut Vec<ScanWarning>,
    total_bytes: &mut u64,
) {
    for dent in jwalk::WalkDir::new(root).skip_hidden(false).follow_links(false) {
        let dent = match dent {
            Ok(d) => d,
            Err(e) => {
                warnings.push(ScanWarning::Unreadable {
                    path: root.to_path_buf(),
                    reason: e.to_string(),
                });
                continue;
            }
        };
        let path = dent.path();
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                warnings.push(ScanWarning::Unreadable { path: path.clone(), reason: e.to_string() });
                continue;
            }
        };
        let rel = match path.strip_prefix(root) {
            Ok(r) => r.to_path_buf(),
            Err(_) => continue,
        };
        // The root itself (rel == "") is the dest_base directory.
        let dest_path = if rel.as_os_str().is_empty() {
            dest_base.to_path_buf()
        } else {
            dest_base.join(&rel)
        };

        let kind = classify(&path, &meta);
        if matches!(kind, FileKind::Other) {
            warnings.push(ScanWarning::Unsupported { path: path.clone() });
            continue;
        }
        if matches!(kind, FileKind::File) {
            *total_bytes += meta.len();
        }
        let entry = make_entry(path.clone(), rel, &meta, kind);
        tick(pb, entries.len() + 1, *total_bytes, &path);
        entries.push(ScannedEntry { entry, dest_path });
    }
}

fn scan_spinner(config: &Config) -> ProgressBar {
    if config.progress == crate::config::ProgressMode::Silent {
        return ProgressBar::hidden();
    }
    let pb = ProgressBar::new_spinner();
    pb.set_draw_target(ProgressDrawTarget::stderr_with_hz(10));
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} {msg} [{elapsed_precise}]")
            .unwrap()
            .tick_chars("◐◓◑◒ "),
    );
    // Animation continue : le spinner tourne même quand une lecture de
    // répertoire est lente (ex. disque externe USB), évitant l'impression
    // de « figé » pendant le pré-scan.
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb
}

fn tick(pb: &ProgressBar, files: usize, bytes: u64, current: &Path) {
    pb.set_message(format!(
        "Scanning… {} files ({}) — {}",
        files,
        bytesize::ByteSize(bytes),
        current.display()
    ));
}
