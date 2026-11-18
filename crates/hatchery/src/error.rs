//! The one error type of the crate.
//!
//! The variants that name a reason a package cannot be installed are stable:
//! tools match on them and show their `Display` text to people.

use std::fmt;
use std::path::PathBuf;

use crate::platform::Platform;
use crate::time::Timestamp;

/// The result type of this crate. The error is [`Error`] unless another is given.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The package, version and registry an error is about. Any part can be
/// unknown when the error happens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct At {
    /// `group/id`, or the id alone when the group is not known yet.
    pub package: String,
    /// A version, or the range that was asked for.
    pub version: Option<String>,
    /// The label of the registry or source.
    pub registry: Option<String>,
}

impl At {
    /// Starts with a package, written as `group/id` or as an id alone. The version and the registry are unset.
    pub fn new(package: impl fmt::Display) -> Self {
        At {
            package: package.to_string(),
            version: None,
            registry: None,
        }
    }

    /// Sets the version, or the range that was asked for.
    pub fn version(mut self, version: impl fmt::Display) -> Self {
        self.version = Some(version.to_string());
        self
    }

    /// Sets the label of the registry or source.
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

/// What can go wrong in this crate. The enum is `non_exhaustive`, so a `match` needs a wildcard arm.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The bytes do not have the checksum the index or the lock file gives.
    #[error("{at}: checksum mismatch, expected sha256 {expected}, got {actual}")]
    ChecksumMismatch {
        /// The package, version and registry of the egg.
        at: At,
        /// The SHA-256 the index or the lock file gives, in lowercase hex.
        expected: String,
        /// The SHA-256 of the bytes that were read, in lowercase hex.
        actual: String,
    },

    /// The registry has a key and the egg is not signed by it.
    #[error("{at}: signature not valid: {reason}")]
    SignatureInvalid {
        /// The package, version and registry.
        at: At,
        /// Why the signature is refused, such as that it is missing or does not verify.
        reason: String,
    },

    /// A line of a target's `requires` block does not hold.
    #[error("{at}: target {target} requires {fact}{wanted}, {found}", wanted = wanted.iter().map(|w| format!(" {w}")).collect::<String>(), found = match found { Some(f) => format!("but it is {f}"), None => "which is not present".to_string() })]
    Unmet {
        /// The package, version and registry.
        at: At,
        /// The target whose `requires` block does not hold.
        target: String,
        /// The fact name of the line that fails, such as `os` or `rookery`.
        fact: String,
        /// The values the line accepts. Empty when the line only asks for the fact to be present.
        wanted: Vec<String>,
        /// What the tool answered for the fact. None when it has no such fact.
        found: Option<String>,
    },

    /// The target declares variants and none is for this platform.
    #[error("{at}: target {target} has no variant for {platform}")]
    NoVariant {
        /// The package, version and registry.
        at: At,
        /// The target that declares the variants.
        target: String,
        /// The platform the target was to be installed for.
        platform: Platform,
    },

    /// The version is younger than the registry's minimum release age.
    #[error("{at}: too new to install until {until}")]
    TooYoung {
        /// The package, version and registry.
        at: At,
        /// When the version becomes installable.
        until: Timestamp,
    },

    /// The version was pulled by its publisher.
    #[error("{at}: withdrawn{}", reason.as_ref().map(|r| format!(" ({r})")).unwrap_or_default())]
    Withdrawn {
        /// The package, version and registry.
        at: At,
        /// The publisher's reason. None when none was given.
        reason: Option<String>,
    },

    /// A git registry has no built egg for the version, and a wanted target
    /// needs a build.
    #[error("{at}: no built egg, and {} a build", match targets.as_slice() { [t] => format!("target {t} needs"), ts => format!("targets {} need", ts.join(", ")) })]
    NotBuilt {
        /// The package, version and registry.
        at: At,
        /// The wanted targets that need a build.
        targets: Vec<String>,
    },

    /// A tag points at a different commit than the lock file records.
    #[error("{at}: tag {tag} moved from {locked} to {now}")]
    TagMoved {
        /// The package, version and registry.
        at: At,
        /// The name of the tag.
        tag: String,
        /// The commit `eggs.lock` records for the tag.
        locked: String,
        /// The commit the tag points at now.
        now: String,
    },

    /// A package cannot be removed while others that depend on it are installed.
    #[error("{at}: required by {}", dependents.join(", "))]
    RequiredBy {
        /// The package that was to be removed.
        at: At,
        /// The installed packages that depend on it, as `group/id`.
        dependents: Vec<String>,
    },

    /// No registry has the package, or no version in the range.
    #[error("{at}: not found")]
    NotFound {
        /// The package, with the range that was asked for as its version when there is one.
        at: At,
    },

    /// An id alone matches packages of several groups.
    #[error("{id} is offered by several groups ({}); name one as group/id", groups.join(", "))]
    Ambiguous {
        /// The id that was asked for, without a group.
        id: String,
        /// The groups that offer a package with that id.
        groups: Vec<String>,
    },

    /// Versions were found, but no set of them satisfies every range.
    #[error("{at}: {message}")]
    Conflict {
        /// The package, version and registry the conflict was found for.
        at: At,
        /// What does not fit, as a sentence.
        message: String,
    },

    /// A manifest, configuration, lock file or index is not valid.
    #[error("{what}: {message}")]
    Invalid {
        /// What was being read, such as a file path or a line of a configuration.
        what: String,
        /// What is wrong with it.
        message: String,
    },

    /// An egg holds an entry that unpacking refuses.
    #[error("unsafe archive: {0}")]
    UnsafeArchive(String),

    /// A target's own check refused its files.
    #[error("target {target}: {}", problems.iter().map(|p| p.to_string()).collect::<Vec<_>>().join("; "))]
    Check {
        /// The target whose check refused its files.
        target: String,
        /// What the check found, one [`Problem`](crate::target::Problem) for each.
        problems: Vec<crate::target::Problem>,
    },

    /// A build command failed.
    #[error("build command {command:?} failed: {message}")]
    Build {
        /// The command that was run.
        command: String,
        /// Why it failed, such as its exit status.
        message: String,
    },

    /// The git command failed.
    #[error("git {args}: {message}")]
    Git {
        /// The arguments git was given, with any password in an address removed.
        args: String,
        /// What git printed, or why it could not be run.
        message: String,
    },

    /// A fetch over the network failed.
    #[error("{url}: {message}")]
    Http {
        /// The address that was fetched.
        url: String,
        /// Why the fetch failed, such as a status code or a network error.
        message: String,
    },

    /// An I/O error, with the path it was about.
    #[error("{}: {source}", path.display())]
    Io {
        /// The file or folder the operation was on.
        path: PathBuf,
        /// The error the operating system gave.
        #[source]
        source: std::io::Error,
    },

    /// An error that fits no other variant. The text is a sentence for people.
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// An [`Error::Invalid`]. `what` names the thing that was read, and `message` says what is wrong with it.
    pub fn invalid(what: impl fmt::Display, message: impl fmt::Display) -> Self {
        Error::Invalid {
            what: what.to_string(),
            message: message.to_string(),
        }
    }

    /// An [`Error::Other`] with this text.
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

#[cfg(test)]
mod unmet_tests {
    use super::*;

    fn unmet(wanted: &[&str], found: Option<&str>) -> String {
        Error::Unmet {
            at: At::new("example/demo"),
            target: "crow".into(),
            fact: "gpu".into(),
            wanted: wanted.iter().map(|w| w.to_string()).collect(),
            found: found.map(String::from),
        }
        .to_string()
    }

    #[test]
    fn unmet_has_no_stray_space_when_nothing_is_wanted() {
        assert!(unmet(&[], None).ends_with("requires gpu, which is not present"));
        assert!(unmet(&["1", "2"], Some("3")).ends_with("requires gpu 1 2, but it is 3"));
    }
}
