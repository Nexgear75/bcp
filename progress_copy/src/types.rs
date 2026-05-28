use std::sync::atomic::AtomicU64;
use std::time::Instant;

/// Shared progress state used by the copy engine and progress display.
///
/// All counters use `AtomicU64` for lock-free access from multiple threads.
/// The copy worker increments `bytes_copied` and `files_done` with relaxed
/// ordering; the progress display thread reads them every 100ms.
///
/// This type is `Send + Sync` because all fields are `Send + Sync`.
pub struct CopyProgress {
    /// Total bytes copied so far.
    pub bytes_copied: AtomicU64,
    /// Number of files completed (always 1 in single-file Phase 1).
    pub files_done: AtomicU64,
    /// When the copy started.
    pub start_time: Instant,
    /// Display name of the source file.
    pub source_name: String,
}

impl CopyProgress {
    /// Create a new `CopyProgress` with all counters at zero and `start_time`
    /// set to the current instant.
    pub fn new(source_name: String) -> Self {
        Self {
            bytes_copied: AtomicU64::new(0),
            files_done: AtomicU64::new(0),
            start_time: Instant::now(),
            source_name,
        }
    }
}
