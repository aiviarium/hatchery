//! Checksums of bytes and of installed folders.

use std::path::Path;

use crate::error::Result;

/// Lowercase hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    crate::fsx::hex(&Sha256::digest(bytes))
}

/// A digest of a folder's files: their paths, whether each is executable,
/// and their contents. The same files give the same digest on any machine.
pub fn tree_digest(dir: &Path) -> Result<String> {
    let _ = dir;
    todo!()
}
