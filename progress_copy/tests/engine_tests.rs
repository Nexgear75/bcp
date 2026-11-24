//! Integration tests for the Phase 2/3 scan → plan → execute pipeline
//! and the Phase 4 move logic.

use std::fs;
use std::path::PathBuf;

use progress_copy::config::{Config, ProgressMode};
use progress_copy::{run_copy, run_move};

fn silent_recursive() -> Config {
    Config { recursive: true, progress: ProgressMode::Silent, ..Default::default() }
}

fn write(path: &std::path::Path, content: &[u8]) {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(path, content).unwrap();
}

#[test]
fn test_recursive_copy_reproduces_tree() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    write(&src.join("a.txt"), b"alpha");
    write(&src.join("sub/b.txt"), b"beta");
    write(&src.join("sub/deep/c.bin"), &vec![7u8; 200_000]);

    let cfg = silent_recursive();
    let report = run_copy(std::slice::from_ref(&src), &dst, &cfg).unwrap();

    assert!(report.files_copied + report.bytes_copied as u64 > 0);
    assert_eq!(fs::read(dst.join("a.txt")).unwrap(), b"alpha");
    assert_eq!(fs::read(dst.join("sub/b.txt")).unwrap(), b"beta");
    assert_eq!(fs::read(dst.join("sub/deep/c.bin")).unwrap().len(), 200_000);
}

#[test]
fn test_multiple_sources_into_directory() {
    let dir = tempfile::tempdir().unwrap();
    let f1 = dir.path().join("one.txt");
    let f2 = dir.path().join("two.txt");
    write(&f1, b"one");
    write(&f2, b"two");
    let dst = dir.path().join("out");
    fs::create_dir(&dst).unwrap();

    let cfg = Config { progress: ProgressMode::Silent, ..Default::default() };
    run_copy(&[f1, f2], &dst, &cfg).unwrap();

    assert_eq!(fs::read(dst.join("one.txt")).unwrap(), b"one");
    assert_eq!(fs::read(dst.join("two.txt")).unwrap(), b"two");
}

#[test]
fn test_dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    write(&src.join("a.txt"), b"x");

    let cfg = Config { recursive: true, dry_run: true, progress: ProgressMode::Silent, ..Default::default() };
    run_copy(&[src], &dst, &cfg).unwrap();

    assert!(!dst.exists(), "dry-run must not create the destination");
}

#[test]
fn test_symlink_recreated_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    write(&src.join("real.txt"), b"data");
    std::os::unix::fs::symlink("real.txt", src.join("link.txt")).unwrap();

    run_copy(&[src], &dst, &silent_recursive()).unwrap();

    let meta = fs::symlink_metadata(dst.join("link.txt")).unwrap();
    assert!(meta.file_type().is_symlink(), "link must stay a symlink");
    assert_eq!(fs::read_link(dst.join("link.txt")).unwrap(), PathBuf::from("real.txt"));
}

#[test]
fn test_symlink_to_existing_absolute_target_not_blocked() {
    // Regression: a symlink whose target is an absolute path that exists
    // (like a virtualenv's `.venv/bin/python3` -> system python) must not
    // be treated as a pre-existing destination. The default copy (no -f)
    // used to fail here with "Destination exists" because the overwrite
    // check followed the symlink to its (existing) target.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    let real = src.join("bin/python");
    write(&real, b"#!/bin/sh\n");
    // Absolute target that resolves to an existing file.
    std::os::unix::fs::symlink(&real, src.join("bin/python3")).unwrap();

    run_copy(&[src], &dst, &silent_recursive()).expect("copy must not be blocked by symlink target");

    let link = dst.join("bin/python3");
    let meta = fs::symlink_metadata(&link).unwrap();
    assert!(meta.file_type().is_symlink(), "python3 must stay a symlink");
    assert_eq!(fs::read_link(&link).unwrap(), real);
}

#[test]
fn test_self_copy_refused() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("a.txt");
    write(&f, b"x");
    let cfg = Config { progress: ProgressMode::Silent, ..Default::default() };
    let res = run_copy(std::slice::from_ref(&f), &f, &cfg);
    assert!(res.is_err(), "copying a file onto itself must fail");
}

#[test]
fn test_no_kernel_copy_fallback_integrity() {
    // Forcing the chunked fallback must still reproduce content exactly.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    let payload = vec![0xABu8; 1_500_000];
    write(&src.join("big.bin"), &payload);

    let cfg = Config {
        recursive: true,
        no_kernel_copy: true,
        no_clone: true,
        progress: ProgressMode::Silent,
        ..Default::default()
    };
    run_copy(&[src], &dst, &cfg).unwrap();
    assert_eq!(fs::read(dst.join("big.bin")).unwrap(), payload);
}

#[test]
fn test_preserve_mode_and_timestamps() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    let file = src.join("exec.sh");
    write(&file, b"#!/bin/sh\necho hi\n");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();

    let cfg = Config {
        recursive: true,
        preserve: progress_copy::Preserve::all(),
        no_clone: true, // exercise the chunked+metadata path
        progress: ProgressMode::Silent,
        ..Default::default()
    };
    run_copy(&[src], &dst, &cfg).unwrap();

    let mode = fs::metadata(dst.join("exec.sh")).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "executable bit must be preserved");
}

#[test]
fn test_move_same_device_renames() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("m.txt");
    let dst = dir.path().join("moved.txt");
    write(&src, b"movable");
    let src_ino = fs::metadata(&src).unwrap().ino_compat();

    let cfg = Config { recursive: true, progress: ProgressMode::Silent, ..Default::default() };
    run_move(std::slice::from_ref(&src), &dst, &cfg).unwrap();

    assert!(!src.exists(), "source must be gone after a move");
    assert_eq!(fs::read(&dst).unwrap(), b"movable");
    // Same-device move is a rename: the inode is preserved (no copy).
    let dst_ino = fs::metadata(&dst).unwrap().ino_compat();
    assert_eq!(src_ino, dst_ino, "same-device move must preserve the inode");
}

#[test]
fn test_move_self_refused() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("s.txt");
    write(&f, b"x");
    let cfg = Config { progress: ProgressMode::Silent, ..Default::default() };
    let r = run_move(std::slice::from_ref(&f), &f, &cfg).unwrap();
    assert_eq!(r.exit_code(), 1, "moving a file onto itself must fail");
    assert!(f.exists());
}

// Small helper trait to read the inode portably within the test.
trait InoCompat {
    fn ino_compat(&self) -> u64;
}
impl InoCompat for std::fs::Metadata {
    fn ino_compat(&self) -> u64 {
        use std::os::unix::fs::MetadataExt;
        self.ino()
    }
}
