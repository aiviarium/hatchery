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

/// A repository address fit for a lock file, an egg or a message: the
/// user and password of an `http(s)://` or `file://` URL are removed, and
/// the password of any other URL. `user@host:path` is kept, since the user
/// there names an account, not a secret.
pub fn redact_url(remote: &str) -> String {
    if !remote.contains("://") {
        return remote.to_string();
    }
    match url::Url::parse(remote) {
        Ok(mut u) if !u.username().is_empty() || u.password().is_some() => {
            let _ = u.set_password(None);
            if matches!(u.scheme(), "http" | "https" | "file") {
                let _ = u.set_username("");
            }
            u.to_string()
        }
        Ok(_) => remote.to_string(),
        // Not a URL we can parse: drop everything up to an `@` after the scheme.
        Err(_) => match remote.split_once("://") {
            Some((scheme, rest)) => match rest.split_once('@') {
                Some((_, host)) if !rest.split('/').next().unwrap_or("").is_empty() => {
                    format!("{scheme}://{host}")
                }
                _ => remote.to_string(),
            },
            None => remote.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_credentials() {
        assert_eq!(
            redact_url("https://ci:ghp_SECRET@example.org/x.git"),
            "https://example.org/x.git"
        );
        assert_eq!(
            redact_url("https://TOKEN@example.org/x.git"),
            "https://example.org/x.git"
        );
        assert_eq!(
            redact_url("ssh://git:pw@example.org/x.git"),
            "ssh://git@example.org/x.git"
        );
        assert_eq!(
            redact_url("git@example.org:org/x.git"),
            "git@example.org:org/x.git"
        );
        assert_eq!(redact_url("/srv/repo.git"), "/srv/repo.git");
    }
}
