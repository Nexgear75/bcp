//! Integration tests for `progress_copy::copy::chunked_copy_with_progress`.
//!
//! Covers TEST-01: chunked copy success, multi-chunk copy, cancellation
//! before/after first chunk, and empty files.

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use progress_copy::copy::chunked_copy_with_progress;
use progress_copy::error::CopyError;
use tempfile::NamedTempFile;

// ── Helpers ────────────────────────────────────────────────────────

/// Create a temp file filled with `data`, returning the `NamedTempFile`
/// (to keep it alive) and an `fs_err::File` open for reading.
fn create_source_file(data: &[u8]) -> (NamedTempFile, fs_err::File) {
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(data).unwrap();
    tmp.as_file_mut().flush().unwrap();
    let path = tmp.path().to_path_buf();
    let f = fs_err::File::open(&path).unwrap();
    (tmp, f)
}

/// Create an empty temp file open for writing, returning the
/// `NamedTempFile` (to keep it alive) and an `fs_err::File`.
fn create_dest_file() -> (NamedTempFile, fs_err::File) {
    let tmp = NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    let f = fs_err::OpenOptions::new().write(true).truncate(true).open(&path).unwrap();
    (tmp, f)
}

// ── Test: full file copy ───────────────────────────────────────────

#[test]
fn test_chunked_copy_full_file() {
    let data = vec![0xABu8; 100 * 1024]; // 100 KB
    let (_src_tmp, mut src) = create_source_file(&data);
    let (_dst_tmp, mut dst) = create_dest_file();
    let bytes_copied = Arc::new(AtomicU64::new(0));
    let cancelled = AtomicBool::new(false);

    let result = chunked_copy_with_progress(&mut src, &mut dst, data.len() as u64, 4096, &bytes_copied, &cancelled);

    assert!(result.is_ok(), "Expected Ok, got {:?}", result.err());
    let total = result.unwrap();
    assert_eq!(total, 100 * 1024);
    assert_eq!(bytes_copied.load(Ordering::Relaxed), 100 * 1024);
}

// ── Test: file larger than buffer (multi-chunk) ────────────────────

#[test]
fn test_chunked_copy_larger_than_buffer() {
    let data = vec![0x42u8; 16384]; // 16 KB
    let (_src_tmp, mut src) = create_source_file(&data);
    let (_dst_tmp, mut dst) = create_dest_file();
    let bytes_copied = Arc::new(AtomicU64::new(0));
    let cancelled = AtomicBool::new(false);

    // buffer_size = 4096 → 4 chunks (16 KB / 4 KB)
    let result = chunked_copy_with_progress(&mut src, &mut dst, data.len() as u64, 4096, &bytes_copied, &cancelled);

    assert!(result.is_ok(), "Multi-chunk copy should succeed");
    assert_eq!(result.unwrap(), 16384);
    // AtomicU64 counter must reflect the total after all chunks
    assert_eq!(bytes_copied.load(Ordering::Relaxed), 16384);
}

// ── Test: cancellation before any read ─────────────────────────────

#[test]
fn test_chunked_copy_cancel_before_start() {
    let data = vec![7u8; 8192];
    let (_src_tmp, mut src) = create_source_file(&data);
    let (_dst_tmp, mut dst) = create_dest_file();
    let bytes_copied = Arc::new(AtomicU64::new(0));
    let cancelled = AtomicBool::new(true); // already cancelled

    let result = chunked_copy_with_progress(&mut src, &mut dst, data.len() as u64, 4096, &bytes_copied, &cancelled);

    assert!(result.is_err(), "Expected Err(Cancelled)");
    match result.unwrap_err() {
        CopyError::Cancelled => {} // expected
        other => panic!("Expected CopyError::Cancelled, got {:?}", other),
    }
    // No data should have been copied
    assert_eq!(bytes_copied.load(Ordering::Relaxed), 0);
}

// ── Test: mid-copy cancellation (after first chunk) ────────────────

/// Reader that raises the cancel flag right after serving its first chunk,
/// making the mid-copy cancellation deterministic (no timing race).
struct CancelAfterFirstRead<'a, R> {
    inner: R,
    flag: &'a AtomicBool,
}

impl<R: std::io::Read> std::io::Read for CancelAfterFirstRead<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.flag.store(true, Ordering::Relaxed);
        Ok(n)
    }
}

#[test]
fn test_chunked_copy_cancel_after_first_chunk() {
    let data = vec![0x55u8; 16384]; // 16 KB
    let (_src_tmp, src) = create_source_file(&data);
    let (_dst_tmp, mut dst) = create_dest_file();
    let bytes_copied = Arc::new(AtomicU64::new(0));
    let cancelled = AtomicBool::new(false);
    let mut src = CancelAfterFirstRead { inner: src, flag: &cancelled };

    let result = chunked_copy_with_progress(&mut src, &mut dst, data.len() as u64, 8, &bytes_copied, &cancelled);

    match result {
        Err(CopyError::Cancelled) => {}
        other => panic!("Expected CopyError::Cancelled, got {other:?}"),
    }
    assert_eq!(bytes_copied.load(Ordering::Relaxed), 8, "exactly one 8-byte chunk before stopping");
}

// ── Test: empty file copy ──────────────────────────────────────────

#[test]
fn test_chunked_copy_empty_file() {
    let data: [u8; 0] = [];
    let (_src_tmp, mut src) = create_source_file(&data);
    let (_dst_tmp, mut dst) = create_dest_file();
    let bytes_copied = Arc::new(AtomicU64::new(0));
    let cancelled = AtomicBool::new(false);

    let result = chunked_copy_with_progress(
        &mut src,
        &mut dst,
        0, // file_size = 0
        4096,
        &bytes_copied,
        &cancelled,
    );

    assert!(result.is_ok(), "Empty file copy should succeed: {:?}", result.err());
    assert_eq!(result.unwrap(), 0, "0 bytes should be copied");
    assert_eq!(bytes_copied.load(Ordering::Relaxed), 0, "Counter must be 0");
}
