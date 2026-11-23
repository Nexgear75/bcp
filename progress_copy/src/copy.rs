//! User-space copy loops: chunked read/write and a hole-preserving variant
//! for sparse files.

use std::cell::RefCell;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use crate::error::CopyError;

thread_local! {
    /// One reusable buffer per worker thread (no per-file allocation).
    static BUFFER: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Run `f` with a thread-local buffer of exactly `len` bytes.
fn with_buffer<T>(len: usize, f: impl FnOnce(&mut [u8]) -> T) -> T {
    BUFFER.with(|b| {
        let mut b = b.borrow_mut();
        if b.len() < len {
            b.resize(len, 0);
        }
        f(&mut b[..len])
    })
}

/// Copy everything from `src` to `dst` (until EOF) in chunks of at most
/// `buffer_size` bytes, adding each chunk to `bytes_copied`.
///
/// `size_hint` (the size seen at scan time) only sizes the buffer: a file that
/// grew since the scan is still copied completely.
///
/// # Errors
/// `Cancelled` when `cancelled` is set between chunks, or any I/O error.
pub fn chunked_copy_with_progress<R: Read, W: Write>(
    src: &mut R,
    dst: &mut W,
    size_hint: u64,
    buffer_size: usize,
    bytes_copied: &Arc<AtomicU64>,
    cancelled: &AtomicBool,
) -> Result<u64, CopyError> {
    // Small files don't need a 1 MiB buffer; +1 lets EOF be seen in one read.
    let want = usize::try_from(size_hint.saturating_add(1)).unwrap_or(usize::MAX);
    let len = buffer_size.max(1).min(want.max(64 * 1024));
    with_buffer(len, |buf| {
        let mut total: u64 = 0;
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err(CopyError::Cancelled);
            }
            let n = match src.read(buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(CopyError::Io(e)),
            };
            dst.write_all(&buf[..n]).map_err(CopyError::Io)?;
            total += n as u64;
            bytes_copied.fetch_add(n as u64, Ordering::Relaxed);
        }
        dst.flush().map_err(CopyError::Io)?;
        Ok(total)
    })
}

/// Copy a sparse file, recreating its holes instead of writing zeros.
///
/// Walks the data segments with `lseek(SEEK_DATA/SEEK_HOLE)`, copies each one
/// at the same offset, then sets the final length. Progress counts the
/// logical size (holes included) so the bar still reaches 100 %.
///
/// Returns `Ok(None)` when the filesystem can't report holes — the caller
/// then uses a plain copy.
///
/// # Errors
/// `Cancelled`, or any I/O error.
pub fn sparse_copy(
    src: &std::fs::File,
    dst: &std::fs::File,
    size: u64,
    buffer_size: usize,
    bytes_copied: &Arc<AtomicU64>,
    cancelled: &AtomicBool,
) -> Result<Option<u64>, CopyError> {
    use std::os::unix::fs::FileExt;
    use std::os::unix::io::AsRawFd;

    let fd = src.as_raw_fd();
    let seek = |off: u64, whence: libc::c_int| -> std::io::Result<Option<u64>> {
        let off = libc::off_t::try_from(off).map_err(|_| std::io::Error::from_raw_os_error(libc::EOVERFLOW))?;
        let r = unsafe { libc::lseek(fd, off, whence) };
        if r < 0 {
            let e = std::io::Error::last_os_error();
            // ENXIO: no more data after `off`.
            if e.raw_os_error() == Some(libc::ENXIO) {
                return Ok(None);
            }
            return Err(e);
        }
        Ok(Some(u64::try_from(r).unwrap_or(0)))
    };

    // Probe support: an fs without hole reporting answers EINVAL.
    match seek(0, libc::SEEK_DATA) {
        Err(e) if e.raw_os_error() == Some(libc::EINVAL) => return Ok(None),
        Err(e) => return Err(CopyError::Io(e)),
        Ok(_) => {}
    }

    let len = buffer_size.max(64 * 1024);
    let mut pos: u64 = 0;
    let mut reported: u64 = 0;
    with_buffer(len, |buf| {
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err(CopyError::Cancelled);
            }
            let Some(data) = seek(pos, libc::SEEK_DATA).map_err(CopyError::Io)? else { break };
            let hole = seek(data, libc::SEEK_HOLE).map_err(CopyError::Io)?.unwrap_or(size.max(data));
            let mut off = data;
            while off < hole {
                if cancelled.load(Ordering::Relaxed) {
                    return Err(CopyError::Cancelled);
                }
                let want = usize::try_from(hole - off).unwrap_or(usize::MAX).min(buf.len());
                let n = src.read_at(&mut buf[..want], off).map_err(CopyError::Io)?;
                if n == 0 {
                    break;
                }
                dst.write_all_at(&buf[..n], off).map_err(CopyError::Io)?;
                off += n as u64;
            }
            if off == data {
                break; // file shrank under us: no progress possible
            }
            // Count the hole before this segment and the segment itself.
            bytes_copied.fetch_add(off - reported, Ordering::Relaxed);
            reported = off;
            pos = off;
            if off >= size && hole >= size {
                break;
            }
        }
        let end = size.max(pos);
        dst.set_len(end).map_err(CopyError::Io)?;
        bytes_copied.fetch_add(end.saturating_sub(reported), Ordering::Relaxed);
        Ok(Some(end))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn copies_everything_and_counts() {
        let data = vec![42u8; 5 * 1024 * 1024 + 17];
        let mut out = Vec::new();
        let counter = Arc::new(AtomicU64::new(0));
        let n = chunked_copy_with_progress(
            &mut Cursor::new(&data), &mut out, data.len() as u64, 1024 * 1024, &counter, &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(n, data.len() as u64);
        assert_eq!(out, data);
        assert_eq!(counter.load(Ordering::Relaxed), data.len() as u64);
    }

    #[test]
    fn copies_past_stale_size_hint() {
        // The file grew after the scan: the copy must not truncate it.
        let data = vec![1u8; 300_000];
        let mut out = Vec::new();
        let counter = Arc::new(AtomicU64::new(0));
        chunked_copy_with_progress(&mut Cursor::new(&data), &mut out, 10, 1 << 20, &counter, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(out.len(), 300_000);
    }

    #[test]
    fn cancel_stops_before_reading() {
        let data = vec![1u8; 1024];
        let mut out = Vec::new();
        let counter = Arc::new(AtomicU64::new(0));
        let r = chunked_copy_with_progress(&mut Cursor::new(&data), &mut out, 1024, 1024, &counter, &AtomicBool::new(true));
        assert!(matches!(r, Err(CopyError::Cancelled)));
        assert!(out.is_empty());
    }

    /// Reader that sets the cancel flag after its first read.
    struct CancelAfterFirst<'a> {
        inner: Cursor<Vec<u8>>,
        flag: &'a AtomicBool,
    }
    impl Read for CancelAfterFirst<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.inner.read(buf)?;
            self.flag.store(true, Ordering::Relaxed);
            Ok(n)
        }
    }

    #[test]
    fn cancel_mid_copy_stops_after_current_chunk() {
        let flag = AtomicBool::new(false);
        let mut src = CancelAfterFirst { inner: Cursor::new(vec![7u8; 1 << 20]), flag: &flag };
        let mut out = Vec::new();
        let counter = Arc::new(AtomicU64::new(0));
        let r = chunked_copy_with_progress(&mut src, &mut out, 1 << 20, 64 * 1024, &counter, &flag);
        assert!(matches!(r, Err(CopyError::Cancelled)));
        assert_eq!(out.len(), 64 * 1024, "exactly one chunk written before stopping");
    }

    #[test]
    fn sparse_copy_keeps_holes() {
        use std::io::{Seek, SeekFrom};
        let dir = tempfile::tempdir().unwrap();
        let sp = dir.path().join("sparse");
        let size: u64 = 64 * 1024 * 1024;
        {
            let mut f = std::fs::File::create(&sp).unwrap();
            f.write_all(b"head").unwrap();
            f.seek(SeekFrom::Start(size - 4)).unwrap();
            f.write_all(b"tail").unwrap();
        }
        let src = std::fs::File::open(&sp).unwrap();
        let dpath = dir.path().join("out");
        let dst = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(&dpath).unwrap();
        let counter = Arc::new(AtomicU64::new(0));
        let r = sparse_copy(&src, &dst, size, 1 << 20, &counter, &AtomicBool::new(false)).unwrap();
        drop(dst);
        let Some(n) = r else { return }; // fs without hole support: nothing to check
        assert_eq!(n, size);
        assert_eq!(counter.load(Ordering::Relaxed), size);
        let out = std::fs::read(&dpath).unwrap();
        assert_eq!(out.len() as u64, size);
        assert_eq!(&out[..4], b"head");
        assert_eq!(&out[out.len() - 4..], b"tail");
        assert!(out[4..out.len() - 4].iter().all(|&b| b == 0));
        let blocks = std::os::unix::fs::MetadataExt::blocks(&std::fs::metadata(&dpath).unwrap());
        assert!(blocks * 512 < size / 2, "destination must stay sparse ({} bytes allocated)", blocks * 512);
    }
}
