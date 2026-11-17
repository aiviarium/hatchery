//! The operating system and architecture a variant is for.

use std::fmt;

use serde::{Deserialize, Serialize};

/// An OS and an architecture, named as Rust names them (`linux`, `macos`,
/// `windows`; `x86_64`, `aarch64`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Platform {
    pub os: String,
    pub arch: String,
}

impl Platform {
    pub fn new(os: impl Into<String>, arch: impl Into<String>) -> Platform {
        Platform {
            os: os.into(),
            arch: arch.into(),
        }
    }

    /// The machine this program runs on.
    pub fn current() -> Platform {
        Platform::new(std::env::consts::OS, std::env::consts::ARCH)
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.os, self.arch)
    }
}

/// Which variants a target fetches when a package is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Variants {
    /// The variant for this platform, at install. A target that declares
    /// variants and has none for it fails with `NoVariant`.
    Matching(Platform),
    /// None at install; [`crate::Client::variant`] fetches each when it is
    /// first needed.
    OnDemand,
}
