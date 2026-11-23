//! macOS APFS copy-on-write clone via `clonefile(2)`.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// `<sys/clonefile.h>`: do not follow a symlink at `src`.
const CLONE_NOFOLLOW: u32 = 0x0001;

/// Clone `src` to `dst` using `clonefile(2)` (APFS copy-on-write).
///
/// Near-instant and shares physical blocks until one side is written.
/// `dst` must not exist; callers clone to a temp name and rename it into
/// place. Unless `follow_symlink` (`-L`/`-H`), a symlink at `src` is never
/// followed (`CLONE_NOFOLLOW`).
///
/// # Errors
/// Any `clonefile` error (`ENOTSUP` on non-APFS, `EXDEV` across volumes…).
pub fn clonefile(src: &Path, dst: &Path, follow_symlink: bool) -> io::Result<()> {
    let src_c = CString::new(src.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "source path has interior NUL"))?;
    let dst_c = CString::new(dst.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "dest path has interior NUL"))?;
    let flags = if follow_symlink { 0 } else { CLONE_NOFOLLOW };
    let rc = unsafe { libc::clonefile(src_c.as_ptr(), dst_c.as_ptr(), flags) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// True for errors meaning "this volume can never clone" (stop trying).
#[must_use]
pub fn clone_permanently_unsupported(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(libc::ENOTSUP | libc::EXDEV))
}
