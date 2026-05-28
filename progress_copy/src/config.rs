use std::path::PathBuf;

/// Configuration for a copy operation.
#[derive(Debug, Clone)]
pub struct Config {
    /// Path to the source file.
    pub source: PathBuf,
    /// Path to the destination file.
    pub destination: PathBuf,
    /// Force overwrite of destination if it exists.
    pub force: bool,
    /// Do not overwrite an existing destination (no-clobber).
    pub no_clobber: bool,
    /// Enable verbose output.
    pub verbose: bool,
    /// Size of the read/write buffer in bytes. Defaults to 1 MB (1048576).
    pub buffer_size: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            source: PathBuf::new(),
            destination: PathBuf::new(),
            force: false,
            no_clobber: false,
            verbose: false,
            buffer_size: 1024 * 1024, // 1 MB per D-04
        }
    }
}
