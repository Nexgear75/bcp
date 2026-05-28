use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;

/// Copy `src_path` to `dest_path` using an atomic-write pattern:
///
/// 1. Create a temp file in `dest_path.parent()` (same filesystem
///    for atomic rename — avoids EXDEV).
/// 2. Copy all data in chunks via `chunked_copy_with_progress`.
/// 3. Platform-appropriate fsync (F_FULLFSYNC on macOS,
///    `sync_all` on Linux).
/// 4. `persist()` the temp file to `dest_path` — destination is never
///    written directly.
///
/// Returns the total number of bytes copied.
pub fn atomic_copy(
    _src_path: &Path,
    _dest_path: &Path,
    _buffer_size: usize,
    _bytes_copied: &Arc<AtomicU64>,
    _cancelled: &AtomicBool,
) -> anyhow::Result<u64> {
    // Stub — returns 0 to make tests fail
    Ok(0)
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
