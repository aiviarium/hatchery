//! `hatchery pack`: from a package folder to eggs.

use std::path::Path;

use crate::egg::Egg;
use crate::error::Result;
use crate::manifest::{Defaults, Origin};
use crate::target::Target;
use crate::time::Timestamp;

/// One variant to pack: a target and the os/arch values that pick it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantPick {
    pub target: String,
    pub os: Option<String>,
    pub arch: Option<String>,
}

/// What `pack` writes. With nothing chosen, it writes the base egg and
/// every variant egg.
#[derive(Default)]
pub struct PackOptions<'a> {
    /// The base egg.
    pub base: bool,
    /// These variant eggs.
    pub variants: Vec<VariantPick>,
    /// One fat egg with every variant (or those in `variants`) inside,
    /// instead of separate eggs.
    pub fat: bool,
    /// Skip the build commands and pack what is there.
    pub no_build: bool,
    /// Tools whose `check` runs on their target's files.
    pub checks: Vec<&'a dyn Target>,
    /// Defaults to apply instead of looking for a `packages.kdl` above the
    /// folder.
    pub defaults: Option<Defaults>,
    /// The origin section to write instead of reading it from git.
    pub origin: Option<Origin>,
    /// The build time when neither `origin` nor `SOURCE_DATE_EPOCH` nor the
    /// git commit gives one.
    pub now: Option<Timestamp>,
}

/// Packs a package folder. Runs build commands first unless `no_build`.
/// Returns the eggs in the order: variant eggs, then the base (or fat) egg.
pub fn pack(dir: &Path, opts: &PackOptions<'_>) -> Result<Vec<Egg>> {
    let _ = (dir, opts);
    todo!()
}
