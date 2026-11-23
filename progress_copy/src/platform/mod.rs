//! Platform-specific fast-copy strategies.
//!
//! - macOS: [`macos::clonefile`] — APFS copy-on-write, near-instant on the
//!   same volume.
//! - Linux: [`linux::copy_file_range`] — kernel-space zero-copy on the same
//!   filesystem, with graceful fallback when unsupported.

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "linux")]
pub mod linux;

/// Whether the platform offers a copy-on-write clone for same-volume files.
///
/// Used by the planner to route eligible files to the clone list.
pub fn clone_supported() -> bool {
    cfg!(target_os = "macos")
}
