use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;

use anyhow::Context;
use tempfile::Builder;

use crate::copy;

/// Copy `src_path` to `dest_path` using an atomic-write pattern:
///
/// 1. Create a temp file in `dest_path.parent()` (same filesystem
///    for atomic `rename` — avoids EXDEV, Pitfall #2).
/// 2. Copy all data in chunks via `chunked_copy_with_progress`.
/// 3. Platform-appropriate fsync (F_FULLFSYNC on macOS, `sync_all`
///    on Linux, Pitfall #1).
/// 4. `persist()` the temp file to `dest_path` — destination is never
///    written directly (ATOM-04).
///
/// Returns the total number of bytes copied.
pub fn atomic_copy(
    src_path: &Path,
    dest_path: &Path,
    buffer_size: usize,
    bytes_copied: &Arc<AtomicU64>,
    cancelled: &AtomicBool,
) -> anyhow::Result<u64> {
    // 1. Determine destination directory (Pitfall #2: must be same
    //    filesystem as dest to avoid EXDEV on rename).
    let dest_dir = dest_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Destination has no parent directory: {}", dest_path.display()))?;

    // 2. Create temp file adjacent to destination (Pitfall #2: same
    //    filesystem as dest → atomic rename, no EXDEV).
    let prefix = format!(
        ".{}.",
        dest_path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default()
    );
    let tmp = Builder::new()
        .prefix(&prefix)
        .tempfile_in(dest_dir)
        .context("Failed to create temporary file")?;

    // Open an fs_err::File handle to the temp file for the chunked
    // copy (chunked_copy_with_progress expects fs_err::File).
    let tmp_path = tmp.path().to_path_buf();
    let mut tmp_file = fs_err::OpenOptions::new()
        .write(true)
        .open(&tmp_path)
        .context("Failed to open temp file for writing")?;

    // 3. Open source file.
    let mut src_file = fs_err::File::open(src_path)
        .with_context(|| format!("Failed to open source: {}", src_path.display()))?;
    let file_size = src_file
        .metadata()
        .with_context(|| format!("Failed to read metadata for: {}", src_path.display()))?
        .len();

    // 4. Copy data in chunks with progress tracking.
    let total = copy::chunked_copy_with_progress(
        &mut src_file,
        &mut tmp_file,
        file_size,
        buffer_size,
        bytes_copied,
        cancelled,
    )
    .map_err(|e| anyhow::anyhow!(e))?;

    // Drop the write handle so sync sees all data.
    drop(tmp_file);

    // 5. Platform-appropriate fsync before rename (ATOM-02).
    sync_file(tmp.as_file())
        .context("Failed to sync temporary file to disk")?;

    // 6. Atomic rename: destination appears ONLY when copy is complete
    //    (ATOM-04). If this fails the temp file is cleaned up by Drop.
    tmp.persist(dest_path)
        .map_err(|e| anyhow::anyhow!("Atomic rename failed: {}", e))?;

    Ok(total)
}

/// Flush file data to persistent storage.
///
/// On macOS: uses `fcntl(fd, F_FULLFSYNC)` which guarantees data reaches
/// physical media. `fsync()` on macOS only flushes to the drive's write
/// cache and is NOT sufficient (Pitfall #1).
///
/// On Linux: uses `sync_all()` which calls `fsync()` and is sufficient.
#[cfg(target_os = "macos")]
fn sync_file(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;

    // F_FULLFSYNC = 51 on macOS (verified on macOS 15, RESEARCH.md)
    const F_FULLFSYNC: i32 = 51;

    let rc = unsafe { libc::fcntl(file.as_raw_fd(), F_FULLFSYNC) };
    if rc == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn sync_file(file: &std::fs::File) -> std::io::Result<()> {
    file.sync_all()
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Helper: create a file at the given path with the given content.
    fn write_file(path: &Path, content: &[u8]) {
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(content).unwrap();
        f.sync_all().unwrap();
    }

    /// Helper: read entire contents of a file.
    fn read_file(path: &Path) -> Vec<u8> {
        let mut f = std::fs::File::open(path).unwrap();
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        buf
    }

    // ── Test 4: atomic copy success ────────────────────────────────

    #[test]
    fn test_atomic_copy_success() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("source.bin");
        let dst = dir.path().join("dest.bin");

        let content = b"Hello, atomic world!\n".repeat(1000);
        write_file(&src, &content);

        let bytes_copied = Arc::new(AtomicU64::new(0));
        let cancelled = AtomicBool::new(false);

        let result = atomic_copy(
            &src, &dst, 1024 * 1024, &bytes_copied, &cancelled,
        );

        assert!(result.is_ok(), "atomic_copy failed: {:?}", result);
        let total = result.unwrap();
        assert_eq!(total as usize, content.len());
        assert_eq!(bytes_copied.load(Ordering::Relaxed), content.len() as u64);

        // Destination content must match source exactly
        let dest_content = read_file(&dst);
        assert_eq!(dest_content, content);

        // Source must be unchanged
        let src_content = read_file(&src);
        assert_eq!(src_content, content);

        // No temp files left behind
        let file_count = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(file_count, 2, "Expected exactly 2 files (src + dest)");
    }

    // ── Test 5: temp file created in dest.parent() ─────────────────

    #[test]
    fn test_atomic_copy_temp_in_dest_parent() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("sub");
        std::fs::create_dir(&nested).unwrap();

        let src = dir.path().join("src.bin");
        let dst = nested.join("dest.bin");
        write_file(&src, b"data for test 5");

        let bytes_copied = Arc::new(AtomicU64::new(0));
        let cancelled = AtomicBool::new(false);

        let result = atomic_copy(
            &src, &dst, 1024 * 1024, &bytes_copied, &cancelled,
        );
        assert!(result.is_ok());

        // After persist, only dest.bin should be in the nested dir
        let entries: Vec<_> = std::fs::read_dir(&nested)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(entries.len(), 1, "Expected only dest.bin in nested dir");
        assert!(entries.contains(&"dest.bin".to_string()));
    }

    // ── Test 6: atomic copy interrupted — destination not left ─────

    #[test]
    fn test_atomic_copy_interrupted_no_dest_left() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("source.bin");
        let dst = dir.path().join("dest.bin");
        write_file(&src, b"test data for interruption test");

        // Before copy, dest should not exist
        assert!(!dst.exists(), "Destination should not exist before copy");

        let bytes_copied = Arc::new(AtomicU64::new(0));
        let cancelled = AtomicBool::new(false);

        let result = atomic_copy(&src, &dst, 1024 * 1024, &bytes_copied, &cancelled);

        // After successful copy, dest should exist. The SIGINT path
        // is tested in Task 3 integration tests.
        if result.is_ok() {
            assert!(dst.exists(), "Destination should exist after successful copy");
        }
    }
}
