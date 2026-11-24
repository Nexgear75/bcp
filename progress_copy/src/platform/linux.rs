//! Linux kernel-space copy via `copy_file_range(2)`.

use std::io;
use std::os::unix::io::AsRawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Bytes per `copy_file_range` call: small enough for a live progress bar and
/// a responsive Ctrl+C, large enough to keep syscall overhead negligible.
const CHUNK: usize = 16 * 1024 * 1024;

/// Result of a kernel copy attempt.
#[derive(Debug, PartialEq, Eq)]
pub enum KernelCopy {
    /// Everything up to EOF was copied.
    Done(u64),
    /// Not usable for these files; nothing was copied.
    Unsupported,
    /// Stopped early (short return); the file offsets are positioned after
    /// the copied bytes, so a user-space copy can simply continue.
    Partial(u64),
}

/// Copy from the current offset of `src` to the current offset of `dst`
/// until EOF using `copy_file_range(2)`.
///
/// - Copies until EOF rather than a size captured at scan time, so a file
///   that grew is not truncated.
/// - Kernels 5.3–5.18 may return 0 before EOF on some filesystems; when that
///   happens before the known size, `Partial` is returned and the caller
///   finishes in user space.
///
/// # Errors
/// `Interrupted` when cancelled, or any unexpected I/O error.
pub fn copy_file_range(
    src: &std::fs::File,
    dst: &std::fs::File,
    size_hint: u64,
    bytes_copied: &Arc<AtomicU64>,
    cancelled: &AtomicBool,
) -> io::Result<KernelCopy> {
    let src_fd = src.as_raw_fd();
    let dst_fd = dst.as_raw_fd();
    let mut total: u64 = 0;

    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let ret =
            unsafe { libc::copy_file_range(src_fd, std::ptr::null_mut(), dst_fd, std::ptr::null_mut(), CHUNK, 0) };
        if ret < 0 {
            let err = io::Error::last_os_error();
            return match err.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::ENOSYS | libc::EXDEV | libc::EINVAL | libc::EOPNOTSUPP | libc::EBADF | libc::EPERM)
                    if total == 0 =>
                {
                    Ok(KernelCopy::Unsupported)
                }
                Some(libc::ENOSYS | libc::EXDEV | libc::EINVAL | libc::EOPNOTSUPP) => Ok(KernelCopy::Partial(total)),
                _ => Err(err),
            };
        }
        if ret == 0 {
            // Real EOF, or a kernel that gave up early: let the caller verify.
            if total < size_hint {
                return Ok(KernelCopy::Partial(total));
            }
            return Ok(KernelCopy::Done(total));
        }
        let n = ret as u64;
        total += n;
        bytes_copied.fetch_add(n, Ordering::Relaxed);
    }
}

/// Hint the kernel that `file` will be read sequentially (bigger readahead).
pub fn advise_sequential(file: &std::fs::File) {
    unsafe {
        libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_SEQUENTIAL);
    }
}
