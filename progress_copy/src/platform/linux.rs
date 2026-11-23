//! Linux kernel-space copy via `copy_file_range(2)`.

use std::io;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Copy `size` bytes from `src` to `dst` using `copy_file_range(2)`.
///
/// This performs the copy in kernel space (zero-copy on the same
/// filesystem). It updates `bytes_copied` after each successful range so
/// the progress display stays live, and honours the `cancelled` flag
/// between ranges.
///
/// Returns `Ok(Some(total))` on success, or `Ok(None)` when the kernel
/// does not support the call for these files (`ENOSYS`/`EXDEV`/`EINVAL`),
/// signalling the caller to fall back to a chunked copy.
pub fn copy_file_range(
    src: &fs_err::File,
    dst: &fs_err::File,
    size: u64,
    bytes_copied: &Arc<AtomicU64>,
    cancelled: &AtomicBool,
) -> io::Result<Option<u64>> {
    let src_fd = src.as_raw_fd();
    let dst_fd = dst.as_raw_fd();
    let mut remaining = size;
    let mut total: u64 = 0;

    while remaining > 0 {
        if cancelled.load(Ordering::Relaxed) {
            // Surface cancellation as an error so the atomic temp file is
            // dropped without being persisted.
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }

        let want = remaining.min(1 << 30) as usize; // cap each call at 1 GiB
        let ret = unsafe {
            libc::copy_file_range(
                src_fd,
                std::ptr::null_mut(),
                dst_fd,
                std::ptr::null_mut(),
                want,
                0,
            )
        };

        if ret < 0 {
            let err = io::Error::last_os_error();
            match err.raw_os_error() {
                // Unsupported for these files → tell caller to fall back.
                Some(libc::ENOSYS) | Some(libc::EXDEV) | Some(libc::EINVAL)
                | Some(libc::EOPNOTSUPP) | Some(libc::EBADF)
                    if total == 0 =>
                {
                    return Ok(None);
                }
                _ => return Err(err),
            }
        }
        if ret == 0 {
            break; // EOF
        }

        let n = ret as u64;
        total += n;
        remaining -= n;
        bytes_copied.fetch_add(n, Ordering::Relaxed);
    }

    Ok(Some(total))
}
