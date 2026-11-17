//! File writes that never leave a half-written file behind.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{IoContext, Result};

/// A name no other writer in this or another process is using.
pub fn nonce() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{n}-{t:x}", std::process::id())
}

/// The parent folder of a path, `.` for a bare file name.
pub fn parent_of(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// A path beside `dest` for staging a write.
pub fn staging_path(dest: &Path) -> PathBuf {
    let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    parent_of(dest).join(format!(".{name}.tmp-{}", nonce()))
}

/// Writes a file through a temporary file and a rename.
pub fn write_atomic(dest: &Path, bytes: &[u8]) -> Result<()> {
    let parent = parent_of(dest);
    fs::create_dir_all(parent).at_path(parent)?;
    let tmp = staging_path(dest);
    if let Err(e) = fs::write(&tmp, bytes) {
        let _ = fs::remove_file(&tmp);
        return Err(e).at_path(&tmp);
    }
    if let Err(e) = fs::rename(&tmp, dest) {
        let _ = fs::remove_file(&tmp);
        return Err(e).at_path(dest);
    }
    Ok(())
}

/// Moves a staged folder into place, replacing what is there. The old
/// folder is put back if the move fails.
pub fn replace_dir(staged: &Path, dest: &Path) -> Result<()> {
    let parent = parent_of(dest);
    fs::create_dir_all(parent).at_path(parent)?;
    if dest.exists() {
        let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("dir");
        let backup = parent.join(format!(".{name}.old-{}", nonce()));
        fs::rename(dest, &backup).at_path(dest)?;
        if let Err(e) = fs::rename(staged, dest) {
            let _ = fs::rename(&backup, dest);
            return Err(e).at_path(dest);
        }
        let _ = fs::remove_dir_all(&backup);
    } else {
        fs::rename(staged, dest).at_path(dest)?;
    }
    Ok(())
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Decodes hex of either case.
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}
