//! macOS APFS copy-on-write clone via `clonefile(2)`.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// Clone `src` to `dst` using `clonefile(2)` (APFS copy-on-write).
///
/// This is near-instant and shares physical blocks until one side is
/// written, with no data duplication. It requires both paths to live on
/// the same APFS volume; on any failure the caller should fall back to a
/// regular copy.
///
/// `dst` must NOT already exist (clonefile fails with `EEXIST` otherwise) —
/// the caller clones to a temp path adjacent to the real destination and
/// renames it into place to preserve atomicity.
pub fn clonefile(src: &Path, dst: &Path) -> io::Result<()> {
    let src_c = CString::new(src.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "source path has interior NUL"))?;
    let dst_c = CString::new(dst.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "dest path has interior NUL"))?;

    // flags = 0: follow symlinks is irrelevant here since we only clone
    // regular files (the planner filters by kind).
    let rc = unsafe { libc::clonefile(src_c.as_ptr(), dst_c.as_ptr(), 0) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
