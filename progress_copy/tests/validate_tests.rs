//! Integration tests for `progress_copy::validate` — pre-flight checks.
//!
//! Covers TEST-04: self-copy refusal, missing source, unwritable dest,
//! overwrite rules (force/no-clobber), conflicting flags, and the
//! integrated `run_preflight` function.

use std::io::Write;
use std::path::Path;

use progress_copy::config::Config;
use progress_copy::error::CopyError;
use progress_copy::validate::{
    check_dest_writable, check_overwrite, check_self_copy, check_source_exists,
    run_preflight, validate_flags,
};
use tempfile::{NamedTempFile, TempDir};

// ── Test: self-copy refusal ────────────────────────────────────────

#[test]
fn test_self_copy_refused() {
    let tmp = NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();

    let result = check_self_copy(&path, &path);
    assert!(result.is_err(), "Expected Err(SelfCopy) for same file");
    match result.unwrap_err() {
        CopyError::SelfCopy(p) => {
            assert_eq!(p, path, "Error should carry the file path");
        }
        other => panic!("Expected CopyError::SelfCopy, got {:?}", other),
    }
}

// ── Test: source not found ─────────────────────────────────────────

#[test]
fn test_source_not_found() {
    let result = check_source_exists(Path::new("/nonexistent_abc_xyz_test_12345"));
    assert!(result.is_err(), "Expected Err(SourceNotFound)");
    match result.unwrap_err() {
        CopyError::SourceNotFound(_) => {} // expected
        other => panic!("Expected CopyError::SourceNotFound, got {:?}", other),
    }
}

// ── Test: destination not writable (parent dir missing) ────────────

#[test]
fn test_dest_not_writable() {
    let dir = TempDir::new().unwrap();
    // Destination whose parent directory does not exist
    let dest = dir.path().join("gone").join("file.txt");

    let result = check_dest_writable(&dest);
    assert!(
        result.is_err(),
        "Expected Err(DestinationNotWritable) when dest parent doesn't exist"
    );
    match result.unwrap_err() {
        CopyError::DestinationNotWritable(p) => {
            assert_eq!(p, dest, "Error should carry the dest path");
        }
        other => panic!(
            "Expected CopyError::DestinationNotWritable, got {:?}", other
        ),
    }
}

// ── Test: no-clobber with existing dest ────────────────────────────

#[test]
fn test_no_clobber_existing() {
    let dir = TempDir::new().unwrap();
    let dest = dir.path().join("exists.txt");
    std::fs::File::create(&dest).unwrap();

    let result = check_overwrite(&dest, false, true); // force=false, no_clobber=true
    assert!(result.is_err(), "Expected Err(DestinationExists)");
    match result.unwrap_err() {
        CopyError::DestinationExists(p) => {
            assert_eq!(p, dest, "Error should carry the destination path");
        }
        other => panic!("Expected CopyError::DestinationExists, got {:?}", other),
    }
}

// ── Test: force overwrite of existing dest ─────────────────────────

#[test]
fn test_force_overwrite_existing() {
    let dir = TempDir::new().unwrap();
    let dest = dir.path().join("overwrite_me.txt");
    std::fs::File::create(&dest).unwrap();

    let result = check_overwrite(&dest, true, false); // force=true, no_clobber=false
    assert!(result.is_ok(), "Expected Ok for force overwrite: {:?}", result.err());
}

// ── Test: default overwrite behaviour (neither flag, dest exists) ──

#[test]
fn test_overwrite_no_flags_existing_refused() {
    let dir = TempDir::new().unwrap();
    let dest = dir.path().join("default.txt");
    std::fs::File::create(&dest).unwrap();

    // Default behaviour: force=false, no_clobber=false → refuse overwrite
    let result = check_overwrite(&dest, false, false);
    assert!(result.is_err(), "Expected Err(DestinationExists) with default flags");
    match result.unwrap_err() {
        CopyError::DestinationExists(_) => {} // expected
        other => panic!("Expected CopyError::DestinationExists, got {:?}", other),
    }
}

// ── Test: conflicting flags (force + no-clobber) ───────────────────

#[test]
fn test_force_and_no_clobber_conflict() {
    let config = Config {
        force: true,
        no_clobber: true,
        ..Default::default()
    };

    let result = validate_flags(&config);
    assert!(result.is_err(), "Expected error for conflicting force+no-clobber");
    let msg = result.unwrap_err().to_string().to_lowercase();
    assert!(
        msg.contains("conflict") || msg.contains("force") || msg.contains("no-clobber"),
        "Error message should mention the conflict: {msg}"
    );
}

// ── Test: overwrite skipped when dest doesn't exist ────────────────

#[test]
fn test_overwrite_skipped_if_dest_doesnt_exist() {
    let dir = TempDir::new().unwrap();
    let nonexistent = dir.path().join("not_here.txt");

    // Regardless of flags, check_overwrite is Ok when dest doesn't exist
    assert!(check_overwrite(&nonexistent, false, false).is_ok());
    assert!(check_overwrite(&nonexistent, true, false).is_ok());
    assert!(check_overwrite(&nonexistent, false, true).is_ok());
}

// ── Test: integrated run_preflight ─────────────────────────────────

#[test]
fn test_run_preflight_integration() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("source.txt");
    let dst = dir.path().join("target.txt");

    // Create a valid source file
    let mut f = std::fs::File::create(&src).unwrap();
    f.write_all(b"hello pre-flight").unwrap();
    f.sync_all().unwrap();

    let config = Config {
        source: src.clone(),
        destination: dst.clone(),
        ..Default::default()
    };

    let result = run_preflight(&config);
    assert!(
        result.is_ok(),
        "run_preflight should pass for valid source/dest: {:?}",
        result.err()
    );
}

// ── Test: run_preflight fails on self-copy ─────────────────────────

#[test]
fn test_run_preflight_self_copy_fails() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("alone.txt");
    std::fs::write(&file, b"data").unwrap();

    let config = Config {
        source: file.clone(),
        destination: file.clone(),
        ..Default::default()
    };

    let result = run_preflight(&config);
    assert!(result.is_err(), "run_preflight should fail on self-copy");
}
