use std::path::Path;

use anyhow::Context;

use crate::config::Config;
use crate::error::CopyError;

/// Check that the source file exists (PLAN-07).
pub fn check_source_exists(source: &Path) -> Result<(), CopyError> {
    fs_err::metadata(source).map_err(|_| CopyError::SourceNotFound(source.to_path_buf()))?;
    Ok(())
}

/// Check that source and destination are not the same file (PLAN-06).
/// Uses `same-file::is_same_file()` to compare device + inode.
pub fn check_self_copy(source: &Path, dest: &Path) -> Result<(), CopyError> {
    if same_file::is_same_file(source, dest).unwrap_or(false) {
        return Err(CopyError::SelfCopy(source.to_path_buf()));
    }
    Ok(())
}

/// Check that the destination parent directory is writable (PLAN-08).
///
/// Strategy: if `dest` is an existing file, check its permissions.
/// If `dest` does not exist, attempt a test-write to `dest.parent()`.
pub fn check_dest_writable(dest: &Path) -> Result<(), CopyError> {
    // If dest exists and is a file, check if it's writable
    if let Ok(metadata) = fs_err::metadata(dest) {
        if metadata.permissions().readonly() {
            return Err(CopyError::DestinationNotWritable(dest.to_path_buf()));
        }
        return Ok(());
    }

    // Dest does not exist — verify we can create a file in its parent
    let parent = dest.parent().unwrap_or(Path::new("."));
    fs_err::metadata(parent).map_err(|_| {
        CopyError::DestinationNotWritable(dest.to_path_buf())
    })?;

    // Test-write to the parent directory
    let test_file = parent.join("_bcp_write_test");
    match std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&test_file)
    {
        Ok(f) => {
            drop(f);
            let _ = std::fs::remove_file(&test_file);
            Ok(())
        }
        Err(_) => Err(CopyError::DestinationNotWritable(dest.to_path_buf())),
    }
}

/// Check overwrite policy per `-f` and `-n` flags.
///
/// - If dest does not exist: always Ok.
/// - If dest exists and `no_clobber`: Err(DestinationExists).
/// - If dest exists and `force`: Ok (allow overwrite).
/// - If dest exists and neither flag: Err(DestinationExists).
pub fn check_overwrite(dest: &Path, force: bool, no_clobber: bool) -> Result<(), CopyError> {
    if dest.exists() {
        if no_clobber {
            return Err(CopyError::DestinationExists(dest.to_path_buf()));
        }
        if !force {
            return Err(CopyError::DestinationExists(dest.to_path_buf()));
        }
        // force && !no_clobber: allow overwrite
    }
    Ok(())
}

/// Validate that Config flags don't conflict.
/// `-f` (force) and `-n` (no-clobber) are mutually exclusive.
pub fn validate_flags(config: &Config) -> anyhow::Result<()> {
    if config.force && config.no_clobber {
        anyhow::bail!(
            "Conflicting flags: -f (force) and -n (no-clobber) cannot be used together"
        );
    }
    Ok(())
}

/// Run all pre-flight checks in order.
/// Returns early with the first error encountered.
pub fn run_preflight(config: &Config) -> anyhow::Result<()> {
    validate_flags(config)?;

    check_source_exists(&config.source)
        .with_context(|| format!("Source not found: {}", config.source.display()))?;

    check_self_copy(&config.source, &config.destination)
        .with_context(|| "Source and destination are the same file".to_string())?;

    check_overwrite(&config.destination, config.force, config.no_clobber)
        .with_context(|| format!(
            "Destination exists: {} (use -f to overwrite)",
            config.destination.display()
        ))?;

    check_dest_writable(&config.destination)
        .with_context(|| format!(
            "Cannot write to destination: {}",
            config.destination.display()
        ))?;

    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::{NamedTempFile, TempDir};

    fn make_config(src: &str, dst: &str) -> Config {
        Config {
            source: PathBuf::from(src),
            destination: PathBuf::from(dst),
            ..Default::default()
        }
    }

    // ── Test 2: self-copy refusal ──────────────────────────────────

    #[test]
    fn test_check_self_copy_refuses_same_file() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let result = check_self_copy(&path, &path);
        assert!(result.is_err(), "Expected Err(SelfCopy) for same path");
        match result.unwrap_err() {
            CopyError::SelfCopy(p) => {
                assert_eq!(p, path);
            }
            other => panic!("Expected SelfCopy, got {:?}", other),
        }
    }

    // ── Test 3: source not found ───────────────────────────────────

    #[test]
    fn test_check_source_exists_missing_file() {
        let result = check_source_exists(Path::new("/nonexistent_xyz_123_test"));
        assert!(result.is_err(), "Expected Err(SourceNotFound)");
        match result.unwrap_err() {
            CopyError::SourceNotFound(_) => {} // expected
            other => panic!("Expected SourceNotFound, got {:?}", other),
        }
    }

    // ── Test 4: dest writable check ────────────────────────────────

    #[test]
    fn test_check_dest_writable_no_permission() {
        // Create a directory with no write permission
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("unwritable");

        // Make the parent directory read-only
        let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(dir.path(), perms).unwrap();

        let result = check_dest_writable(&dest);
        // Restore write permission so TempDir can clean up
        let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
        perms.set_readonly(false);
        std::fs::set_permissions(dir.path(), perms).unwrap();

        // Should refuse unwritable destination
        assert!(result.is_err(), "Expected Err(DestinationNotWritable)");
    }

    // ── Test 5: no-clobber with existing dest ──────────────────────

    #[test]
    fn test_check_overwrite_no_clobber_existing() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("existing.txt");
        std::fs::File::create(&dest).unwrap();

        let result = check_overwrite(&dest, false, true);
        assert!(result.is_err(), "Expected Err(DestinationExists)");
        match result.unwrap_err() {
            CopyError::DestinationExists(p) => {
                assert_eq!(p, dest);
            }
            other => panic!("Expected DestinationExists, got {:?}", other),
        }
    }

    // ── Test 6: force overwrite ────────────────────────────────────

    #[test]
    fn test_check_overwrite_force_existing() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("force_overwrite.txt");
        std::fs::File::create(&dest).unwrap();

        let result = check_overwrite(&dest, true, false);
        assert!(result.is_ok(), "Expected Ok for force overwrite");
    }

    // ── Test 7: conflicting flags ──────────────────────────────────

    #[test]
    fn test_validate_flags_conflicting() {
        let config = Config {
            force: true,
            no_clobber: true,
            ..Default::default()
        };

        let result = validate_flags(&config);
        assert!(result.is_err(), "Expected error for conflicting flags");
        let err = result.unwrap_err().to_string().to_lowercase();
        assert!(
            err.contains("conflict") || err.contains("force") || err.contains("no-clobber"),
            "Error should mention conflict: {}",
            err
        );
    }
}
