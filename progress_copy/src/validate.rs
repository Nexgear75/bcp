use std::path::Path;

use crate::config::Config;
use crate::error::CopyError;

/// Check that the source file exists.
pub fn check_source_exists(source: &Path) -> Result<(), CopyError> {
    // Stub — always returns Ok to make tests fail on missing-source case
    let _ = source;
    Ok(())
}

/// Check that source and destination are not the same file.
/// Uses `same-file::is_same_file()` to compare device + inode (PLAN-06).
pub fn check_self_copy(source: &Path, dest: &Path) -> Result<(), CopyError> {
    // Stub — always returns Ok to make tests fail on self-copy case
    let _ = (source, dest);
    Ok(())
}

/// Check that the destination parent directory is writable.
pub fn check_dest_writable(dest: &Path) -> Result<(), CopyError> {
    // Stub — always returns Ok to make tests fail on unwritable case
    let _ = dest;
    Ok(())
}

/// Check overwrite policy respecting -f and -n flags.
pub fn check_overwrite(dest: &Path, force: bool, no_clobber: bool) -> Result<(), CopyError> {
    // Stub — always returns Ok to make tests fail on no-clobber case
    let _ = (dest, force, no_clobber);
    Ok(())
}

/// Validate that the Config flags don't conflict.
pub fn validate_flags(config: &Config) -> anyhow::Result<()> {
    // Stub — always returns Ok to make tests fail on conflicting flags
    let _ = config;
    Ok(())
}

/// Run all pre-flight checks.
pub fn run_preflight(config: &Config) -> anyhow::Result<()> {
    validate_flags(config)?;
    check_source_exists(&config.source)?;
    check_self_copy(&config.source, &config.destination)?;
    check_overwrite(&config.destination, config.force, config.no_clobber)?;
    check_dest_writable(&config.destination)?;
    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
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
