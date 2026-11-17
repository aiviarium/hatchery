//! `eggs.lock`: what is installed in a folder and from where.

use std::collections::BTreeMap;
use std::path::Path;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::name::Name;

pub const FILE: &str = "eggs.lock";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lock {
    #[serde(default, rename = "package")]
    pub packages: Vec<LockedPackage>,
}

/// One installed package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedPackage {
    pub name: Name,
    pub version: Version,
    /// The targets installed from it.
    pub targets: Vec<String>,
    /// A static registry or a git registry, by its address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    /// A git repository the package came from (a git registry or one folder).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// An egg file or URL it was installed from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// A package folder it was installed from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// The base egg's checksum.
    pub sha256: String,
    /// The variant eggs fetched so far, `"<target>.<key>" = sha256`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variants: BTreeMap<String, String>,
}

impl Lock {
    /// Reads `eggs.lock` in a folder; an empty lock when there is none.
    pub fn load(dir: &Path) -> Result<Lock> {
        let _ = dir;
        todo!()
    }

    /// Writes `eggs.lock` in a folder, sorted by name.
    pub fn save(&self, dir: &Path) -> Result<()> {
        let _ = dir;
        todo!()
    }

    pub fn get(&self, name: &Name) -> Option<&LockedPackage> {
        self.packages.iter().find(|p| &p.name == name)
    }
}
