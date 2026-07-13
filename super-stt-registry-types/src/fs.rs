// SPDX-License-Identifier: GPL-3.0-only
//! Small filesystem helpers shared by the daemon and the indexer.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Atomically write `contents` to `path`: write a sibling temp file, flush and
/// `fsync` it, then rename it over `path`. A crash mid-write leaves either the
/// old file or the new one, never a truncated mix. The temp file lives in the
/// same directory so the rename stays on one filesystem (a cross-device rename
/// is not atomic).
///
/// Replaces the daemon's cache-write and the indexer's two `index.json` writers
/// (which wrote in place, non-atomically, and disagreed on a trailing newline).
///
/// # Errors
/// Returns any I/O error from creating/writing/syncing the temp file or the
/// final rename. On error the temp file is best-effort removed.
pub fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);

    // Scope the file handle so it is closed before the rename.
    let write_result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(contents)?;
        f.flush()?;
        f.sync_all()
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::write_atomic;

    #[test]
    fn writes_and_overwrites_atomically() {
        let dir = std::env::temp_dir().join(format!("stt-write-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.json");

        write_atomic(&path, b"first").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        // Overwriting replaces the contents and leaves no `.tmp` sibling.
        write_atomic(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        let mut tmp = path.clone().into_os_string();
        tmp.push(".tmp");
        assert!(
            !std::path::Path::new(&tmp).exists(),
            "temp file must be gone"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
