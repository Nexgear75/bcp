use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur during copy operations.
#[derive(Error, Debug)]
pub enum CopyError {
    /// The source file does not exist.
    #[error("Source not found: {0}")]
    SourceNotFound(PathBuf),

    /// The source and destination are the same file.
    #[error("Cannot copy file to itself: {0}")]
    SelfCopy(PathBuf),

    /// The destination exists and no-clobber is set.
    #[error("Destination exists and -n (no-clobber) is set: {0}")]
    DestinationExists(PathBuf),

    /// The destination path cannot be written to.
    #[error("Cannot write to destination: {0}")]
    DestinationNotWritable(PathBuf),

    /// The copy was cancelled by the user (SIGINT).
    #[error("Copy cancelled by user")]
    Cancelled,

    /// A standard I/O error occurred.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
