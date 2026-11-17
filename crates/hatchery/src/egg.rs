//! Eggs: built packages. A base egg holds the manifest, the files `about`
//! points at and each target's shared files; a variant egg holds one
//! variant's files; a fat egg is a base egg with variant eggs inside.

use std::path::Path;

use bytes::Bytes;
use semver::Version;

use crate::error::Result;
use crate::manifest::Manifest;
use crate::name::Name;
use crate::target::{Target, TargetFiles};

/// The header file of a variant egg.
pub const VARIANT_FILE: &str = "variant.kdl";
/// The folder of a fat egg that holds its variant eggs.
pub const VARIANTS_DIR: &str = "variants";

/// What a variant egg says it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantHeader {
    pub name: Name,
    pub version: Version,
    pub target: String,
    pub os: Option<String>,
    pub arch: Option<String>,
}

impl VariantHeader {
    pub fn key(&self) -> String {
        crate::manifest::variant_key(self.os.as_deref(), self.arch.as_deref())
    }

    /// `<id>-<version>.<target>.<key>.egg`.
    pub fn file_name(&self) -> String {
        crate::manifest::variant_egg_name(&self.name.id, &self.version, &self.target, &self.key())
    }
}

/// One file in an egg.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// `/`-separated, relative.
    pub path: String,
    pub data: Bytes,
    pub executable: bool,
}

/// An opened egg. Every entry was checked when it was opened.
#[derive(Debug, Clone)]
pub struct Egg {
    _private: (),
}

impl Egg {
    /// Packs a folder into a base egg without running any build, checking
    /// each target with the given tools. See [`crate::pack`] for the rest.
    pub fn pack(dir: &Path, targets: &[&dyn Target]) -> Result<Egg> {
        let _ = (dir, targets);
        todo!()
    }

    /// Opens an egg of any kind, refusing unsafe entries.
    pub fn open(bytes: impl Into<Bytes>) -> Result<Egg> {
        let _ = bytes.into();
        todo!()
    }

    /// Reads and opens an egg file.
    pub fn read(path: &Path) -> Result<Egg> {
        let _ = path;
        todo!()
    }

    /// Builds an egg from entries, written deterministically.
    pub fn from_entries(entries: Vec<Entry>) -> Result<Egg> {
        let _ = entries;
        todo!()
    }

    /// The package, about, targets and origin. For a variant egg, a manifest
    /// with the package, version and the one target and variant.
    pub fn manifest(&self) -> &Manifest {
        todo!()
    }

    /// The header, for a variant egg.
    pub fn variant(&self) -> Option<&VariantHeader> {
        todo!()
    }

    /// Whether this is a fat egg, with variant eggs inside.
    pub fn is_fat(&self) -> bool {
        todo!()
    }

    pub fn entries(&self) -> &[Entry] {
        todo!()
    }

    /// The shared files of one target, None if the egg has no such target.
    pub fn target(&self, name: &str) -> Option<TargetFiles> {
        let _ = name;
        todo!()
    }

    /// The same egg with only the named targets: their files, their
    /// variant eggs, and a manifest that lists only them. None when it has
    /// none of them.
    pub fn subset(&self, targets: &[&str]) -> Result<Option<Egg>> {
        let _ = targets;
        todo!()
    }

    /// Lowercase hex SHA-256 of the bytes.
    pub fn sha256(&self) -> &str {
        todo!()
    }

    pub fn bytes(&self) -> &Bytes {
        todo!()
    }

    /// `<id>-<version>.egg` for a base or fat egg,
    /// `<id>-<version>.<target>.<key>.egg` for a variant egg.
    pub fn file_name(&self) -> String {
        todo!()
    }

    /// The `(target, key)` of each variant egg inside a fat egg.
    pub fn embedded_variants(&self) -> Vec<(String, String)> {
        todo!()
    }

    /// A variant egg from inside a fat egg.
    pub fn embedded_variant(&self, target: &str, key: &str) -> Result<Option<Egg>> {
        let _ = (target, key);
        todo!()
    }

    /// The base egg: a fat egg without its variant eggs, byte for byte the
    /// base egg it was made from. A base egg returns itself.
    pub fn base(&self) -> Result<Egg> {
        todo!()
    }

    /// A fat egg: this base egg with the given variant eggs inside.
    pub fn fatten(&self, variants: &[Egg]) -> Result<Egg> {
        let _ = variants;
        todo!()
    }

    /// Unpacks into a package folder.
    ///
    /// A base or fat egg replaces `dest` as a whole, atomically; the
    /// variant eggs of a fat egg are unpacked into it too. A variant egg
    /// replaces `dest/<target>/variants/<key>`.
    pub fn unpack(&self, dest: &Path) -> Result<()> {
        let _ = dest;
        todo!()
    }
}
