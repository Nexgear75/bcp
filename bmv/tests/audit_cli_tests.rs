//! End-to-end regression tests for bmv audit fixes.

use std::fs;
use std::time::Duration;

use assert_cmd::Command;
use predicates::prelude::*;

fn bmv() -> Command {
    let mut c = Command::cargo_bin("bmv").unwrap();
    c.timeout(Duration::from_secs(30)).arg("--no-progress");
    c
}

#[test]
fn dry_run_does_not_move() {
    let d = tempfile::tempdir().unwrap();
    let (a, b) = (d.path().join("a.txt"), d.path().join("b.txt"));
    fs::write(&a, b"hi").unwrap();
    bmv().arg("--dry-run").arg(&a).arg(&b).assert().success().stderr(predicate::str::contains("would move"));
    assert!(a.exists(), "--dry-run must not move anything");
    assert!(!b.exists());
}

#[test]
fn existing_destination_refused_then_forced() {
    let d = tempfile::tempdir().unwrap();
    let (a, b) = (d.path().join("a"), d.path().join("b"));
    fs::write(&a, b"new").unwrap();
    fs::write(&b, b"old").unwrap();
    bmv().arg(&a).arg(&b).assert().code(1).stderr(predicate::str::contains("-f to overwrite"));
    assert!(a.exists());
    bmv().arg("-n").arg(&a).arg(&b).assert().success();
    assert_eq!(fs::read(&b).unwrap(), b"old");
    bmv().args(["-f", "-b"]).arg(&a).arg(&b).assert().success();
    assert_eq!(fs::read(&b).unwrap(), b"new");
    assert_eq!(fs::read(d.path().join("b~")).unwrap(), b"old");
    assert!(!a.exists());
}

#[test]
fn missing_source_exits_1_but_moves_the_others() {
    let d = tempfile::tempdir().unwrap();
    let good = d.path().join("good");
    fs::write(&good, b"g").unwrap();
    let out = d.path().join("out");
    fs::create_dir(&out).unwrap();
    bmv().arg(d.path().join("missing")).arg(&good).arg(&out).assert().code(1);
    assert!(out.join("good").exists());
}

#[test]
fn target_directory_flag() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path().join("a");
    fs::write(&a, b"a").unwrap();
    let out = d.path().join("out");
    fs::create_dir(&out).unwrap();
    bmv().arg("-t").arg(&out).arg(&a).assert().success();
    assert!(out.join("a").exists() && !a.exists());
}

#[test]
fn move_directory_into_itself_rejected() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path().join("a");
    fs::create_dir_all(a.join("sub")).unwrap();
    bmv().arg(&a).arg(a.join("sub")).assert().code(1);
    assert!(a.join("sub").is_dir());
}
