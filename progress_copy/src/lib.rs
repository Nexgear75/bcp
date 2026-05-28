#![allow(dead_code)]

pub mod config;
pub mod error;
pub mod types;
pub mod copy;
pub mod atomic;
pub mod progress;
pub mod signals;
pub mod validate;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::Context;

pub use config::Config;
pub use error::CopyError;

/// Main entry point for a copy operation.
///
/// Orchestrates: pre-flight validation → SIGINT handler → progress bar
/// setup → atomic copy → cleanup.
pub fn run(config: Config) -> anyhow::Result<()> {
    // Stub — returns Ok to let CLI tests compile, but doesn't copy.
    // Will be fully implemented in GREEN phase.
    let _ = config;
    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn test_run_config_roundtrip() {
        // Verify that run() accepts a Config and returns Result.
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        std::fs::write(&src, b"hello").unwrap();

        let config = Config {
            source: src.clone(),
            destination: dst.clone(),
            ..Default::default()
        };

        // Stub returns Ok — the real implementation will actually copy.
        let result = run(config);
        assert!(result.is_ok(), "run() should accept valid config");
    }
}
