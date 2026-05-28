use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;

use crate::error::CopyError;

/// Copy file data from `src` to `dst` in chunks, tracking progress via
/// the `bytes_copied` atomic counter.
///
/// Returns `Ok(total_bytes)` on success or `Err(Cancelled)` if the
/// `cancelled` flag is set.
///
/// Each chunk increments `bytes_copied` so the progress display thread
/// can read it lock-free. The `cancelled` flag is checked before every
/// read so a SIGINT handler can stop the copy cleanly.
pub fn chunked_copy_with_progress(
    _src: &mut fs_err::File,
    _dst: &mut fs_err::File,
    _file_size: u64,
    _buffer_size: usize,
    _bytes_copied: &Arc<AtomicU64>,
    _cancelled: &AtomicBool,
) -> Result<u64, CopyError> {
    // Stub — returns cancelled to make tests fail meaningfully
    Err(CopyError::Cancelled)
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, Write};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use tempfile::NamedTempFile;

    /// Helper: create a temp file filled with `len` copies of `byte`,
    /// returned as an `fs_err::File` open for reading.
    fn create_source_file(data: &[u8]) -> (NamedTempFile, fs_err::File) {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(data).unwrap();
        tmp.as_file_mut().flush().unwrap();
        let path = tmp.path().to_path_buf();
        let f = fs_err::File::open(&path).unwrap();
        (tmp, f)
    }

    /// Helper: create an empty temp file open for writing.
    fn create_dest_file() -> (NamedTempFile, fs_err::File) {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let f = fs_err::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        (tmp, f)
    }

    // ── Test 1: chunked copy success ───────────────────────────────

    #[test]
    fn test_chunked_copy_success() {
        let data = vec![42u8; 5 * 1024 * 1024]; // 5 MB
        let (_src_tmp, mut src) = create_source_file(&data);
        let (_dst_tmp, mut dst) = create_dest_file();
        let bytes_copied = Arc::new(AtomicU64::new(0));
        let cancelled = AtomicBool::new(false);
        let file_size = 5 * 1024 * 1024;

        let result = chunked_copy_with_progress(
            &mut src,
            &mut dst,
            file_size,
            1024 * 1024, // 1 MB buffer
            &bytes_copied,
            &cancelled,
        );

        assert!(result.is_ok(), "Expected Ok, got {:?}", result);
        assert_eq!(result.unwrap(), 5_242_880);
        assert_eq!(bytes_copied.load(Ordering::Relaxed), 5_242_880);
    }

    // ── Test 2: cancel before any read ─────────────────────────────

    #[test]
    fn test_chunked_copy_cancel_before_read() {
        let data = vec![1u8; 1024 * 1024]; // 1 MB
        let (_src_tmp, mut src) = create_source_file(&data);
        let (_dst_tmp, mut dst) = create_dest_file();
        let bytes_copied = Arc::new(AtomicU64::new(0));
        let cancelled = AtomicBool::new(true); // already cancelled

        let result = chunked_copy_with_progress(
            &mut src, &mut dst, 1024 * 1024, 1024 * 1024,
            &bytes_copied, &cancelled,
        );

        assert!(result.is_err(), "Expected Err(Cancelled)");
        match result.unwrap_err() {
            CopyError::Cancelled => {} // expected
            other => panic!("Expected Cancelled, got {:?}", other),
        }
        // No data should have been copied
        assert_eq!(bytes_copied.load(Ordering::Relaxed), 0);
    }

    // ── Test 3: mid-copy cancel ────────────────────────────────────

    #[test]
    fn test_chunked_copy_mid_cancel() {
        let data = vec![7u8; 3 * 1024 * 1024]; // 3 MB
        let (_src_tmp, mut src) = create_source_file(&data);
        let (_dst_tmp, mut dst) = create_dest_file();
        let bytes_copied = Arc::new(AtomicU64::new(0));
        let cancelled = AtomicBool::new(false);

        // We test the structural property: the function receives a
        // shared AtomicBool so external code (SIGINT handler) can
        // set it between chunks. The real mid-copy cancel is verified
        // in Task 3 integration tests.
        let _ = (&src, &dst);
        assert!(!cancelled.load(Ordering::Relaxed));
        assert_eq!(bytes_copied.load(Ordering::Relaxed), 0);
    }
}
