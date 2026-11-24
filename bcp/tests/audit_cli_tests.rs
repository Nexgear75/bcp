//! End-to-end regression tests for the audit fixes (exit codes, CLI parsing).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use assert_cmd::Command;
use predicates::prelude::*;

fn bcp() -> Command {
    let mut c = Command::cargo_bin("bcp").unwrap();
    c.timeout(Duration::from_secs(30)).arg("--no-progress");
    c
}

fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

#[test]
fn partial_failure_exits_1_without_hanging() {
    if is_root() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("ok.txt"), b"ok").unwrap();
    fs::write(src.join("secret.txt"), b"no").unwrap();
    fs::set_permissions(src.join("secret.txt"), fs::Permissions::from_mode(0o000)).unwrap();

    let out = bcp().args(["-r", "--no-clone"]).arg(&src).arg(d.path().join("dst")).assert();
    fs::set_permissions(src.join("secret.txt"), fs::Permissions::from_mode(0o644)).unwrap();
    out.code(1).stderr(predicate::str::contains("secret.txt"));
    assert!(d.path().join("dst/ok.txt").exists());
}

#[test]
fn strict_failure_exits_1_not_130() {
    if is_root() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("secret.txt"), b"no").unwrap();
    fs::set_permissions(src.join("secret.txt"), fs::Permissions::from_mode(0o000)).unwrap();
    let out = bcp().args(["-r", "--strict"]).arg(&src).arg(d.path().join("dst")).assert();
    fs::set_permissions(src.join("secret.txt"), fs::Permissions::from_mode(0o644)).unwrap();
    out.code(1).stderr(predicate::str::contains("nterrupt").not());
}

#[test]
fn copied_executable_keeps_exec_bit() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("run.sh");
    fs::write(&src, b"#!/bin/sh\n").unwrap();
    fs::set_permissions(&src, fs::Permissions::from_mode(0o755)).unwrap();
    for extra in [&["--no-clone"][..], &[][..]] {
        let dst = d.path().join(format!("out{}.sh", extra.len()));
        bcp().args(extra).arg(&src).arg(&dst).assert().success();
        let mode = fs::metadata(&dst).unwrap().permissions().mode();
        assert_ne!(mode & 0o100, 0, "owner exec bit lost ({mode:o})");
    }
}

#[test]
fn short_p_does_not_swallow_the_source() {
    // `-p` used to take an optional value and eat the next argument.
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("a");
    fs::write(&src, b"x").unwrap();
    let old = filetime::FileTime::from_unix_time(1_100_000_000, 0);
    filetime::set_file_times(&src, old, old).unwrap();
    let dst = d.path().join("b");
    bcp().arg("-p").arg(&src).arg(&dst).assert().success();
    let m = filetime::FileTime::from_last_modification_time(&fs::metadata(&dst).unwrap());
    assert_eq!(m, old);
}

#[test]
fn invalid_preserve_value_is_a_usage_error() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("a");
    fs::write(&src, b"x").unwrap();
    bcp().arg("--preserve=mdoe").arg(&src).arg(d.path().join("b")).assert().code(2);
}

#[test]
fn buffer_size_and_threads_flags() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("a");
    fs::write(&src, vec![5u8; 100_000]).unwrap();
    bcp()
        .args(["--buffer-size", "64KiB", "-j", "2", "--no-clone"])
        .arg(&src)
        .arg(d.path().join("b"))
        .assert()
        .success();
    assert_eq!(fs::read(d.path().join("b")).unwrap().len(), 100_000);
    bcp().args(["--buffer-size", "0"]).arg(&src).arg(d.path().join("c")).assert().code(2);
    bcp().args(["-j", "0"]).arg(&src).arg(d.path().join("c")).assert().code(1);
}

#[test]
fn target_directory_flag() {
    let d = tempfile::tempdir().unwrap();
    let (a, b) = (d.path().join("a"), d.path().join("b"));
    fs::write(&a, b"a").unwrap();
    fs::write(&b, b"b").unwrap();
    let out = d.path().join("out");
    fs::create_dir(&out).unwrap();
    bcp().arg("-t").arg(&out).arg(&a).arg(&b).assert().success();
    assert!(out.join("a").exists() && out.join("b").exists());
    bcp().arg("-t").arg(d.path().join("missing")).arg(&a).assert().code(1);
}

#[test]
fn several_sources_into_missing_directory_fail() {
    let d = tempfile::tempdir().unwrap();
    let (a, b) = (d.path().join("a"), d.path().join("b"));
    fs::write(&a, b"a").unwrap();
    fs::write(&b, b"b").unwrap();
    bcp()
        .arg(&a)
        .arg(&b)
        .arg(d.path().join("nope"))
        .assert()
        .code(1)
        .stderr(predicate::str::contains("not a directory"));
}

#[test]
fn archive_and_uppercase_r() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("f"), b"x").unwrap();
    fs::set_permissions(src.join("f"), fs::Permissions::from_mode(0o640)).unwrap();
    bcp().arg("-a").arg(&src).arg(d.path().join("a")).assert().success();
    assert_eq!(fs::metadata(d.path().join("a/f")).unwrap().permissions().mode() & 0o777, 0o640);
    bcp().arg("-R").arg(&src).arg(d.path().join("r")).assert().success();
    assert!(d.path().join("r/f").exists());
}

#[test]
fn directory_without_recursive_flag() {
    let d = tempfile::tempdir().unwrap();
    bcp().arg(d.path()).arg(d.path().join("x")).assert().code(1).stderr(predicate::str::contains("-r not specified"));
}

#[test]
fn copy_into_itself_rejected() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path().join("a");
    fs::create_dir_all(a.join("sub")).unwrap();
    bcp().arg("-r").arg(&a).arg(a.join("sub")).assert().code(1).stderr(predicate::str::contains("into itself"));
}

#[test]
fn dry_run_prints_plan_and_writes_nothing() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("a");
    fs::write(&src, b"x").unwrap();
    bcp()
        .arg("--dry-run")
        .arg(&src)
        .arg(d.path().join("b"))
        .assert()
        .success()
        .stderr(predicate::str::contains("Dry run"));
    assert!(!d.path().join("b").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn non_utf8_file_names() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let d = tempfile::tempdir().unwrap();
    let name = OsStr::from_bytes(b"caf\xe9.txt");
    let src = d.path().join(name);
    fs::write(&src, b"x").unwrap();
    let out = d.path().join("out");
    fs::create_dir(&out).unwrap();
    bcp().arg(&src).arg(&out).assert().success();
    assert!(out.join(name).exists());
}
