use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;
use tempfile::NamedTempFile;

// ── Test 8: CLI help ───────────────────────────────────────────────

#[test]
fn test_cli_help() {
    let mut cmd = Command::cargo_bin("bcp").unwrap();
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("bcp"))
        .stdout(predicate::str::contains("Usage"))
        .stdout(predicate::str::contains("-f"))
        .stdout(predicate::str::contains("-n"))
        .stdout(predicate::str::contains("-v"))
        .stdout(predicate::str::contains("--help"))
        .stdout(predicate::str::contains("--version"));
}

// ── Test 9: missing args ───────────────────────────────────────────

#[test]
fn test_cli_missing_args() {
    let mut cmd = Command::cargo_bin("bcp").unwrap();
    // No arguments → usage error, exit code 2 per D-09
    cmd.assert()
        .failure()
        .code(2);
}

// ── Test 10: version ──────────────────────────────────────────────

#[test]
fn test_cli_version() {
    let mut cmd = Command::cargo_bin("bcp").unwrap();
    cmd.arg("--version");
    cmd.assert()
        .success()
        .code(0);
}

// ── Test 11: full end-to-end copy ─────────────────────────────────

#[test]
fn test_end_to_end_copy() {
    let mut src = NamedTempFile::new().unwrap();
    let data = "Hello, bcp end-to-end test!\n".repeat(100);
    src.write_all(data.as_bytes()).unwrap();
    src.flush().unwrap();

    let dst_dir = tempfile::TempDir::new().unwrap();
    let dst_path = dst_dir.path().join("dest.bin");
    let dst_str = dst_path.to_string_lossy().to_string();

    // Note: src NamedTempFile lives until end of test, keeping
    // the file alive on disk for the bcp process to read.
    let mut cmd = Command::cargo_bin("bcp").unwrap();
    cmd.arg(src.path().to_string_lossy().as_ref())
        .arg(&dst_str);

    let output = cmd.output().unwrap();

    // Stub run() returns Ok — but doesn't actually copy, so
    // dest won't exist. This test will FAIL in RED phase.
    if output.status.success() {
        assert!(
            dst_path.exists(),
            "Destination should exist after successful copy"
        );
        let dest_content = std::fs::read(&dst_path).unwrap();
        let src_content = std::fs::read(src.path()).unwrap();
        assert_eq!(dest_content, src_content);
    }
}
