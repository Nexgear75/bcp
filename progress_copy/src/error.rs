use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur during copy operations.
#[derive(Error, Debug)]
pub enum CopyError {
    /// The source does not exist.
    #[error("cannot stat '{0}': No such file or directory")]
    SourceNotFound(PathBuf),

    /// The source and destination are the same file.
    #[error("'{0}' and its destination are the same file")]
    SelfCopy(PathBuf),

    /// The destination exists and the policy refuses to replace it.
    #[error("'{0}' already exists (use -f to overwrite, -n to skip, -i to ask, -u to replace older files)")]
    DestinationExists(PathBuf),

    /// The copy was cancelled (Ctrl+C, or `--strict` after a failure).
    #[error("cancelled")]
    Cancelled,

    /// A standard I/O error occurred.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
