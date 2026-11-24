//! Integration tests for `progress_copy::atomic::atomic_copy`.
//!
//! Covers TEST-01: atomic copy success, cancellation, temp file cleanup,
//! and error paths.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use progress_copy::atomic::atomic_copy;
use tempfile::TempDir;

// ── Helpers ────────────────────────────────────────────────────────

/// Create a file with deterministic but non-zero content.
/// Each byte = (position % 256) as u8, so every position maps to a
/// unique value — no accidental all-zeros or repeating patterns.
fn create_test_file(dir: &Path, name: &str, size: usize) -> PathBuf {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    let content: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
    f.write_all(&content).unwrap();
    f.sync_all().unwrap();
    path
}

/// Read entire file contents into a Vec<u8>.
fn read_file(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()))
}

// ── Test: successful atomic copy ───────────────────────────────────

#[test]
fn test_atomic_copy_success() {
    let dir = TempDir::new().unwrap();
    let src = create_test_file(dir.path(), "source.bin", 100 * 1024); // 100 KB
    let dst = dir.path().join("dest.bin");

    assert!(src.exists(), "Source must exist before copy");
    assert!(!dst.exists(), "Destination must not exist before copy");

    let bytes_copied = Arc::new(AtomicU64::new(0));
    let cancelled = AtomicBool::new(false);

    let result = atomic_copy(&src, &dst, 1024 * 1024, &bytes_copied, &cancelled);

    assert!(result.is_ok(), "atomic_copy failed: {:?}", result.err());
    let total = result.unwrap();
    assert_eq!(total, 100 * 1024);
    assert_eq!(bytes_copied.load(Ordering::Relaxed), 100 * 1024);

    // Destination content must match source byte-for-byte
    assert!(dst.exists(), "Destination must exist after copy");
    let dest_content = read_file(&dst);
    let src_content = read_file(&src);
    assert_eq!(dest_content, src_content, "Destination content must match source");
    assert_eq!(dest_content.len(), 100 * 1024);

    // Source must be unchanged
    assert_eq!(read_file(&src), src_content, "Source must not be modified");
}

// ── Test: empty file ───────────────────────────────────────────────

#[test]
fn test_atomic_copy_empty_file() {
    let dir = TempDir::new().unwrap();
    let src = create_test_file(dir.path(), "empty.bin", 0);
    let dst = dir.path().join("empty_dest.bin");

    let bytes_copied = Arc::new(AtomicU64::new(0));
    let cancelled = AtomicBool::new(false);

    let result = atomic_copy(&src, &dst, 1024 * 1024, &bytes_copied, &cancelled);

    assert!(result.is_ok(), "atomic_copy of empty file should succeed");
    assert_eq!(result.unwrap(), 0);
    assert_eq!(bytes_copied.load(Ordering::Relaxed), 0);

    assert!(dst.exists(), "Destination must exist after empty copy");
    let dest_meta = std::fs::metadata(&dst).unwrap();
    assert_eq!(dest_meta.len(), 0, "Destination must be 0 bytes");
}

// ── Test: cancellation before copy ─────────────────────────────────

#[test]
fn test_atomic_copy_cancellation() {
    let dir = TempDir::new().unwrap();
    let src = create_test_file(dir.path(), "src.bin", 4096);
    let dst = dir.path().join("should_not_exist.bin");

    let bytes_copied = Arc::new(AtomicU64::new(0));
    let cancelled = AtomicBool::new(true); // already cancelled

    let result = atomic_copy(&src, &dst, 1024 * 1024, &bytes_copied, &cancelled);

    assert!(result.is_err(), "Expected Err because copy was cancelled");

    // No partial file should exist at destination
    assert!(!dst.exists(), "No partial file should exist after cancelled copy");

    // Temp files should be cleaned up — check the dir only has source
    let entries: Vec<_> =
        std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
    assert_eq!(entries.len(), 1, "Expected only source file in dir, got: {:?}", entries);
    assert!(entries.contains(&"src.bin".to_string()));
}

// ── Test: destination not writable (parent dir doesn't exist) ──────

#[test]
fn test_atomic_copy_dest_not_writable() {
    let dir = TempDir::new().unwrap();
    let src = create_test_file(dir.path(), "src.bin", 1024);

    // Destination inside a non-existent subdirectory
    let dst = dir.path().join("nonexistent_subdir").join("dest.bin");

    let bytes_copied = Arc::new(AtomicU64::new(0));
    let cancelled = AtomicBool::new(false);

    let result = atomic_copy(&src, &dst, 1024 * 1024, &bytes_copied, &cancelled);

    assert!(result.is_err(), "Expected Err when dest parent doesn't exist");
    assert!(!dst.exists(), "No dest file should exist after error");
}

// ── Test: no partial file left on error ───────────────────────────

#[test]
fn test_atomic_copy_no_partial_on_error() {
    let dir = TempDir::new().unwrap();
    let src = create_test_file(dir.path(), "src.bin", 8192);
    let dst = dir.path().join("dest.bin");

    let bytes_copied = Arc::new(AtomicU64::new(0));
    let cancelled = AtomicBool::new(true); // cancel before copy

    assert!(!dst.exists(), "Dest must not exist before test");

    let result = atomic_copy(&src, &dst, 1024 * 1024, &bytes_copied, &cancelled);
    assert!(result.is_err());

    // After error, dest must NOT exist — the temp file is cleaned up
    assert!(!dst.exists(), "No dest file must remain after error");

    // No stray temp files in the directory (only source + possibly nothing else)
    let entries: Vec<_> =
        std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
    assert_eq!(entries.len(), 1, "Expected only source file in directory, got: {:?}", entries);
}

// ── Test: temp file created in dest.parent(), not /tmp ─────────────

#[test]
fn test_atomic_copy_temp_in_dest_dir() {
    let dir = TempDir::new().unwrap();
    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();

    let src = create_test_file(dir.path(), "src.bin", 2048);
    let dst = sub.join("dest.bin");

    let bytes_copied = Arc::new(AtomicU64::new(0));
    let cancelled = AtomicBool::new(false);

    let result = atomic_copy(&src, &dst, 1024 * 1024, &bytes_copied, &cancelled);
    assert!(result.is_ok(), "atomic_copy failed: {:?}", result.err());

    // After persist, only dest.bin should be in the subdir.
    // If the temp file was created in /tmp (Pitfall #2), rename would
    // fail with EXDEV, or the temp would remain in /tmp.
    let entries: Vec<_> =
        std::fs::read_dir(&sub).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
    assert_eq!(entries.len(), 1, "Expected exactly dest.bin in subdir, got: {:?}", entries);
    assert!(entries.contains(&"dest.bin".to_string()));
}
