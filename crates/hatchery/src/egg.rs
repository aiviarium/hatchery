//! Eggs: built packages. A base egg holds the manifest, the files `about`
//! points at and each target's shared files; a variant egg holds one
//! variant's files; a fat egg is a base egg with variant eggs inside.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use bytes::Bytes;
use semver::Version;

use crate::archive;
use crate::digest::sha256_hex;
use crate::error::{Error, IoContext, Result};
use crate::kdlx;
use crate::manifest::{self, Manifest, Package, TargetSpec, VariantSpec};
use crate::name::{Name, parse_version, validate_segment};
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

    /// The folder of the variant's files in its egg:
    /// `<target>/variants/<key>`.
    pub fn dir(&self) -> String {
        format!("{}/{VARIANTS_DIR}/{}", self.target, self.key())
    }

    /// The text of `variant.kdl`.
    pub fn to_kdl(&self) -> String {
        let mut c = vec![
            kdlx::node("group", &[&self.name.group]),
            kdlx::node("id", &[&self.name.id]),
            kdlx::node("version", &[self.version.to_string()]),
            kdlx::node("target", &[&self.target]),
        ];
        if let Some(os) = &self.os {
            c.push(kdlx::node("os", &[os]));
        }
        if let Some(arch) = &self.arch {
            c.push(kdlx::node("arch", &[arch]));
        }
        let mut format = kdl::KdlNode::new("format");
        format
            .entries_mut()
            .push(kdl::KdlEntry::new(i128::from(manifest::FORMAT)));
        kdlx::write(vec![
            format,
            kdlx::with_children(kdl::KdlNode::new("variant"), c),
        ])
    }

    /// Reads the text of `variant.kdl`.
    pub fn parse(text: &str) -> Result<VariantHeader> {
        let what = VARIANT_FILE;
        let doc = kdlx::parse(text, what)?;
        let mut header = None;
        for n in doc.nodes() {
            match kdlx::name(n) {
                "format" => {
                    manifest::parse_format(n, what)?;
                }
                "variant" => {
                    let (mut group, mut id, mut version, mut target) = (None, None, None, None);
                    let (mut os, mut arch) = (None, None);
                    for c in kdlx::children(n) {
                        let v = kdlx::one_string(c, what)?;
                        match kdlx::name(c) {
                            "group" => group = Some(v),
                            "id" => id = Some(v),
                            "version" => version = Some(parse_version(&v)?),
                            "target" => target = Some(v),
                            "os" => os = Some(v),
                            "arch" => arch = Some(v),
                            _ => {}
                        }
                    }
                    let missing = |f: &str| Error::invalid(what, format!("variant has no {f}"));
                    let target = target.ok_or_else(|| missing("target"))?;
                    validate_segment("target", &target)?;
                    for k in [&os, &arch].into_iter().flatten() {
                        validate_segment("variant", k)?;
                    }
                    header = Some(VariantHeader {
                        name: Name::new(
                            group.ok_or_else(|| missing("group"))?,
                            id.ok_or_else(|| missing("id"))?,
                        )?,
                        version: version.ok_or_else(|| missing("version"))?,
                        target,
                        os,
                        arch,
                    });
                }
                _ => {}
            }
        }
        header.ok_or_else(|| Error::invalid(what, "has no variant section"))
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
    bytes: Bytes,
    sha256: String,
    entries: Vec<Entry>,
    manifest: Manifest,
    kind: Kind,
}

#[derive(Debug, Clone)]
enum Kind {
    Base,
    Fat(Vec<Egg>),
    Variant(VariantHeader),
}

/// The most a whole egg may hold unpacked.
const MAX_UNPACKED: u64 = archive::MAX_TOTAL_BYTES;

fn manifest_of_variant(h: &VariantHeader) -> Manifest {
    let mut t = TargetSpec::new(h.target.clone());
    t.variants.push(VariantSpec {
        os: h.os.clone(),
        arch: h.arch.clone(),
        path: None,
        build: None,
        extra: Vec::new(),
    });
    Manifest {
        format: manifest::FORMAT,
        package: Package {
            group: h.name.group.clone(),
            id: h.name.id.clone(),
            version: h.version.clone(),
            extra: Vec::new(),
        },
        about: Default::default(),
        targets: vec![t],
        origin: None,
        extra: Vec::new(),
    }
}

fn is_variant_entry(path: &str) -> bool {
    path.split('/').next() == Some(VARIANTS_DIR)
}

impl Egg {
    /// Packs a folder into a base egg without running any build, checking
    /// each target with the given tools. See [`crate::pack`] for the rest.
    pub fn pack(dir: &Path, targets: &[&dyn Target]) -> Result<Egg> {
        let opts = crate::pack::PackOptions {
            base: true,
            no_build: true,
            checks: targets.to_vec(),
            ..Default::default()
        };
        crate::pack::pack(dir, &opts)?
            .pop()
            .ok_or_else(|| Error::other("pack produced no egg"))
    }

    /// Opens an egg of any kind, refusing unsafe entries.
    pub fn open(bytes: impl Into<Bytes>) -> Result<Egg> {
        let bytes: Bytes = bytes.into();
        let entries = archive::read(&bytes)?;
        let total: u64 = entries.iter().map(|e| e.data.len() as u64).sum();
        if total > MAX_UNPACKED {
            return Err(Error::UnsafeArchive(format!(
                "egg holds {total} bytes unpacked, more than {MAX_UNPACKED}"
            )));
        }
        let find = |name: &str| entries.iter().find(|e| e.path == name);
        let text = |e: &Entry, what: &str| {
            std::str::from_utf8(&e.data)
                .map(str::to_string)
                .map_err(|_| Error::invalid(what, "is not UTF-8 text"))
        };

        let (manifest, kind) = if let Some(v) = find(VARIANT_FILE) {
            if find(manifest::FILE).is_some() {
                return Err(Error::invalid(
                    "egg",
                    format!("has both {} and {VARIANT_FILE}", manifest::FILE),
                ));
            }
            let header = VariantHeader::parse(&text(v, VARIANT_FILE)?)?;
            let prefix = format!("{}/", header.dir());
            if let Some(e) = entries
                .iter()
                .find(|e| e.path != VARIANT_FILE && !e.path.starts_with(&prefix))
            {
                return Err(Error::invalid(
                    "variant egg",
                    format!("{:?} is outside {prefix}", e.path),
                ));
            }
            (manifest_of_variant(&header), Kind::Variant(header))
        } else {
            let m = find(manifest::FILE)
                .ok_or_else(|| Error::invalid("egg", format!("has no {}", manifest::FILE)))?;
            let manifest = Manifest::parse(&text(m, manifest::FILE)?)?;
            let mut embedded = Vec::new();
            for e in entries.iter().filter(|e| is_variant_entry(&e.path)) {
                let file = e
                    .path
                    .strip_prefix("variants/")
                    .filter(|f| !f.contains('/') && f.ends_with(".egg"))
                    .ok_or_else(|| {
                        Error::invalid("egg", format!("{:?} is not a variant egg", e.path))
                    })?;
                let inner = Egg::open(e.data.clone())?;
                let ok = match inner.variant() {
                    Some(h) => {
                        h.name == manifest.name()
                            && h.version == *manifest.version()
                            && h.file_name() == file
                    }
                    None => false,
                };
                if !ok {
                    return Err(Error::invalid(
                        "egg",
                        format!(
                            "{:?} is not a variant egg of {} {}",
                            e.path,
                            manifest.name(),
                            manifest.version()
                        ),
                    ));
                }
                embedded.push(inner);
            }
            let kind = if embedded.is_empty() {
                Kind::Base
            } else {
                Kind::Fat(embedded)
            };
            (manifest, kind)
        };
        Ok(Egg {
            sha256: sha256_hex(&bytes),
            bytes,
            entries,
            manifest,
            kind,
        })
    }

    /// Reads and opens an egg file.
    pub fn read(path: &Path) -> Result<Egg> {
        let bytes = fs::read(path).at_path(path)?;
        Egg::open(bytes)
    }

    /// Builds an egg from entries, written deterministically.
    pub fn from_entries(entries: Vec<Entry>) -> Result<Egg> {
        Egg::open(archive::write(&entries)?)
    }

    /// The package, about, targets and origin. For a variant egg, a manifest
    /// with the package, version and the one target and variant.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The header, for a variant egg.
    pub fn variant(&self) -> Option<&VariantHeader> {
        match &self.kind {
            Kind::Variant(h) => Some(h),
            _ => None,
        }
    }

    /// Whether this is a fat egg, with variant eggs inside.
    pub fn is_fat(&self) -> bool {
        matches!(self.kind, Kind::Fat(_))
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The shared files of one target, None if the egg has no such target.
    pub fn target(&self, name: &str) -> Option<TargetFiles> {
        self.manifest.target(name)?;
        let prefix = format!("{name}/");
        let variants = format!("{name}/{VARIANTS_DIR}/");
        let files: BTreeMap<String, Bytes> = self
            .entries
            .iter()
            .filter(|e| e.path.starts_with(&prefix) && !e.path.starts_with(&variants))
            .map(|e| (e.path[prefix.len()..].to_string(), e.data.clone()))
            .collect();
        Some(TargetFiles::new(name, files))
    }

    /// The same egg with only the named targets: their files, their
    /// variant eggs, and a manifest that lists only them. None when it has
    /// none of them.
    pub fn subset(&self, targets: &[&str]) -> Result<Option<Egg>> {
        let kept: Vec<&str> = self
            .manifest
            .targets
            .iter()
            .map(|t| t.name.as_str())
            .filter(|n| targets.contains(n))
            .collect();
        if kept.is_empty() {
            return Ok(None);
        }
        if self.variant().is_some() {
            return Ok(Some(self.clone()));
        }
        let manifest = self.manifest.subset(&kept);
        let about: Vec<&str> = [
            &manifest.about.icon,
            &manifest.about.readme,
            &manifest.about.changelog,
        ]
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect();
        let mut entries = vec![Entry {
            path: manifest::FILE.to_string(),
            data: Bytes::from(manifest.to_kdl()),
            executable: false,
        }];
        for e in &self.entries {
            let first = e.path.split('/').next().unwrap_or("");
            let keep = if e.path == manifest::FILE {
                false
            } else if about.contains(&e.path.as_str()) {
                true
            } else if first == VARIANTS_DIR {
                match &self.kind {
                    Kind::Fat(inner) => inner.iter().any(|v| {
                        v.file_name() == e.path["variants/".len()..]
                            && v.variant()
                                .is_some_and(|h| kept.contains(&h.target.as_str()))
                    }),
                    _ => false,
                }
            } else {
                kept.contains(&first)
            };
            if keep {
                entries.push(e.clone());
            }
        }
        Egg::from_entries(entries).map(Some)
    }

    /// Lowercase hex SHA-256 of the bytes.
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub fn bytes(&self) -> &Bytes {
        &self.bytes
    }

    /// `<id>-<version>.egg` for a base or fat egg,
    /// `<id>-<version>.<target>.<key>.egg` for a variant egg.
    pub fn file_name(&self) -> String {
        match &self.kind {
            Kind::Variant(h) => h.file_name(),
            _ => self.manifest.egg_name(),
        }
    }

    /// The `(target, key)` of each variant egg inside a fat egg.
    pub fn embedded_variants(&self) -> Vec<(String, String)> {
        match &self.kind {
            Kind::Fat(inner) => inner
                .iter()
                .filter_map(|v| v.variant())
                .map(|h| (h.target.clone(), h.key()))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// A variant egg from inside a fat egg.
    pub fn embedded_variant(&self, target: &str, key: &str) -> Result<Option<Egg>> {
        Ok(match &self.kind {
            Kind::Fat(inner) => inner
                .iter()
                .find(|v| {
                    v.variant()
                        .is_some_and(|h| h.target == target && h.key() == key)
                })
                .cloned(),
            _ => None,
        })
    }

    /// The base egg: a fat egg without its variant eggs, byte for byte the
    /// base egg it was made from. A base egg returns itself.
    pub fn base(&self) -> Result<Egg> {
        match &self.kind {
            Kind::Fat(_) => Egg::from_entries(
                self.entries
                    .iter()
                    .filter(|e| !is_variant_entry(&e.path))
                    .cloned()
                    .collect(),
            ),
            Kind::Base => Ok(self.clone()),
            Kind::Variant(_) => Err(Error::invalid("egg", "a variant egg has no base egg")),
        }
    }

    /// A fat egg: this base egg with the given variant eggs inside.
    pub fn fatten(&self, variants: &[Egg]) -> Result<Egg> {
        let base = self.base()?;
        let mut entries = base.entries;
        for v in variants {
            let ok = v.variant().is_some_and(|h| {
                h.name == base.manifest.name() && h.version == *base.manifest.version()
            });
            if !ok {
                return Err(Error::invalid(
                    "egg",
                    format!(
                        "{} is not a variant egg of {} {}",
                        v.file_name(),
                        base.manifest.name(),
                        base.manifest.version()
                    ),
                ));
            }
            entries.push(Entry {
                path: format!("{VARIANTS_DIR}/{}", v.file_name()),
                data: v.bytes.clone(),
                executable: false,
            });
        }
        Egg::from_entries(entries)
    }

    /// Unpacks into a package folder.
    ///
    /// A base or fat egg replaces `dest` as a whole, atomically; the
    /// variant eggs of a fat egg are unpacked into it too. A variant egg
    /// replaces `dest/<target>/variants/<key>`.
    pub fn unpack(&self, dest: &Path) -> Result<()> {
        match &self.kind {
            Kind::Variant(h) => {
                let target = dest.join(&h.target).join(VARIANTS_DIR).join(h.key());
                let prefix = format!("{}/", h.dir());
                stage_and_replace(&target, |staging| {
                    for e in &self.entries {
                        if let Some(rel) = e.path.strip_prefix(&prefix) {
                            write_entry(staging, rel, e)?;
                        }
                    }
                    Ok(())
                })
            }
            _ => stage_and_replace(dest, |staging| {
                for e in self.entries.iter().filter(|e| !is_variant_entry(&e.path)) {
                    write_entry(staging, &e.path, e)?;
                }
                if let Kind::Fat(inner) = &self.kind {
                    for v in inner {
                        for e in v.entries.iter().filter(|e| e.path != VARIANT_FILE) {
                            write_entry(staging, &e.path, e)?;
                        }
                    }
                }
                Ok(())
            }),
        }
    }
}

/// Fills a fresh folder beside `dest`, then swaps it in.
fn stage_and_replace(dest: &Path, fill: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    let staging = crate::fsx::staging_path(dest);
    fs::create_dir_all(&staging).at_path(&staging)?;
    let result = fill(&staging).and_then(|()| crate::fsx::replace_dir(&staging, dest));
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

/// Writes one entry below `root`; its path was checked when the egg was
/// opened.
fn write_entry(root: &Path, rel: &str, e: &Entry) -> Result<()> {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).at_path(parent)?;
    }
    fs::write(&path, &e.data).at_path(&path)?;
    set_mode(&path, e.executable)
}

#[cfg(unix)]
fn set_mode(path: &Path, executable: bool) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if executable { 0o755 } else { 0o644 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).at_path(path)
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _executable: bool) -> Result<()> {
    Ok(())
}
