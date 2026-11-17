//! What a tool tells Hatchery about itself, and how `requires` lines are
//! matched against the facts it answers.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use bytes::Bytes;
use semver::{Version, VersionReq};

use crate::error::Result;
use crate::manifest::{Manifest, Requirement};
use crate::platform::{Platform, Variants};

/// A tool that installs packages, such as an app framework or a host
/// provisioner. Implemented in the tool's own code.
pub trait Target: Send + Sync {
    /// The name under `targets` this tool installs.
    fn name(&self) -> &str;

    /// Answers a name from a `requires` block. None means the fact is
    /// absent. `os` and `arch` fall back to [`Target::variants`]' platform,
    /// or this machine, when this returns None.
    fn fact(&self, name: &str) -> Option<String>;

    /// Fetch the variant for this machine at install (the default), or
    /// fetch variants when asked.
    fn variants(&self) -> Variants {
        Variants::Matching(Platform::current())
    }

    /// Called at pack and at install with the target's files. Reads the
    /// tool's own manifest.
    fn check(&self, files: &TargetFiles) -> Result<(), Vec<Problem>> {
        let _ = files;
        Ok(())
    }

    /// For a folder without a `package.kdl`: derive one from the tool's own
    /// manifest.
    fn derive(&self, dir: &Path) -> Result<Option<Manifest>> {
        let _ = dir;
        Ok(None)
    }
}

/// One thing a target's check found wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The file, relative to the target's folder.
    pub path: Option<String>,
    pub message: String,
}

impl Problem {
    pub fn new(message: impl Into<String>) -> Problem {
        Problem {
            path: None,
            message: message.into(),
        }
    }

    pub fn in_file(path: impl Into<String>, message: impl Into<String>) -> Problem {
        Problem {
            path: Some(path.into()),
            message: message.into(),
        }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.path {
            Some(p) => write!(f, "{p}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

/// The files of one target, by path relative to the target's folder
/// (`/`-separated). The `variants/` folder is not part of them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TargetFiles {
    pub target: String,
    pub files: BTreeMap<String, Bytes>,
}

impl TargetFiles {
    pub fn new(target: impl Into<String>, files: BTreeMap<String, Bytes>) -> TargetFiles {
        TargetFiles {
            target: target.into(),
            files,
        }
    }

    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(|b| b.as_ref())
    }

    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// A target that answers from a fixed table. The `hatchery` command uses it
/// with `--target` and `--fact`, and tests use it.
#[derive(Debug, Clone)]
pub struct StaticTarget {
    pub name: String,
    pub facts: BTreeMap<String, String>,
    pub variants: Variants,
}

impl StaticTarget {
    pub fn new(name: impl Into<String>) -> StaticTarget {
        StaticTarget {
            name: name.into(),
            facts: BTreeMap::new(),
            variants: Variants::Matching(Platform::current()),
        }
    }

    pub fn fact(mut self, name: impl Into<String>, value: impl Into<String>) -> StaticTarget {
        self.facts.insert(name.into(), value.into());
        self
    }

    pub fn variants(mut self, variants: Variants) -> StaticTarget {
        self.variants = variants;
        self
    }
}

impl Target for StaticTarget {
    fn name(&self) -> &str {
        &self.name
    }

    fn fact(&self, name: &str) -> Option<String> {
        self.facts.get(name).cloned()
    }

    fn variants(&self) -> Variants {
        self.variants.clone()
    }
}

/// The value of a fact for a target, with `os` and `arch` built in.
pub fn fact_of(target: &dyn Target, name: &str) -> Option<String> {
    if let Some(v) = target.fact(name) {
        return Some(v);
    }
    let platform = match target.variants() {
        Variants::Matching(p) => p,
        Variants::OnDemand => Platform::current(),
    };
    match name {
        "os" => Some(platform.os),
        "arch" => Some(platform.arch),
        _ => None,
    }
}

/// Parses a fact as a version, allowing `1` and `0.7` for `1.0.0` and `0.7.0`.
pub fn fact_version(s: &str) -> Option<Version> {
    if let Ok(v) = Version::parse(s) {
        return Some(v);
    }
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() < 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    {
        let mut full = parts.join(".");
        for _ in parts.len()..3 {
            full.push_str(".0");
        }
        return Version::parse(&full).ok();
    }
    None
}

impl Requirement {
    /// Whether a fact satisfies this line. A line with no values wants the
    /// fact present. When the fact is a version and a value is a range,
    /// they are compared as versions; otherwise the fact must equal one of
    /// the values.
    pub fn matches(&self, fact: Option<&str>) -> bool {
        let Some(fact) = fact else {
            return false;
        };
        if self.values.is_empty() {
            return true;
        }
        let as_version = fact_version(fact);
        self.values.iter().any(|want| {
            if want == fact {
                return true;
            }
            match (
                &as_version,
                looks_like_range(want)
                    .then(|| VersionReq::parse(want).ok())
                    .flatten(),
            ) {
                (Some(v), Some(req)) => req.matches(v),
                _ => false,
            }
        })
    }
}

/// A value is read as a range only when it starts like one, so that a value
/// such as `linux` is never parsed as a version.
fn looks_like_range(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '^' | '~' | '=' | '>' | '<' | '*'))
}

/// Checks every line of a `requires` block against a target. Returns the
/// first line that does not hold, with the fact's value.
pub fn unmet<'a>(
    target: &dyn Target,
    requires: &'a [Requirement],
) -> Option<(&'a Requirement, Option<String>)> {
    requires.iter().find_map(|r| {
        let fact = fact_of(target, &r.fact);
        (!r.matches(fact.as_deref())).then_some((r, fact))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(fact: &str, values: &[&str]) -> Requirement {
        Requirement {
            fact: fact.into(),
            values: values.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn matches_the_three_rules() {
        assert!(req("gpu", &[]).matches(Some("yes")));
        assert!(!req("gpu", &[]).matches(None));
        assert!(req("crow", &["^1"]).matches(Some("1.3.0")));
        assert!(req("crow", &["^1"]).matches(Some("1")));
        assert!(!req("crow", &["^1"]).matches(Some("2.0.0")));
        assert!(req("rookery", &[">=0.7"]).matches(Some("0.8.1")));
        assert!(!req("rookery", &[">=0.7"]).matches(Some("0.6.0")));
        assert!(req("os", &["linux", "macos"]).matches(Some("macos")));
        assert!(!req("os", &["linux", "macos"]).matches(Some("windows")));
        assert!(!req("os", &["linux"]).matches(None));
    }

    #[test]
    fn os_and_arch_are_built_in() {
        let t = StaticTarget::new("rookery")
            .variants(Variants::Matching(Platform::new("linux", "aarch64")));
        assert_eq!(fact_of(&t, "arch").as_deref(), Some("aarch64"));
        let t = t.fact("arch", "x86_64");
        assert_eq!(fact_of(&t, "arch").as_deref(), Some("x86_64"));
        let lines = [req("os", &["linux"]), req("gpu", &[])];
        let (line, found) = unmet(&t, &lines).unwrap();
        assert_eq!(line.fact, "gpu");
        assert_eq!(found, None);
    }
}
