//! One folder of a git repository at one tag, branch or commit.

use std::path::PathBuf;

use crate::git::Git;

/// One package folder of a git repository at one ref. The repository needs
/// no `packages.kdl`; when it has one, its defaults apply.
pub struct GitFolder {
    _private: (),
}

impl GitFolder {
    pub fn new(
        url: impl Into<String>,
        path: Option<String>,
        git_ref: Option<String>,
        cache: impl Into<PathBuf>,
        git: Git,
    ) -> GitFolder {
        let _ = (url.into(), path, git_ref, cache.into(), git);
        todo!()
    }
}
