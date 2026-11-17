//! A git repository with a `packages.kdl`, used as a registry.

use std::path::PathBuf;

use crate::git::Git;
use crate::sign::VerifyingKey;

/// A git repository used as a registry.
pub struct GitRegistry {
    _private: (),
}

impl GitRegistry {
    /// `cache` is the folder for this crate's git caches; each repository
    /// gets its own folder in it. `key` is required of built eggs.
    pub fn new(
        url: impl Into<String>,
        cache: impl Into<PathBuf>,
        git: Git,
        key: Option<VerifyingKey>,
    ) -> GitRegistry {
        let _ = (url.into(), cache.into(), git, key);
        todo!()
    }
}
