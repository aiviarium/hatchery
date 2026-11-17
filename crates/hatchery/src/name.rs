//! Package names (`group/id`) and what `add` is asked to install.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use semver::{Version, VersionReq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};

/// A package's group and id, written `group/id`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name {
    pub group: String,
    pub id: String,
}

/// Checks that a group or an id can be a folder name on every system:
/// ASCII letters, digits, `-`, `_` and `.`, not starting with `.` or `-`,
/// and not `eggs.lock`, the lock file beside the group folders.
pub fn validate_segment(kind: &str, s: &str) -> Result<()> {
    let ok = !s.is_empty()
        && s.len() <= 100
        && !s.starts_with('.')
        && !s.starts_with('-')
        && s != crate::lock::FILE
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if ok {
        Ok(())
    } else {
        Err(Error::invalid(
            kind,
            format!(
                "{s:?} must be ASCII letters, digits, '-', '_' or '.', not start with '.' or '-', and not be eggs.lock"
            ),
        ))
    }
}

impl Name {
    pub fn new(group: impl Into<String>, id: impl Into<String>) -> Result<Name> {
        let (group, id) = (group.into(), id.into());
        validate_segment("group", &group)?;
        validate_segment("id", &id)?;
        Ok(Name { group, id })
    }

    /// Parses `group/id`.
    pub fn parse(s: &str) -> Result<Name> {
        match s.split_once('/') {
            Some((g, i)) => Name::new(g, i),
            None => Err(Error::invalid(
                "package name",
                format!("{s:?} is not group/id"),
            )),
        }
    }

    /// The relative folder a package is installed in, `group/id`.
    pub fn dir(&self) -> PathBuf {
        PathBuf::from(&self.group).join(&self.id)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.group, self.id)
    }
}

impl FromStr for Name {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Name::parse(s)
    }
}

impl Serialize for Name {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Name {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Name::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// Parses a range in Cargo's syntax (`^1.2`, `~1.4.0`, `>=1.0, <2.0`,
/// `1.*`, `=1.2.3`; a bare `1.2` means `^1.2`). An empty string means any.
pub fn parse_range(s: &str) -> Result<VersionReq> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(VersionReq::STAR);
    }
    VersionReq::parse(s).map_err(|e| Error::invalid("version range", format!("{s:?}: {e}")))
}

/// Parses a version, which must be complete semver (`1.4.0`).
pub fn parse_version(s: &str) -> Result<Version> {
    Version::parse(s.trim()).map_err(|e| Error::invalid("version", format!("{s:?}: {e}")))
}

/// A package named in a registry: `group/id` or the id alone, with an
/// optional range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    /// None when the id alone was given; the registries then decide.
    pub group: Option<String>,
    pub id: String,
    /// None means the newest allowed version.
    pub range: Option<VersionReq>,
    /// A branch, tag or commit of a git registry, for development.
    pub git_ref: Option<String>,
}

impl Wanted {
    pub fn range(&self) -> VersionReq {
        self.range.clone().unwrap_or(VersionReq::STAR)
    }

    /// `group/id` or `id`, for messages.
    pub fn label(&self) -> String {
        match &self.group {
            Some(g) => format!("{g}/{}", self.id),
            None => self.id.clone(),
        }
    }
}

/// What to install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spec {
    /// From the configured registries: `aiviarium/claude@^1.4` or `claude`.
    Registry(Wanted),
    /// One folder of a git repository at one ref. No ranges.
    Git {
        url: String,
        /// The package's folder in the repository; the root when None.
        path: Option<String>,
        /// A tag, branch or commit; the default branch when None.
        git_ref: Option<String>,
    },
    /// A package folder on disk, for development.
    Folder(PathBuf),
    /// An egg file, or an `http(s)://` address of one.
    File(String),
}

impl Spec {
    /// Parses what a person types after `add`.
    ///
    /// - `./claude`, `../x`, `/abs/x` or `~/x`: a folder, or a file when it
    ///   ends in `.egg`.
    /// - `https://…` or `http://…`: an egg file over the network.
    /// - anything ending in `.egg`: a file.
    /// - `group/id[@range]` or `id[@range]`: a registry package. A complete
    ///   version such as `1.4.0` means exactly that version; anything else is
    ///   a range in Cargo's syntax.
    pub fn parse(s: &str) -> Result<Spec> {
        let s = s.trim();
        if s.is_empty() {
            return Err(Error::invalid("package", "empty"));
        }
        if s.starts_with("https://") || s.starts_with("http://") {
            return Ok(Spec::File(s.to_string()));
        }
        let pathlike = s.starts_with("./")
            || s.starts_with("../")
            || s.starts_with('/')
            || s.starts_with("~/")
            || s == "."
            || s == "..";
        if s.ends_with(".egg") {
            return Ok(Spec::File(s.to_string()));
        }
        if pathlike {
            return Ok(Spec::Folder(PathBuf::from(s)));
        }
        let (name, range) = match s.split_once('@') {
            Some((n, r)) => (n, Some(r)),
            None => (s, None),
        };
        let (group, id) = match name.split_once('/') {
            Some((g, i)) => {
                validate_segment("group", g)?;
                (Some(g.to_string()), i)
            }
            None => (None, name),
        };
        validate_segment("id", id)?;
        let range = match range {
            None => None,
            Some(r) if r.trim().is_empty() => {
                return Err(Error::invalid(
                    "package",
                    format!("empty range after '@' in {s:?}"),
                ));
            }
            Some(r) => Some(match Version::parse(r.trim()) {
                Ok(v) => exact(&v),
                Err(_) => parse_range(r)?,
            }),
        };
        Ok(Spec::Registry(Wanted {
            group,
            id: id.to_string(),
            range,
            git_ref: None,
        }))
    }

    /// One folder of a git repository.
    pub fn git(url: impl Into<String>, path: Option<String>, git_ref: Option<String>) -> Spec {
        Spec::Git {
            url: url.into(),
            path,
            git_ref,
        }
    }
}

impl FromStr for Spec {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Spec::parse(s)
    }
}

/// The range that matches exactly one version.
pub fn exact(v: &Version) -> VersionReq {
    VersionReq {
        comparators: vec![semver::Comparator {
            op: semver::Op::Exact,
            major: v.major,
            minor: Some(v.minor),
            patch: Some(v.patch),
            pre: v.pre.clone(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_and_specs() {
        let n = Name::parse("aiviarium/claude").unwrap();
        assert_eq!(n.to_string(), "aiviarium/claude");
        assert!(Name::parse("claude").is_err());
        assert!(Name::parse("../x").is_err());
        assert!(Name::new("a", ".hidden").is_err());
        assert!(Name::new("-a", "x").is_err());
        assert!(Name::new("eggs.lock", "x").is_err());

        let Spec::Registry(w) = Spec::parse("aiviarium/claude@^1.4").unwrap() else {
            panic!()
        };
        assert_eq!(w.group.as_deref(), Some("aiviarium"));
        assert!(w.range().matches(&Version::new(1, 5, 0)));

        let Spec::Registry(w) = Spec::parse("claude@1.4.1").unwrap() else {
            panic!()
        };
        assert_eq!(w.group, None);
        assert!(w.range().matches(&Version::new(1, 4, 1)));
        assert!(!w.range().matches(&Version::new(1, 4, 2)));

        assert_eq!(
            Spec::parse("./claude").unwrap(),
            Spec::Folder("./claude".into())
        );
        assert_eq!(
            Spec::parse("dist/claude-1.4.0.egg").unwrap(),
            Spec::File("dist/claude-1.4.0.egg".into())
        );
        assert_eq!(
            Spec::parse("https://example.org/claude-1.4.0.egg").unwrap(),
            Spec::File("https://example.org/claude-1.4.0.egg".into())
        );
        assert!(Spec::parse("claude@").is_err());
        assert!(Spec::parse("a b").is_err());
    }
}
