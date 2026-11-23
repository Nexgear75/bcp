//! Metadata preservation applied to the temp file before it is renamed
//! into place (so the destination appears with final attributes atomically).

use std::path::Path;

use crate::config::Preserve;

/// Apply the requested metadata from `src` onto `dst` (the temp file).
///
/// Best-effort: a failure to copy one attribute is reported as an error
/// string but never corrupts the data already written. Callers decide
/// whether to treat it as fatal (strict mode) or collect it as a warning.
pub fn apply(src: &Path, dst: &Path, preserve: Preserve) -> Result<(), String> {
    if !preserve.any() {
        return Ok(());
    }

    let meta = std::fs::symlink_metadata(src)
        .map_err(|e| format!("read source metadata: {e}"))?;

    if preserve.mode {
        let perms = meta.permissions();
        std::fs::set_permissions(dst, perms)
            .map_err(|e| format!("set permissions: {e}"))?;
    }

    if preserve.timestamps {
        let atime = filetime::FileTime::from_last_access_time(&meta);
        let mtime = filetime::FileTime::from_last_modification_time(&meta);
        filetime::set_file_times(dst, atime, mtime)
            .map_err(|e| format!("set timestamps: {e}"))?;
    }

    if preserve.xattrs {
        copy_xattrs(src, dst).map_err(|e| format!("copy xattrs: {e}"))?;
    }

    Ok(())
}

fn copy_xattrs(src: &Path, dst: &Path) -> std::io::Result<()> {
    for name in xattr::list(src)? {
        if let Some(value) = xattr::get(src, &name)? {
            // Some attributes (e.g. system ones) may be rejected; ignore
            // those rather than failing the whole copy.
            let _ = xattr::set(dst, &name, &value);
        }
    }
    Ok(())
}
