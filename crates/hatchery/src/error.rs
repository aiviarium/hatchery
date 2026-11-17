//! The one error type of the crate.
//!
//! The variants that name a reason a package cannot be installed are stable:
//! tools match on them and show their `Display` text to people.

use std::fmt;
use std::path::PathBuf;

use crate::platform::Platform;
use crate::time::Timestamp;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The package, version and registry an error is about. Any part can be
/// unknown when the error happens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct At {
    /// `group/id`, or the id alone when the group is not known yet.
    pub package: String,
    /// A version, or the range that was asked for.
    pub version: Option<String>,
    /// The label of the registry or source.
    pub registry: Option<String>,
}

impl At {
    pub fn new(package: impl fmt::Display) -> Self {
        At {
            package: package.to_string(),
            version: None,
            registry: None,
        }
    }

    pub fn version(mut self, version: impl fmt::Display) -> Self {
        self.version = Some(version.to_string());
        self
    }

    pub fn registry(mut self, registry: impl fmt::Display) -> Self {
        self.registry = Some(registry.to_string());
        self
    }
}

impl fmt::Display for At {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.package)?;
        if let Some(v) = &self.version {
            write!(f, " {v}")?;
        }
        if let Some(r) = &self.registry {
            write!(f, " from {r}")?;
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The bytes do not have the checksum the index or the lock file gives.
    #[error("{at}: checksum mismatch, expected sha256 {expected}, got {actual}")]
    ChecksumMismatch {
        at: At,
        expected: String,
        actual: String,
    },

    /// The registry has a key and the egg is not signed by it.
    #[error("{at}: signature not valid: {reason}")]
    SignatureInvalid { at: At, reason: String },

    /// A line of a target's `requires` block does not hold.
    #[error("{at}: target {target} requires {fact} {wanted}, {found}", wanted = wanted.join(" "), found = match found { Some(f) => format!("but it is {f}"), None => "which is not present".to_string() })]
    Unmet {
        at: At,
        target: String,
        fact: String,
        wanted: Vec<String>,
        found: Option<String>,
    },

    /// The target declares variants and none is for this platform.
    #[error("{at}: target {target} has no variant for {platform}")]
    NoVariant {
        at: At,
        target: String,
        platform: Platform,
    },

    /// The version is younger than the registry's minimum release age.
    #[error("{at}: too new to install until {until}")]
    TooYoung { at: At, until: Timestamp },

    /// The version was pulled by its publisher.
    #[error("{at}: withdrawn{}", reason.as_ref().map(|r| format!(" ({r})")).unwrap_or_default())]
    Withdrawn { at: At, reason: Option<String> },

    /// A git registry has no built egg for the version, and a wanted target
    /// needs a build.
    #[error("{at}: no built egg, and {} a build", match targets.as_slice() { [t] => format!("target {t} needs"), ts => format!("targets {} need", ts.join(", ")) })]
    NotBuilt { at: At, targets: Vec<String> },

    /// A tag points at a different commit than the lock file records.
    #[error("{at}: tag {tag} moved from {locked} to {now}")]
    TagMoved {
        at: At,
        tag: String,
        locked: String,
        now: String,
    },

    /// A package cannot be removed while others that depend on it are installed.
    #[error("{at}: required by {}", dependents.join(", "))]
    RequiredBy { at: At, dependents: Vec<String> },

    /// No registry has the package, or no version in the range.
    #[error("{at}: not found")]
    NotFound { at: At },

    /// An id alone matches packages of several groups.
    #[error("{id} is offered by several groups ({}); name one as group/id", groups.join(", "))]
    Ambiguous { id: String, groups: Vec<String> },

    /// Versions were found, but no set of them satisfies every range.
    #[error("{at}: {message}")]
    Conflict { at: At, message: String },

    /// A manifest, configuration, lock file or index is not valid.
    #[error("{what}: {message}")]
    Invalid { what: String, message: String },

    /// An egg holds an entry that unpacking refuses.
    #[error("unsafe archive: {0}")]
    UnsafeArchive(String),

    /// A target's own check refused its files.
    #[error("target {target}: {}", problems.iter().map(|p| p.to_string()).collect::<Vec<_>>().join("; "))]
    Check {
        target: String,
        problems: Vec<crate::target::Problem>,
    },

    /// A build command failed.
    #[error("build command {command:?} failed: {message}")]
    Build { command: String, message: String },

    /// The git command failed.
    #[error("git {args}: {message}")]
    Git { args: String, message: String },

    /// A fetch over the network failed.
    #[error("{url}: {message}")]
    Http { url: String, message: String },

    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn invalid(what: impl fmt::Display, message: impl fmt::Display) -> Self {
        Error::Invalid {
            what: what.to_string(),
            message: message.to_string(),
        }
    }

    pub fn other(message: impl fmt::Display) -> Self {
        Error::Other(message.to_string())
    }

    /// The package, version and registry, for the variants that carry them.
    pub fn at(&self) -> Option<&At> {
        match self {
            Error::ChecksumMismatch { at, .. }
            | Error::SignatureInvalid { at, .. }
            | Error::Unmet { at, .. }
            | Error::NoVariant { at, .. }
            | Error::TooYoung { at, .. }
            | Error::Withdrawn { at, .. }
            | Error::NotBuilt { at, .. }
            | Error::TagMoved { at, .. }
            | Error::RequiredBy { at, .. }
            | Error::NotFound { at }
            | Error::Conflict { at, .. } => Some(at),
            _ => None,
        }
    }
}

/// Adds the path to an I/O error.
pub(crate) trait IoContext<T> {
    fn at_path(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn at_path(self, path: impl Into<PathBuf>) -> Result<T> {
        self.map_err(|source| Error::Io {
            path: path.into(),
            source,
        })
    }
}
