use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;
use tempfile::NamedTempFile;

// ── CLI help ───────────────────────────────────────────────────────

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

// ── Missing args (usage error) ─────────────────────────────────────

#[test]
fn test_cli_missing_args() {
    let mut cmd = Command::cargo_bin("bcp").unwrap();
    // No arguments → usage error, exit code 2 per D-09
    cmd.assert()
        .failure()
        .code(2);
}

// ── Version flag ───────────────────────────────────────────────────

#[test]
fn test_cli_version() {
    let mut cmd = Command::cargo_bin("bcp").unwrap();
    cmd.arg("--version");
    cmd.assert()
        .success()
        .code(0);
}

// ── End-to-end copy ────────────────────────────────────────────────

#[test]
fn test_end_to_end_copy() {
    let mut src = NamedTempFile::new().unwrap();
    let data = "Hello, bcp end-to-end test!\n".repeat(100);
    src.write_all(data.as_bytes()).unwrap();
    src.flush().unwrap();

    let dst_dir = tempfile::TempDir::new().unwrap();
    let dst_path = dst_dir.path().join("dest.bin");
    let dst_str = dst_path.to_string_lossy().to_string();

    let mut cmd = Command::cargo_bin("bcp").unwrap();
    cmd.arg(src.path().to_string_lossy().as_ref())
        .arg(&dst_str);

    cmd.assert()
        .success()
        .code(0);

    // Destination should exist with matching content
    assert!(
        dst_path.exists(),
        "Destination should exist after successful copy"
    );
    let dest_content = std::fs::read(&dst_path).unwrap();
    let src_content = std::fs::read(src.path()).unwrap();
    assert_eq!(dest_content, src_content, "Dest content must match source");
}

// ── Self-copy rejection ────────────────────────────────────────────

#[test]
fn test_self_copy_rejection() {
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(b"test data").unwrap();
    tmp.flush().unwrap();

    let path_str = tmp.path().to_string_lossy().to_string();

    let mut cmd = Command::cargo_bin("bcp").unwrap();
    cmd.arg(&path_str).arg(&path_str);
    cmd.assert().failure();
}

// ── Verbose flag ───────────────────────────────────────────────────

#[test]
fn test_verbose_flag() {
    let mut src = NamedTempFile::new().unwrap();
    src.write_all(b"verbose test content").unwrap();
    src.flush().unwrap();

    let dst_dir = tempfile::TempDir::new().unwrap();
    let dst_path = dst_dir.path().join("verbose_out.bin");
    let dst_str = dst_path.to_string_lossy().to_string();

    let mut cmd = Command::cargo_bin("bcp").unwrap();
    cmd.arg("-v")
        .arg(src.path().to_string_lossy().as_ref())
        .arg(&dst_str);

    cmd.assert().success();
}

// ── Force flag overwrites existing dest ────────────────────────────

#[test]
fn test_force_flag_overwrites() {
    let mut src = NamedTempFile::new().unwrap();
    src.write_all(b"original content").unwrap();
    src.flush().unwrap();

    let dst_dir = tempfile::TempDir::new().unwrap();
    let dst_path = dst_dir.path().join("force_dest.bin");

    // Pre-create destination with different content
    std::fs::write(&dst_path, b"different content").unwrap();
    let dst_str = dst_path.to_string_lossy().to_string();

    let mut cmd = Command::cargo_bin("bcp").unwrap();
    cmd.arg("-f")
        .arg(src.path().to_string_lossy().as_ref())
        .arg(&dst_str);

    cmd.assert().success();

    // Destination content should now match source
    let dest_content = std::fs::read(&dst_path).unwrap();
    let src_content = std::fs::read(src.path()).unwrap();
    assert_eq!(dest_content, src_content, "Force must overwrite dest with source content");
}

// ── No-clobber skips existing dest (like cp -n) ────────────────────

#[test]
fn test_no_clobber_skips() {
    let mut src = NamedTempFile::new().unwrap();
    src.write_all(b"new content").unwrap();
    src.flush().unwrap();

    let dst_dir = tempfile::TempDir::new().unwrap();
    let dst_path = dst_dir.path().join("no_clobber_dest.bin");
    std::fs::write(&dst_path, b"existing content").unwrap();

    Command::cargo_bin("bcp").unwrap()
        .arg("-n")
        .arg(src.path())
        .arg(&dst_path)
        .assert()
        .success();
    assert_eq!(std::fs::read(&dst_path).unwrap(), b"existing content");
}

#[test]
fn test_existing_dest_refused_by_default() {
    let mut src = NamedTempFile::new().unwrap();
    src.write_all(b"new").unwrap();
    let dst_dir = tempfile::TempDir::new().unwrap();
    let dst_path = dst_dir.path().join("d");
    std::fs::write(&dst_path, b"old").unwrap();

    Command::cargo_bin("bcp").unwrap()
        .arg(src.path())
        .arg(&dst_path)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("-f to overwrite"));
    assert_eq!(std::fs::read(&dst_path).unwrap(), b"old");
}

// ── Nonexistent source ─────────────────────────────────────────────

#[test]
fn test_nonexistent_source() {
    let mut cmd = Command::cargo_bin("bcp").unwrap();
    cmd.arg("/nonexistent_path_xyz_12345").arg("/tmp/nonexistent_out");
    cmd.assert().failure();
}

// ── -f / -n / -i: the last one wins (like GNU cp) ──────────────────

#[test]
fn test_last_overwrite_flag_wins() {
    let mut src = NamedTempFile::new().unwrap();
    src.write_all(b"new").unwrap();
    let dst_dir = tempfile::TempDir::new().unwrap();
    let dst_path = dst_dir.path().join("d");
    std::fs::write(&dst_path, b"old").unwrap();

    // -f then -n → -n: skip.
    Command::cargo_bin("bcp").unwrap().args(["-f", "-n"]).arg(src.path()).arg(&dst_path).assert().success();
    assert_eq!(std::fs::read(&dst_path).unwrap(), b"old");
    // -n then -f → -f: replace.
    Command::cargo_bin("bcp").unwrap().args(["-n", "-f"]).arg(src.path()).arg(&dst_path).assert().success();
    assert_eq!(std::fs::read(&dst_path).unwrap(), b"new");
}
