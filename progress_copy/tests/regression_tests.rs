//! Regression tests for every finding of the 2026-11 audit.

mod common;

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;

use common::*;
use progress_copy::{Config, Dereference, Overwrite, Preserve, SyncMode, run_copy, run_move};

// ── P0: permissions ────────────────────────────────────────────────

#[test]
fn default_mode_is_source_masked_by_umask_on_every_path() {
    for no_clone in [true, false] {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("script.sh");
        write(&src, b"#!/bin/sh\n");
        fs::set_permissions(&src, fs::Permissions::from_mode(0o755)).unwrap();
        let dst = d.path().join("out.sh");
        let cfg = Config { no_clone, ..silent() };
        run_copy(&[src], &dst, &cfg).unwrap();
        assert_eq!(mode(&dst), 0o755 & !umask(), "no_clone={no_clone}: must not be 0600");
    }
}

#[test]
fn setuid_bit_not_copied_without_preserve() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("s");
    write(&src, b"x");
    fs::set_permissions(&src, fs::Permissions::from_mode(0o4755)).unwrap();
    let dst = d.path().join("d");
    run_copy(&[src], &dst, &Config { no_clone: true, ..silent() }).unwrap();
    assert_eq!(mode(&dst) & 0o7000, 0);
}

// ── P0: partial failure (hang + exit code) ─────────────────────────

#[test]
fn unreadable_file_fails_fast_and_reports() {
    if is_root() {
        return; // root can read mode-000 files
    }
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    write(&src.join("ok.txt"), b"ok");
    write(&src.join("secret.txt"), b"no");
    fs::set_permissions(src.join("secret.txt"), fs::Permissions::from_mode(0o000)).unwrap();
    let dst = d.path().join("dst");

    let (s, t) = (src.clone(), dst.clone());
    let report = within(20, move || run_copy(&[s], &t, &Config { no_clone: true, ..silent_recursive() }).unwrap());

    fs::set_permissions(src.join("secret.txt"), fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(report.files_failed, 1);
    assert_eq!(report.exit_code(), 1, "a failed file must give exit code 1");
    assert!(!report.cancelled);
    assert_eq!(fs::read(dst.join("ok.txt")).unwrap(), b"ok");
    assert!(!dst.join("secret.txt").exists());
    assert!(leftover_temps(&dst).is_empty());
}

#[test]
fn strict_failure_is_an_error_not_an_interruption() {
    if is_root() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    write(&src.join("secret.txt"), b"no");
    fs::set_permissions(src.join("secret.txt"), fs::Permissions::from_mode(0o000)).unwrap();
    let cfg = Config { strict: true, no_clone: true, ..silent_recursive() };
    let (s, t) = (src.clone(), d.path().join("dst"));
    let r = within(20, move || run_copy(&[s], &t, &cfg));
    fs::set_permissions(src.join("secret.txt"), fs::Permissions::from_mode(0o644)).unwrap();
    let err = format!("{:#}", r.unwrap_err());
    assert!(err.contains("secret.txt"), "{err}");
    assert!(!err.to_lowercase().contains("interrupt"), "{err}");
}

#[test]
fn socket_in_tree_is_reported_as_failure() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    write(&src.join("a.txt"), b"a");
    let _sock = std::os::unix::net::UnixListener::bind(src.join("s.sock")).unwrap();
    let r = run_copy(&[src], &d.path().join("dst"), &silent_recursive()).unwrap();
    assert_eq!(r.exit_code(), 1);
    assert!(r.errors.iter().any(|e| e.contains("s.sock")), "{:?}", r.errors);
}

// ── Overwrite policy ───────────────────────────────────────────────

fn pair() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let s = d.path().join("s");
    let t = d.path().join("t");
    write(&s, b"new");
    write(&t, b"old");
    (d, s, t)
}

#[test]
fn existing_destination_refused_by_default_with_hint() {
    let (_d, s, t) = pair();
    let e = format!("{:#}", run_copy(&[s], &t, &silent()).unwrap_err());
    assert!(e.contains("-f"), "message must explain how to overwrite: {e}");
    assert_eq!(fs::read(&t).unwrap(), b"old");
}

#[test]
fn no_clobber_skips_silently() {
    let (_d, s, t) = pair();
    let r = run_copy(&[s], &t, &Config { overwrite: Overwrite::Skip, ..silent() }).unwrap();
    assert_eq!(r.files_skipped, 1);
    assert_eq!(r.exit_code(), 0);
    assert_eq!(fs::read(&t).unwrap(), b"old");
}

#[test]
fn force_replaces_and_backup_keeps_old() {
    let (d, s, t) = pair();
    let cfg = Config { overwrite: Overwrite::Force, backup_suffix: Some("~".into()), ..silent() };
    run_copy(&[s], &t, &cfg).unwrap();
    assert_eq!(fs::read(&t).unwrap(), b"new");
    assert_eq!(fs::read(d.path().join("t~")).unwrap(), b"old");
}

#[test]
fn update_replaces_only_older() {
    let (_d, s, t) = pair();
    let old = filetime::FileTime::from_unix_time(1_000_000_000, 0);
    filetime::set_file_times(&s, old, old).unwrap();
    let r = run_copy(std::slice::from_ref(&s), &t, &Config { update: true, ..silent() }).unwrap();
    assert_eq!(r.files_skipped, 1);
    assert_eq!(fs::read(&t).unwrap(), b"old");

    filetime::set_file_times(&t, old, old).unwrap();
    let now = filetime::FileTime::now();
    filetime::set_file_times(&s, now, now).unwrap();
    run_copy(&[s], &t, &Config { update: true, ..silent() }).unwrap();
    assert_eq!(fs::read(&t).unwrap(), b"new");
}

// ── Tree semantics ─────────────────────────────────────────────────

#[test]
fn copy_into_itself_refused() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path().join("a");
    write(&a.join("f"), b"x");
    fs::create_dir(a.join("sub")).unwrap();
    let e = run_copy(std::slice::from_ref(&a), &a.join("sub"), &silent_recursive()).unwrap_err();
    assert!(format!("{e:#}").contains("into itself"));
    assert!(!a.join("sub/a").exists());
}

#[test]
fn several_sources_need_a_directory() {
    let d = tempfile::tempdir().unwrap();
    write(&d.path().join("1"), b"1");
    write(&d.path().join("2"), b"2");
    let r = run_copy(&[d.path().join("1"), d.path().join("2")], &d.path().join("nope"), &silent());
    assert!(r.is_err());
}

#[test]
fn no_target_directory_copies_onto_name() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    write(&src.join("f"), b"x");
    let dst = d.path().join("dst");
    fs::create_dir(&dst).unwrap();
    let cfg = Config { no_target_directory: true, ..silent_recursive() };
    run_copy(&[src], &dst, &cfg).unwrap();
    assert!(dst.join("f").exists(), "-T: contents go directly into dst");
    assert!(!dst.join("src").exists());
}

#[test]
fn hardlinks_are_preserved() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    write(&src.join("a"), &vec![9u8; 100_000]);
    fs::hard_link(src.join("a"), src.join("b")).unwrap();
    let dst = d.path().join("dst");
    let r = run_copy(&[src], &dst, &Config { no_clone: true, ..silent_recursive() }).unwrap();
    assert_eq!(r.hardlinks_created, 1);
    assert_eq!(fs::metadata(dst.join("a")).unwrap().ino(), fs::metadata(dst.join("b")).unwrap().ino());
    assert_eq!(r.bytes_copied, 100_000, "the data is copied once");
}

#[test]
fn sparse_files_stay_sparse() {
    use std::io::{Seek, SeekFrom, Write};
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("disk.img");
    let size: u64 = 128 * 1024 * 1024;
    {
        let mut f = fs::File::create(&src).unwrap();
        f.write_all(b"MBR").unwrap();
        f.seek(SeekFrom::Start(size - 3)).unwrap();
        f.write_all(b"END").unwrap();
    }
    if fs::metadata(&src).unwrap().blocks() * 512 >= size {
        return; // filesystem without sparse support
    }
    let dst = d.path().join("copy.img");
    let r = run_copy(std::slice::from_ref(&src), &dst, &Config { no_clone: true, ..silent() }).unwrap();
    assert_eq!(r.bytes_copied, size);
    let m = fs::metadata(&dst).unwrap();
    assert_eq!(m.len(), size);
    assert!(m.blocks() * 512 < size / 4, "copy densified the sparse file ({} bytes)", m.blocks() * 512);
    assert_eq!(fs::read(&dst).unwrap(), fs::read(&src).unwrap());
}

#[test]
fn top_level_fifo_move_and_copy_do_not_block() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("p");
    nix::unistd::mkfifo(&p, nix::sys::stat::Mode::from_bits_truncate(0o644)).unwrap();
    let (s, t) = (p.clone(), d.path().join("copy"));
    let r = within(20, move || run_copy(&[s], &t, &Config { preserve: Preserve::all(), ..silent() }).unwrap());
    assert!(r.is_success(), "{:?}", r.errors);
    let (s, t) = (p.clone(), d.path().join("moved"));
    let r = within(20, move || run_move(&[s], &t, &Config { preserve: Preserve::all(), ..silent() }).unwrap());
    assert!(r.is_success(), "{:?}", r.errors);
    assert!(!p.exists());
}

#[test]
fn fifo_is_recreated() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    fs::create_dir(&src).unwrap();
    nix::unistd::mkfifo(&src.join("p"), nix::sys::stat::Mode::from_bits_truncate(0o640)).unwrap();
    let dst = d.path().join("dst");
    let r = run_copy(&[src], &dst, &silent_recursive()).unwrap();
    assert_eq!(r.fifos_created, 1);
    use std::os::unix::fs::FileTypeExt;
    assert!(fs::symlink_metadata(dst.join("p")).unwrap().file_type().is_fifo());
}

#[test]
fn dereference_follows_links() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    write(&src.join("real"), b"payload");
    std::os::unix::fs::symlink(src.join("real"), src.join("link")).unwrap();
    let dst = d.path().join("dst");
    run_copy(&[src], &dst, &Config { dereference: Dereference::Always, ..silent_recursive() }).unwrap();
    let m = fs::symlink_metadata(dst.join("link")).unwrap();
    assert!(m.file_type().is_file(), "-L must copy the target, not the link");
    assert_eq!(fs::read(dst.join("link")).unwrap(), b"payload");
}

#[test]
fn directory_metadata_applied_after_contents() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    write(&src.join("ro/f"), b"x");
    fs::set_permissions(src.join("ro"), fs::Permissions::from_mode(0o555)).unwrap();
    let old = filetime::FileTime::from_unix_time(1_200_000_000, 0);
    filetime::set_file_times(src.join("ro"), old, old).unwrap();
    let dst = d.path().join("dst");
    let cfg = Config { preserve: Preserve::all(), ..silent_recursive() };
    let r = run_copy(std::slice::from_ref(&src), &dst, &cfg).unwrap();
    fs::set_permissions(src.join("ro"), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(r.is_success(), "{:?}", r.errors);
    assert_eq!(fs::read(dst.join("ro/f")).unwrap(), b"x", "read-only dir must still be filled");
    assert_eq!(mode(&dst.join("ro")), 0o555);
    assert_eq!(fs::metadata(dst.join("ro")).unwrap().mtime(), 1_200_000_000);
    fs::set_permissions(dst.join("ro"), fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn every_sync_mode_produces_identical_copies_and_no_temps() {
    for sync in [SyncMode::None, SyncMode::Batch, SyncMode::Full] {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        for i in 0..50 {
            write(&src.join(format!("d{}/f{i}", i % 5)), format!("content {i}").as_bytes());
        }
        let dst = d.path().join("dst");
        let r = run_copy(&[src], &dst, &Config { sync, no_clone: true, ..silent_recursive() }).unwrap();
        assert_eq!(r.files_copied, 50, "{sync:?}");
        assert_eq!(fs::read(dst.join("d3/f13")).unwrap(), b"content 13");
        assert!(leftover_temps(d.path()).is_empty());
    }
}

#[test]
fn long_file_names_work() {
    let d = tempfile::tempdir().unwrap();
    let name = "n".repeat(250);
    let src = d.path().join(&name);
    write(&src, b"x");
    let dst = d.path().join("out");
    fs::create_dir(&dst).unwrap();
    run_copy(&[src], &dst, &Config { no_clone: true, ..silent() }).unwrap();
    assert!(dst.join(&name).exists());
}

// ── bmv ────────────────────────────────────────────────────────────

#[test]
fn move_dry_run_changes_nothing() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("a.txt");
    write(&src, b"x");
    let dst = d.path().join("b.txt");
    run_move(std::slice::from_ref(&src), &dst, &Config { dry_run: true, ..silent_recursive() }).unwrap();
    assert!(src.exists(), "dry-run must not move");
    assert!(!dst.exists());
}

#[test]
fn move_refuses_existing_destination_by_default() {
    let (_d, s, t) = pair();
    let r = run_move(std::slice::from_ref(&s), &t, &silent_recursive()).unwrap();
    assert_eq!(r.exit_code(), 1);
    assert!(s.exists());
    assert_eq!(fs::read(&t).unwrap(), b"old");
}

#[test]
fn move_into_own_subdirectory_refused() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path().join("a");
    fs::create_dir_all(a.join("sub")).unwrap();
    let r = run_move(std::slice::from_ref(&a), &a.join("sub"), &silent_recursive()).unwrap();
    assert_eq!(r.exit_code(), 1);
    assert!(a.join("sub").exists());
}

#[test]
fn move_continues_after_a_failing_source() {
    let d = tempfile::tempdir().unwrap();
    let good = d.path().join("good");
    write(&good, b"g");
    let out = d.path().join("out");
    fs::create_dir(&out).unwrap();
    let r = run_move(&[d.path().join("missing"), good.clone()], &out, &silent_recursive()).unwrap();
    assert_eq!(r.exit_code(), 1);
    assert!(out.join("good").exists(), "other sources must still be moved");
}

// ── bmv across filesystems (real second filesystem) ────────────────

#[test]
fn cross_fs_move_tree_with_metadata() {
    let Some(other) = OtherFs::new() else { return };
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("tree");
    write(&src.join("a.txt"), b"alpha");
    write(&src.join("sub/b.bin"), &vec![3u8; 300_000]);
    fs::hard_link(src.join("a.txt"), src.join("a2.txt")).unwrap();
    std::os::unix::fs::symlink("a.txt", src.join("link")).unwrap();
    fs::set_permissions(src.join("sub"), fs::Permissions::from_mode(0o750)).unwrap();
    let old = filetime::FileTime::from_unix_time(1_300_000_000, 0);
    filetime::set_file_times(src.join("a.txt"), old, old).unwrap();

    let cfg = Config { preserve: Preserve::all(), ..silent_recursive() };
    let r = run_move(std::slice::from_ref(&src), &other.dir, &cfg).unwrap();
    assert!(r.is_success(), "{:?}", r.errors);
    assert!(!src.exists(), "source must be gone after a successful move");
    let t = other.dir.join("tree");
    assert_eq!(fs::read(t.join("a.txt")).unwrap(), b"alpha");
    assert_eq!(fs::read(t.join("sub/b.bin")).unwrap().len(), 300_000);
    assert_eq!(fs::read_link(t.join("link")).unwrap(), PathBuf::from("a.txt"));
    assert_eq!(fs::metadata(t.join("a.txt")).unwrap().ino(), fs::metadata(t.join("a2.txt")).unwrap().ino());
    assert_eq!(mode(&t.join("sub")), 0o750);
    assert_eq!(fs::metadata(t.join("a.txt")).unwrap().mtime(), 1_300_000_000);
}

#[test]
fn cross_fs_move_keeps_source_when_something_cannot_be_copied() {
    // Regression for the data-loss bug: special files that are not copied
    // used to be deleted by remove_dir_all.
    let Some(other) = OtherFs::new() else { return };
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("tree");
    write(&src.join("file.txt"), b"data");
    let _sock = std::os::unix::net::UnixListener::bind(src.join("app.sock")).unwrap();

    let r = run_move(std::slice::from_ref(&src), &other.dir, &silent_recursive()).unwrap();
    assert_eq!(r.exit_code(), 1);
    assert!(src.join("file.txt").exists(), "source must be left intact");
    assert!(src.join("app.sock").exists(), "uncopied entry must never be deleted");
}

#[test]
fn cross_fs_move_with_fifo_moves_it() {
    let Some(other) = OtherFs::new() else { return };
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("tree");
    write(&src.join("file.txt"), b"data");
    nix::unistd::mkfifo(&src.join("pipe"), nix::sys::stat::Mode::from_bits_truncate(0o644)).unwrap();
    // Same options as the bmv binary (preserve everything): metadata on a
    // FIFO used to open it and block forever.
    let cfg = Config { preserve: Preserve::all(), ..silent_recursive() };
    let (s, o) = (src.clone(), other.dir.clone());
    let r = within(30, move || run_move(&[s], &o, &cfg).unwrap());
    assert!(r.is_success(), "{:?}", r.errors);
    assert!(!src.exists());
    use std::os::unix::fs::FileTypeExt;
    assert!(fs::symlink_metadata(other.dir.join("tree/pipe")).unwrap().file_type().is_fifo());
}

#[test]
fn cross_fs_move_unreadable_file_keeps_source() {
    if is_root() {
        return;
    }
    let Some(other) = OtherFs::new() else { return };
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("tree");
    write(&src.join("ok"), b"ok");
    write(&src.join("locked"), b"no");
    fs::set_permissions(src.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let (s, o) = (src.clone(), other.dir.clone());
    let r = within(30, move || run_move(&[s], &o, &silent_recursive()).unwrap());
    fs::set_permissions(src.join("locked"), fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(r.exit_code(), 1);
    assert!(src.join("ok").exists() && src.join("locked").exists(), "nothing deleted");
}

#[test]
fn cross_fs_move_single_file_and_dry_run() {
    let Some(other) = OtherFs::new() else { return };
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("f.bin");
    write(&src, &vec![1u8; 50_000]);

    run_move(std::slice::from_ref(&src), &other.dir, &Config { dry_run: true, ..silent_recursive() }).unwrap();
    assert!(src.exists() && !other.dir.join("f.bin").exists());

    let r = run_move(std::slice::from_ref(&src), &other.dir, &silent_recursive()).unwrap();
    assert!(r.is_success(), "{:?}", r.errors);
    assert!(!src.exists());
    assert_eq!(fs::read(other.dir.join("f.bin")).unwrap().len(), 50_000);
    assert!(leftover_temps(&other.dir).is_empty());
}

#[test]
fn cross_fs_copy_falls_back_from_clone() {
    // Clone is impossible across volumes: every file must still be copied.
    let Some(other) = OtherFs::new() else { return };
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    for i in 0..20 {
        write(&src.join(format!("f{i}")), format!("{i}").as_bytes());
    }
    let r = run_copy(&[src], &other.dir.join("dst"), &silent_recursive()).unwrap();
    assert_eq!(r.files_copied, 20);
    assert_eq!(fs::read(other.dir.join("dst/f7")).unwrap(), b"7");
}
