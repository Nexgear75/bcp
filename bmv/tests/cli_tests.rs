use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;

#[test]
fn test_cli_help() {
    let mut cmd = Command::cargo_bin("bmv").unwrap();
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("bmv"))
        .stdout(predicate::str::contains("Usage"))
        .stdout(predicate::str::contains("-f"))
        .stdout(predicate::str::contains("-n"));
}

#[test]
fn test_cli_missing_args() {
    let mut cmd = Command::cargo_bin("bmv").unwrap();
    cmd.assert().failure().code(2);
}

#[test]
fn test_cli_version() {
    let mut cmd = Command::cargo_bin("bmv").unwrap();
    cmd.arg("--version").assert().success().code(0);
}

#[test]
fn test_same_device_move() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("a.txt");
    let dst = dir.path().join("b.txt");
    fs::write(&src, b"payload").unwrap();

    let mut cmd = Command::cargo_bin("bmv").unwrap();
    cmd.arg("--no-progress").arg(src.to_string_lossy().as_ref()).arg(dst.to_string_lossy().as_ref());
    cmd.assert().success();

    assert!(!src.exists(), "source removed after move");
    assert_eq!(fs::read(&dst).unwrap(), b"payload");
}

#[test]
fn test_self_move_refused() {
    let dir = tempfile::TempDir::new().unwrap();
    let f = dir.path().join("x.txt");
    fs::write(&f, b"x").unwrap();
    let s = f.to_string_lossy().to_string();

    let mut cmd = Command::cargo_bin("bmv").unwrap();
    cmd.arg("--no-progress").arg(&s).arg(&s);
    cmd.assert().failure();
}

#[test]
fn test_move_into_directory() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("f.txt");
    fs::write(&src, b"data").unwrap();
    let destdir = dir.path().join("dst");
    fs::create_dir(&destdir).unwrap();

    let mut cmd = Command::cargo_bin("bmv").unwrap();
    cmd.arg("--no-progress").arg(src.to_string_lossy().as_ref()).arg(destdir.to_string_lossy().as_ref());
    cmd.assert().success();

    assert!(!src.exists());
    assert_eq!(fs::read(destdir.join("f.txt")).unwrap(), b"data");
}
