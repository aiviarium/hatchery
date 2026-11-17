//! Checksums of bytes and of installed folders.

use std::fs;
use std::path::Path;

use crate::error::{Error, IoContext, Result};

/// Lowercase hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    crate::fsx::hex(&Sha256::digest(bytes))
}

/// A digest of a folder's files: their paths, whether each is executable,
/// and their contents. The same files give the same digest on any machine.
/// Symlinks are refused.
pub fn tree_digest(dir: &Path) -> Result<String> {
    let mut lines = Vec::new();
    collect(dir, "", &mut lines)?;
    lines.sort();
    let mut text = Vec::new();
    for l in lines {
        text.extend_from_slice(l.as_bytes());
    }
    Ok(sha256_hex(&text))
}

fn collect(dir: &Path, prefix: &str, out: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(dir).at_path(dir)? {
        let entry = entry.at_path(dir)?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = format!("{prefix}{name}");
        let meta = fs::symlink_metadata(&path).at_path(&path)?;
        if meta.file_type().is_symlink() {
            return Err(Error::invalid(
                path.display(),
                "is a symlink, which a package cannot hold",
            ));
        }
        if meta.is_dir() {
            collect(&path, &format!("{rel}/"), out)?;
        } else if meta.is_file() {
            let data = fs::read(&path).at_path(&path)?;
            let x = if is_executable(&meta) { "x" } else { "-" };
            out.push(format!("{rel}\0{x}\0{}\n", sha256_hex(&data)));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for (p, c) in files {
            let f = d.path().join(p);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, c).unwrap();
        }
        d
    }

    #[test]
    fn same_files_same_digest() {
        let a = tree(&[("a.txt", "1"), ("d/b.txt", "2")]);
        let b = tree(&[("d/b.txt", "2"), ("a.txt", "1")]);
        assert_eq!(
            tree_digest(a.path()).unwrap(),
            tree_digest(b.path()).unwrap()
        );
        assert_eq!(tree_digest(a.path()).unwrap().len(), 64);
    }

    #[test]
    fn changes_with_content_and_path() {
        let base = tree_digest(tree(&[("a.txt", "1")]).path()).unwrap();
        assert_ne!(base, tree_digest(tree(&[("a.txt", "2")]).path()).unwrap());
        assert_ne!(base, tree_digest(tree(&[("b.txt", "1")]).path()).unwrap());
        assert_ne!(
            base,
            tree_digest(tree(&[("a.txt", "1"), ("c", "")]).path()).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn changes_with_the_executable_bit_and_refuses_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let d = tree(&[("run.sh", "x")]);
        let before = tree_digest(d.path()).unwrap();
        fs::set_permissions(d.path().join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        assert_ne!(before, tree_digest(d.path()).unwrap());
        std::os::unix::fs::symlink("run.sh", d.path().join("link")).unwrap();
        assert!(tree_digest(d.path()).is_err());
    }
}
