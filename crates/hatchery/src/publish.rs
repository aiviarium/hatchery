//! `hatchery publish`: an egg into a static registry folder or the eggs
//! branch of a git repository.

use std::path::{Path, PathBuf};

use semver::Version;

use crate::egg::Egg;
use crate::error::Result;
use crate::name::Name;
use crate::sign::SigningKey;
use crate::time::Timestamp;

/// Where to publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishTo {
    /// A static registry folder.
    Dir(PathBuf),
    /// A branch of a git repository, in the static registry layout, under
    /// `path` when given. The branch is created when it does not exist.
    Git {
        url: String,
        branch: String,
        path: Option<String>,
    },
}

/// What was published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub name: Name,
    pub version: Version,
    /// The egg's file name in the registry.
    pub file: String,
    pub sha256: String,
    pub signed: bool,
    pub published: Timestamp,
}

/// Publishes an egg (base or variant) with the current time.
pub fn publish(egg: &Egg, to: &PublishTo, key: Option<&SigningKey>) -> Result<Published> {
    publish_at(egg, to, key, Timestamp::now())
}

/// Publishes an egg with a given publish time.
pub fn publish_at(
    egg: &Egg,
    to: &PublishTo,
    key: Option<&SigningKey>,
    now: Timestamp,
) -> Result<Published> {
    let _ = (egg, to, key, now);
    todo!()
}

/// Publishes into a static registry folder.
pub fn publish_dir(
    egg: &Egg,
    dir: &Path,
    key: Option<&SigningKey>,
    now: Timestamp,
) -> Result<Published> {
    let _ = (egg, dir, key, now);
    todo!()
}

/// Marks a published version as withdrawn in a static registry folder.
pub fn withdraw_dir(
    dir: &Path,
    name: &Name,
    version: &Version,
    reason: Option<&str>,
) -> Result<()> {
    let _ = (dir, name, version, reason);
    todo!()
}
