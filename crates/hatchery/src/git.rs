//! The `git` command, and the per-repository cache of shallow, sparse
//! checkouts.

use std::path::PathBuf;

/// How to run git.
#[derive(Debug, Clone)]
pub struct Git {
    pub program: PathBuf,
}

impl Default for Git {
    fn default() -> Self {
        Git {
            program: PathBuf::from("git"),
        }
    }
}

impl Git {
    pub fn new() -> Git {
        Git::default()
    }
}
