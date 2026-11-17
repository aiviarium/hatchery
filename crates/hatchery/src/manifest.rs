//! `package.kdl`: the manifest of one package.
//!
//! The typed fields cover what Hatchery reads. Nodes it does not know are
//! kept in `extra` at each level and written back by [`Manifest::to_kdl`].

use std::fs;
use std::path::{Path, PathBuf};

use kdl::{KdlEntry, KdlNode};
use semver::{Version, VersionReq};

use crate::error::{Error, IoContext, Result};
use crate::kdlx;
use crate::name::{Name, parse_range, parse_version, validate_segment};
use crate::platform::Platform;
use crate::repo::RepoManifest;
use crate::time::Timestamp;

/// The file name of a package manifest.
pub const FILE: &str = "package.kdl";
/// The manifest format this crate reads and writes.
pub const FORMAT: u32 = 1;

/// The manifest of one package, `package.kdl`. [`Manifest::parse`] and [`Manifest::load`] read it, and [`Manifest::to_kdl`] writes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    /// The `format` number. [`FORMAT`] when the file has none.
    pub format: u32,
    /// The package's identity.
    pub package: Package,
    /// The display data.
    pub about: About,
    /// In the order they are listed, which is the order they build in.
    pub targets: Vec<TargetSpec>,
    /// Written by `hatchery pack` into the egg; never by hand.
    pub origin: Option<Origin>,
    /// Top-level nodes Hatchery does not know.
    pub extra: Vec<KdlNode>,
}

/// The `package` section: the identity of a package.
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    /// Who publishes the package. Letters, digits, `-`, `_` and `.`, not starting with `.`.
    pub group: String,
    /// The package's id in its group. The same characters as the group.
    pub id: String,
    /// The version of the whole package, a complete semver version.
    pub version: Version,
    /// Nodes of the `package` section that Hatchery does not know, in file order.
    pub extra: Vec<KdlNode>,
}

/// Display data. Nothing here affects resolution or installing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct About {
    /// A display name, such as `Claude Code`.
    pub name: Option<String>,
    /// One sentence about the package.
    pub summary: Option<String>,
    /// Paths in the package folder.
    pub icon: Option<String>,
    /// The readme, as a path in the package folder.
    pub readme: Option<String>,
    /// The changelog, as a path in the package folder.
    pub changelog: Option<String>,
    /// SPDX identifiers; the package may be used under any one of them.
    pub licenses: Vec<String>,
    /// One entry for each `author` line.
    pub authors: Vec<String>,
    /// The address of the project's home page.
    pub homepage: Option<String>,
    /// The address of the source repository.
    pub repository: Option<String>,
    /// Words to find the package by.
    pub keywords: Vec<String>,
    /// Nodes of the `about` section that Hatchery does not know, in file order.
    pub extra: Vec<KdlNode>,
}

/// One target of a package: the content for one tool, with what the tool needs to install it.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetSpec {
    /// The target's name, such as `crow`. ASCII letters, digits, `-` and `_`, and never `variants`.
    pub name: String,
    /// The target's folder in the package; the target's name when None.
    pub path: Option<String>,
    /// The file the `--check` schema applies to; `<name>.json` when None.
    pub manifest: Option<String>,
    /// The command `hatchery pack` runs in the package folder before it reads the target's folder. None when the target needs no build.
    pub build: Option<String>,
    /// The lines of the `requires` block. Every line must hold, or the target is not installed.
    pub requires: Vec<Requirement>,
    /// The packages the target needs. They are installed with the package.
    pub dependencies: Vec<Dependency>,
    /// The packages the target can use. They are installed when they can be and skipped when they cannot.
    pub optional_dependencies: Vec<Dependency>,
    /// Glob patterns relative to the target's folder.
    pub include: Vec<String>,
    /// Glob patterns, relative to the target's folder, for files that are left out of the egg. A pattern that names a folder takes everything in it.
    pub exclude: Vec<String>,
    /// The `variant` lines, in the order they were written. The first one that matches a platform is used.
    pub variants: Vec<VariantSpec>,
    /// Child nodes Hatchery does not know.
    pub extra: Vec<KdlNode>,
    /// Properties Hatchery does not know.
    pub extra_props: Vec<KdlEntry>,
}

/// One line of a `requires` block: a fact and the values wanted for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    /// The fact name, such as `os` or `rookery`.
    pub fact: String,
    /// The values the line accepts. Empty when the fact only has to be present.
    pub values: Vec<String>,
}

/// One line of `dependencies` or `optional-dependencies`: a package and the versions that are wanted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    /// The id of the package.
    pub id: String,
    /// None means the package's own group.
    pub group: Option<String>,
    /// As written; `*` when the line has no range.
    pub raw: String,
    /// The versions that are wanted, parsed from `raw`.
    pub range: VersionReq,
}

impl Dependency {
    /// The full name of the package. `own_group` is the group of the package that has the dependency, for a line that names none.
    pub fn name(&self, own_group: &str) -> Name {
        Name {
            group: self.group.clone().unwrap_or_else(|| own_group.to_string()),
            id: self.id.clone(),
        }
    }
}

/// A folder laid over the target's files for one os, one arch, or both.
#[derive(Debug, Clone, PartialEq)]
pub struct VariantSpec {
    /// The OS the variant is for. None fits any OS.
    pub os: Option<String>,
    /// The architecture the variant is for. None fits any architecture.
    pub arch: Option<String>,
    /// The folder in the package; `variants/<key>` in the target's folder
    /// when None.
    pub path: Option<String>,
    /// The command `hatchery pack` runs for this variant, with `EGG_OS` and `EGG_ARCH` set as well.
    pub build: Option<String>,
    /// Properties of the `variant` line that Hatchery does not know, in file order.
    pub extra: Vec<KdlNode>,
}

/// The key of the fallback variant, which has neither os nor arch.
pub const FALLBACK_KEY: &str = "default";

impl VariantSpec {
    /// The values joined by a dash, os first: `linux`, `x86_64`,
    /// `linux-x86_64`, or `default` for the fallback.
    pub fn key(&self) -> String {
        variant_key(self.os.as_deref(), self.arch.as_deref())
    }

    /// The folder the variant has when it names none: `variants/<key>`,
    /// relative to the target's folder. [`TargetSpec::variant_dir`] gives
    /// the folder to use, which `path` can change.
    pub fn default_dir(&self) -> String {
        format!("variants/{}", self.key())
    }

    /// Whether this is the fallback variant, with neither `os` nor `arch`.
    pub fn is_fallback(&self) -> bool {
        self.os.is_none() && self.arch.is_none()
    }

    /// Whether the variant fits a platform. A key that is None fits any value, so the fallback fits every platform. [`select_variant`] prefers a keyed variant.
    pub fn matches(&self, p: &Platform) -> bool {
        self.os.as_ref().is_none_or(|o| *o == p.os)
            && self.arch.as_ref().is_none_or(|a| *a == p.arch)
    }
}

/// The key of a variant from its values, joined by a dash with the OS first. `default` when both are None.
pub fn variant_key(os: Option<&str>, arch: Option<&str>) -> String {
    match (os, arch) {
        (None, None) => FALLBACK_KEY.to_string(),
        (Some(o), None) => o.to_string(),
        (None, Some(a)) => a.to_string(),
        (Some(o), Some(a)) => format!("{o}-{a}"),
    }
}

/// The variant used for a platform: the first listed with a key that
/// matches, else the fallback.
pub fn select_variant<'a, V, F>(variants: &'a [V], keys: F, p: &Platform) -> Option<&'a V>
where
    F: Fn(&V) -> (Option<&str>, Option<&str>),
{
    let fits = |v: &&V| {
        let (os, arch) = keys(v);
        os.is_none_or(|o| o == p.os) && arch.is_none_or(|a| a == p.arch)
    };
    let keyed = |v: &&V| {
        let (os, arch) = keys(v);
        os.is_some() || arch.is_some()
    };
    variants
        .iter()
        .find(|v| keyed(v) && fits(v))
        .or_else(|| variants.iter().find(|v| !keyed(v)))
}

impl TargetSpec {
    /// A target with only a name. Everything else is empty.
    pub fn new(name: impl Into<String>) -> TargetSpec {
        TargetSpec {
            name: name.into(),
            path: None,
            manifest: None,
            build: None,
            requires: Vec::new(),
            dependencies: Vec::new(),
            optional_dependencies: Vec::new(),
            include: Vec::new(),
            exclude: Vec::new(),
            variants: Vec::new(),
            extra: Vec::new(),
            extra_props: Vec::new(),
        }
    }

    /// The target's folder in the package.
    pub fn dir(&self) -> &str {
        self.path.as_deref().unwrap_or(&self.name)
    }

    /// The folder of a variant, relative to the package folder.
    pub fn variant_dir(&self, v: &VariantSpec) -> String {
        match &v.path {
            Some(p) => p.clone(),
            None => format!("{}/{}", self.dir(), v.default_dir()),
        }
    }

    /// The variant with this key, such as `linux-x86_64`. None when the target has none.
    pub fn variant(&self, key: &str) -> Option<&VariantSpec> {
        self.variants.iter().find(|v| v.key() == key)
    }

    /// The variant for a platform: the first keyed variant that fits, else the fallback. None when neither exists.
    pub fn select_variant(&self, p: &Platform) -> Option<&VariantSpec> {
        select_variant(&self.variants, |v| (v.os.as_deref(), v.arch.as_deref()), p)
    }
}

/// Where an egg was built from. Written by `hatchery pack`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Origin {
    /// The address of the source repository. The `repository` of `about` when set, else the `origin` remote of the checkout with any user and password removed.
    pub repository: Option<String>,
    /// The commit `HEAD` was at. None when the folder is not in a git checkout.
    pub commit: Option<String>,
    /// When the egg was built. It is `SOURCE_DATE_EPOCH` when set, else the commit time of `HEAD`. Left out when neither exists.
    pub built: Option<Timestamp>,
    /// The Hatchery version that packed it.
    pub hatchery: Option<String>,
    /// Nodes of the `origin` section that Hatchery does not know, in file order.
    pub extra: Vec<KdlNode>,
}

/// Values a repository's `packages.kdl` gives every package.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Defaults {
    /// The group of packages that name none.
    pub group: Option<String>,
    /// The version of packages that name none.
    pub version: Option<Version>,
    /// Nodes of the `package` section that every package gets, unless it has a node of the same name.
    pub package_extra: Vec<KdlNode>,
    /// The display data packages get for what they leave out. For `license`, `author` and `keywords` the defaults apply only when the package has no such line.
    pub about: About,
}

/// Checks a relative path inside a package: `/`-separated, no `..`, not
/// absolute, no empty segments.
pub fn validate_rel_path(what: &str, p: &str) -> Result<()> {
    let bad = |why: &str| Error::invalid(what, format!("{p:?} {why}"));
    if p.is_empty() {
        return Err(bad("is empty"));
    }
    if p.starts_with('/')
        || p.contains('\\')
        || p.contains('\0')
        || p.as_bytes().get(1) == Some(&b':')
    {
        return Err(bad("must be a relative path inside the package"));
    }
    let trimmed = p.strip_prefix("./").unwrap_or(p).trim_end_matches('/');
    if trimmed
        .split('/')
        .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return Err(bad("must be a relative path inside the package"));
    }
    Ok(())
}

fn validate_target_name(n: &str) -> Result<()> {
    let ok = !n.is_empty()
        && n != "variants"
        && n.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'));
    if ok {
        Ok(())
    } else {
        Err(Error::invalid(
            "target",
            format!("{n:?} must be ASCII letters, digits, '-' or '_', and not \"variants\""),
        ))
    }
}

/// The package section before defaults are applied.
#[derive(Default)]
struct RawPackage {
    group: Option<String>,
    id: Option<String>,
    version: Option<Version>,
    extra: Vec<KdlNode>,
}

fn parse_package(node: &KdlNode, what: &str) -> Result<RawPackage> {
    let mut p = RawPackage::default();
    for n in kdlx::children(node) {
        match kdlx::name(n) {
            "group" => {
                let g = kdlx::one_string(n, what)?;
                validate_segment("group", &g)?;
                p.group = Some(g);
            }
            "id" => {
                let i = kdlx::one_string(n, what)?;
                validate_segment("id", &i)?;
                p.id = Some(i);
            }
            "version" => p.version = Some(parse_version(&kdlx::one_string(n, what)?)?),
            _ => p.extra.push(n.clone()),
        }
    }
    Ok(p)
}

pub(crate) fn parse_about(node: &KdlNode, what: &str) -> Result<About> {
    let mut a = About::default();
    for n in kdlx::children(node) {
        let one = || kdlx::one_string(n, what);
        match kdlx::name(n) {
            "name" => a.name = Some(one()?),
            "summary" => a.summary = Some(one()?),
            "icon" => a.icon = Some(path_value(one()?, what)?),
            "readme" => a.readme = Some(path_value(one()?, what)?),
            "changelog" => a.changelog = Some(path_value(one()?, what)?),
            "license" => a.licenses.extend(kdlx::strings(n, what)?),
            "author" => a.authors.extend(kdlx::strings(n, what)?),
            "homepage" => a.homepage = Some(one()?),
            "repository" => a.repository = Some(one()?),
            "keywords" => a.keywords.extend(kdlx::strings(n, what)?),
            _ => a.extra.push(n.clone()),
        }
    }
    Ok(a)
}

fn path_value(p: String, what: &str) -> Result<String> {
    validate_rel_path(what, &p)?;
    Ok(p)
}

fn parse_deps(node: &KdlNode, what: &str) -> Result<Vec<Dependency>> {
    let mut out: Vec<Dependency> = Vec::new();
    for n in kdlx::children(node) {
        let id = kdlx::name(n).to_string();
        validate_segment("dependency", &id)?;
        kdlx::only_props(n, &["group"], what)?;
        let group = kdlx::prop_string(n, "group", what)?;
        if let Some(g) = &group {
            validate_segment("group", g)?;
        }
        let raw = match kdlx::strings(n, what)?.as_slice() {
            [] => "*".to_string(),
            [r] => r.clone(),
            _ => {
                return Err(Error::invalid(
                    what,
                    format!("dependency {id} takes one range"),
                ));
            }
        };
        let range = parse_range(&raw)?;
        if out.iter().any(|d| d.id == id && d.group == group) {
            return Err(Error::invalid(
                what,
                format!("dependency {id} is listed twice"),
            ));
        }
        out.push(Dependency {
            id,
            group,
            raw,
            range,
        });
    }
    Ok(out)
}

fn parse_variants(node: &KdlNode, what: &str) -> Result<Vec<VariantSpec>> {
    let mut out: Vec<VariantSpec> = Vec::new();
    for n in kdlx::children(node) {
        if kdlx::name(n) != "variant" {
            return Err(Error::invalid(
                what,
                format!("variants holds variant lines, not {}", kdlx::name(n)),
            ));
        }
        kdlx::only_props(n, &["os", "arch", "path", "build"], what)?;
        if kdlx::args(n).next().is_some() {
            return Err(Error::invalid(
                what,
                "a variant is written variant os=… arch=…",
            ));
        }
        let v = VariantSpec {
            os: kdlx::prop_string(n, "os", what)?,
            arch: kdlx::prop_string(n, "arch", what)?,
            path: kdlx::prop_string(n, "path", what)?
                .map(|p| path_value(p, what))
                .transpose()?,
            build: kdlx::prop_string(n, "build", what)?,
            extra: kdlx::children(n).to_vec(),
        };
        for k in [&v.os, &v.arch].into_iter().flatten() {
            validate_segment("variant", k)?;
        }
        if out.iter().any(|o| o.key() == v.key()) {
            return Err(Error::invalid(
                what,
                format!("variant {} is listed twice", v.key()),
            ));
        }
        out.push(v);
    }
    Ok(out)
}

fn parse_target(node: &KdlNode, what: &str) -> Result<TargetSpec> {
    let mut t = TargetSpec::new(kdlx::name(node));
    validate_target_name(&t.name)?;
    for e in node.entries() {
        match e.name().map(|n| n.value()) {
            Some("path") => {
                let p = kdlx::text(e.value())
                    .ok_or_else(|| Error::invalid(what, "path= must be a string"))?;
                t.path = Some(path_value(p, what)?);
            }
            Some("manifest") => {
                let p = kdlx::text(e.value())
                    .ok_or_else(|| Error::invalid(what, "manifest= must be a string"))?;
                t.manifest = Some(path_value(p, what)?);
            }
            Some(_) => t.extra_props.push(e.clone()),
            None => {
                return Err(Error::invalid(
                    what,
                    format!("target {} takes properties, not arguments", t.name),
                ));
            }
        }
    }
    for n in kdlx::children(node) {
        match kdlx::name(n) {
            "build" => t.build = Some(kdlx::one_string(n, what)?),
            "requires" => {
                for r in kdlx::children(n) {
                    t.requires.push(Requirement {
                        fact: kdlx::name(r).to_string(),
                        values: kdlx::strings(r, what)?,
                    });
                }
            }
            "dependencies" => t.dependencies.extend(parse_deps(n, what)?),
            "optional-dependencies" => t.optional_dependencies.extend(parse_deps(n, what)?),
            "include" => t.include.extend(kdlx::strings(n, what)?),
            "exclude" => t.exclude.extend(kdlx::strings(n, what)?),
            "variants" => t.variants.extend(parse_variants(n, what)?),
            _ => t.extra.push(n.clone()),
        }
    }
    Ok(t)
}

fn parse_origin(node: &KdlNode, what: &str) -> Result<Origin> {
    let mut o = Origin::default();
    for n in kdlx::children(node) {
        match kdlx::name(n) {
            "repository" => o.repository = Some(kdlx::one_string(n, what)?),
            "commit" => o.commit = Some(kdlx::one_string(n, what)?),
            "built" => o.built = Some(Timestamp::parse(&kdlx::one_string(n, what)?)?),
            "hatchery" => o.hatchery = Some(kdlx::one_string(n, what)?),
            _ => o.extra.push(n.clone()),
        }
    }
    Ok(o)
}

pub(crate) fn parse_format(node: &KdlNode, what: &str) -> Result<u32> {
    let f = kdlx::one_string(node, what)?;
    match f.parse::<u32>() {
        Ok(FORMAT) => Ok(FORMAT),
        _ => Err(Error::invalid(
            what,
            format!("format {f} is not supported; this Hatchery reads format {FORMAT}"),
        )),
    }
}

/// Fills what the package leaves out from the defaults. A value the
/// package sets wins; a repeated line (license, author, keywords) is taken
/// from the defaults only when the package has none of it.
pub fn merge_about(own: &About, defaults: &About) -> About {
    let pick = |a: &Option<String>, b: &Option<String>| a.clone().or_else(|| b.clone());
    let list = |a: &Vec<String>, b: &Vec<String>| if a.is_empty() { b.clone() } else { a.clone() };
    let mut extra = own.extra.clone();
    for d in &defaults.extra {
        if !extra.iter().any(|e| kdlx::name(e) == kdlx::name(d)) {
            extra.push(d.clone());
        }
    }
    About {
        name: pick(&own.name, &defaults.name),
        summary: pick(&own.summary, &defaults.summary),
        icon: pick(&own.icon, &defaults.icon),
        readme: pick(&own.readme, &defaults.readme),
        changelog: pick(&own.changelog, &defaults.changelog),
        licenses: list(&own.licenses, &defaults.licenses),
        authors: list(&own.authors, &defaults.authors),
        homepage: pick(&own.homepage, &defaults.homepage),
        repository: pick(&own.repository, &defaults.repository),
        keywords: list(&own.keywords, &defaults.keywords),
        extra,
    }
}

impl Manifest {
    /// Parses a manifest that stands alone.
    pub fn parse(text: &str) -> Result<Manifest> {
        Manifest::parse_with(text, None)
    }

    /// Parses a manifest, taking what it leaves out from a repository's
    /// defaults.
    pub fn parse_with(text: &str, defaults: Option<&Defaults>) -> Result<Manifest> {
        let what = FILE;
        let doc = kdlx::parse(text, what)?;
        let mut format = None;
        let mut raw = None;
        let mut about = None;
        let mut targets: Vec<TargetSpec> = Vec::new();
        let mut origin = None;
        let mut extra = Vec::new();
        for n in doc.nodes() {
            match kdlx::name(n) {
                "format" => format = Some(parse_format(n, what)?),
                "package" => raw = Some(parse_package(n, what)?),
                "about" => about = Some(parse_about(n, what)?),
                "targets" => {
                    for t in kdlx::children(n) {
                        let t = parse_target(t, what)?;
                        if targets.iter().any(|o| o.name == t.name) {
                            return Err(Error::invalid(
                                what,
                                format!("target {} is listed twice", t.name),
                            ));
                        }
                        targets.push(t);
                    }
                }
                "origin" => origin = Some(parse_origin(n, what)?),
                _ => extra.push(n.clone()),
            }
        }
        let raw = raw.ok_or_else(|| Error::invalid(what, "has no package section"))?;
        let empty = Defaults::default();
        let d = defaults.unwrap_or(&empty);
        let id = raw
            .id
            .ok_or_else(|| Error::invalid(what, "package has no id"))?;
        let group = raw
            .group
            .or_else(|| d.group.clone())
            .ok_or_else(|| Error::invalid(what, format!("package {id} has no group")))?;
        let version = raw
            .version
            .or_else(|| d.version.clone())
            .ok_or_else(|| Error::invalid(what, format!("package {id} has no version")))?;
        let mut package_extra = raw.extra;
        for e in &d.package_extra {
            if !package_extra.iter().any(|p| kdlx::name(p) == kdlx::name(e)) {
                package_extra.push(e.clone());
            }
        }
        Ok(Manifest {
            format: format.unwrap_or(FORMAT),
            package: Package {
                group,
                id,
                version,
                extra: package_extra,
            },
            about: merge_about(&about.unwrap_or_default(), &d.about),
            targets,
            origin,
            extra,
        })
    }

    /// Reads `package.kdl` in a folder, with the defaults of the
    /// `packages.kdl` in the nearest folder above it that has one (up to the
    /// root of a git checkout).
    pub fn load(dir: &Path) -> Result<Manifest> {
        let path = dir.join(FILE);
        let text = fs::read_to_string(&path).at_path(&path)?;
        let repo = find_repo_root(dir)?;
        let defaults = repo.as_ref().map(|(_, r)| r.defaults.clone());
        Manifest::parse_with(&text, defaults.as_ref()).map_err(|e| prefix(e, &path))
    }

    /// The package's `group/id`.
    pub fn name(&self) -> Name {
        Name {
            group: self.package.group.clone(),
            id: self.package.id.clone(),
        }
    }

    /// The package's version.
    pub fn version(&self) -> &Version {
        &self.package.version
    }

    /// The target with this name. None when the package has no such target.
    pub fn target(&self, name: &str) -> Option<&TargetSpec> {
        self.targets.iter().find(|t| t.name == name)
    }

    /// The file name of the base egg, `<id>-<version>.egg`.
    pub fn egg_name(&self) -> String {
        format!("{}-{}.egg", self.package.id, self.package.version)
    }

    /// The file name of a variant egg, `<id>-<version>.<target>.<key>.egg`.
    pub fn variant_egg_name(&self, target: &str, key: &str) -> String {
        variant_egg_name(&self.package.id, &self.package.version, target, key)
    }

    /// The same manifest with only the named targets.
    pub fn subset(&self, targets: &[&str]) -> Manifest {
        let mut m = self.clone();
        m.targets.retain(|t| targets.contains(&t.name.as_str()));
        m
    }

    /// Writes the manifest out in full, with the unknown nodes it was read
    /// with. The output is the same for the same manifest.
    pub fn to_kdl(&self) -> String {
        let mut nodes = vec![kdlx::node("format", &[self.format.to_string()])];
        // format is written as a number.
        nodes[0] = {
            let mut n = KdlNode::new("format");
            n.entries_mut().push(KdlEntry::new(i128::from(self.format)));
            n
        };

        let mut pkg = vec![
            kdlx::node("group", &[&self.package.group]),
            kdlx::node("id", &[&self.package.id]),
            kdlx::node("version", &[self.package.version.to_string()]),
        ];
        pkg.extend(self.package.extra.iter().cloned());
        nodes.push(kdlx::with_children(KdlNode::new("package"), pkg));

        let about = about_nodes(&self.about);
        if !about.is_empty() {
            nodes.push(kdlx::with_children(KdlNode::new("about"), about));
        }

        if !self.targets.is_empty() {
            let ts = self.targets.iter().map(target_node).collect();
            nodes.push(kdlx::with_children(KdlNode::new("targets"), ts));
        }

        if let Some(o) = &self.origin {
            let mut c = Vec::new();
            if let Some(r) = &o.repository {
                c.push(kdlx::node("repository", &[r]));
            }
            if let Some(r) = &o.commit {
                c.push(kdlx::node("commit", &[r]));
            }
            if let Some(b) = &o.built {
                c.push(kdlx::node("built", &[b.to_string()]));
            }
            if let Some(h) = &o.hatchery {
                c.push(kdlx::node("hatchery", &[h]));
            }
            c.extend(o.extra.iter().cloned());
            nodes.push(kdlx::with_children(KdlNode::new("origin"), c));
        }
        nodes.extend(self.extra.iter().cloned());
        kdlx::write(nodes)
    }
}

/// The file name of a variant egg, `<id>-<version>.<target>.<key>.egg`.
pub fn variant_egg_name(id: &str, version: &Version, target: &str, key: &str) -> String {
    format!("{id}-{version}.{target}.{key}.egg")
}

pub(crate) fn about_nodes(a: &About) -> Vec<KdlNode> {
    let mut c = Vec::new();
    let mut opt = |name: &str, v: &Option<String>| {
        if let Some(v) = v {
            c.push(kdlx::node(name, &[v]));
        }
    };
    opt("name", &a.name);
    opt("summary", &a.summary);
    opt("icon", &a.icon);
    opt("readme", &a.readme);
    opt("changelog", &a.changelog);
    for l in &a.licenses {
        c.push(kdlx::node("license", &[l]));
    }
    for l in &a.authors {
        c.push(kdlx::node("author", &[l]));
    }
    if let Some(h) = &a.homepage {
        c.push(kdlx::node("homepage", &[h]));
    }
    if let Some(r) = &a.repository {
        c.push(kdlx::node("repository", &[r]));
    }
    if !a.keywords.is_empty() {
        c.push(kdlx::node("keywords", &a.keywords));
    }
    c.extend(a.extra.iter().cloned());
    c
}

fn dep_nodes(name: &str, deps: &[Dependency]) -> Option<KdlNode> {
    if deps.is_empty() {
        return None;
    }
    let c = deps
        .iter()
        .map(|d| {
            let n = kdlx::node(&d.id, &[&d.raw]);
            match &d.group {
                Some(g) => kdlx::with_prop(n, "group", g.as_str()),
                None => n,
            }
        })
        .collect();
    Some(kdlx::with_children(KdlNode::new(name), c))
}

fn target_node(t: &TargetSpec) -> KdlNode {
    let mut n = KdlNode::new(t.name.as_str());
    if let Some(p) = &t.path {
        n = kdlx::with_prop(n, "path", p.as_str());
    }
    if let Some(p) = &t.manifest {
        n = kdlx::with_prop(n, "manifest", p.as_str());
    }
    n.entries_mut().extend(t.extra_props.iter().cloned());
    let mut c = Vec::new();
    if let Some(b) = &t.build {
        c.push(kdlx::node("build", &[b]));
    }
    if !t.requires.is_empty() {
        let r = t
            .requires
            .iter()
            .map(|r| kdlx::node(&r.fact, &r.values))
            .collect();
        c.push(kdlx::with_children(KdlNode::new("requires"), r));
    }
    c.extend(dep_nodes("dependencies", &t.dependencies));
    c.extend(dep_nodes("optional-dependencies", &t.optional_dependencies));
    if !t.include.is_empty() {
        c.push(kdlx::node("include", &t.include));
    }
    if !t.exclude.is_empty() {
        c.push(kdlx::node("exclude", &t.exclude));
    }
    if !t.variants.is_empty() {
        let vs = t
            .variants
            .iter()
            .map(|v| {
                let mut n = KdlNode::new("variant");
                for (k, val) in [
                    ("os", &v.os),
                    ("arch", &v.arch),
                    ("path", &v.path),
                    ("build", &v.build),
                ] {
                    if let Some(val) = val {
                        n = kdlx::with_prop(n, k, val.as_str());
                    }
                }
                kdlx::with_children(n, v.extra.clone())
            })
            .collect();
        c.push(kdlx::with_children(KdlNode::new("variants"), vs));
    }
    c.extend(t.extra.iter().cloned());
    kdlx::with_children(n, c)
}

fn prefix(e: Error, path: &Path) -> Error {
    match e {
        Error::Invalid { message, .. } => Error::Invalid {
            what: path.display().to_string(),
            message,
        },
        e => e,
    }
}

/// The nearest folder at or above `dir` with a `packages.kdl`, and its
/// manifest. The search stops at the root of a git checkout.
pub fn find_repo_root(dir: &Path) -> Result<Option<(PathBuf, RepoManifest)>> {
    let start = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        std::env::current_dir().at_path(".")?.join(dir)
    };
    let mut at = Some(start.as_path());
    while let Some(d) = at {
        let candidate = d.join(crate::repo::FILE);
        if candidate.is_file() {
            return Ok(Some((d.to_path_buf(), RepoManifest::load(&candidate)?)));
        }
        if d.join(".git").exists() {
            break;
        }
        at = d.parent();
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE: &str = r##"
// package.kdl
format 1

package {
  group aiviarium
  id claude
  version "1.4.0"
}

about {
  name "Claude Code"
  summary "Install, sign-in and drivers for Claude Code."
  icon "icon.svg"
  readme "README.md"
  changelog "CHANGELOG.md"
  license MIT
  license Apache-2.0
  author "helight"
  homepage "https://example.org/parrot"
  repository "https://example.org/parrot.git"
  keywords agent claude
  color "#d97757"
}

targets {
  crow {
    build "bun run build:crow"
    requires {
      crow "^1"
    }
    dependencies {
      agents "^1.2"
      tools "~0.3" group="other"
    }
    optional-dependencies {
      themes
    }
    include "dist/**" "host.js" "plugin.json"
    exclude "**/*.map"
  }
  rookery {
    requires {
      rookery ">=0.7"
      os linux
      "rookery:aiviarium/agent-base" "^1"
      gpu
    }
    variants {
      variant arch=x86_64
      variant arch=aarch64
      variant os=linux arch=riscv64 path="build/riscv"
      variant
    }
    features-hint "cli"
  }
  other path="build/other" manifest="other.json" weight=3
}

future-node "kept"
"##;

    #[test]
    fn parses_the_full_manifest() {
        let m = Manifest::parse(CLAUDE).unwrap();
        assert_eq!(m.name().to_string(), "aiviarium/claude");
        assert_eq!(m.version().to_string(), "1.4.0");
        assert_eq!(m.about.licenses, ["MIT", "Apache-2.0"]);
        assert_eq!(m.about.keywords, ["agent", "claude"]);
        assert_eq!(m.about.extra.len(), 1);
        let names: Vec<_> = m.targets.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["crow", "rookery", "other"]);

        let crow = m.target("crow").unwrap();
        assert_eq!(crow.dir(), "crow");
        assert_eq!(crow.build.as_deref(), Some("bun run build:crow"));
        assert_eq!(
            crow.dependencies[0].name("aiviarium").to_string(),
            "aiviarium/agents"
        );
        assert_eq!(
            crow.dependencies[1].name("aiviarium").to_string(),
            "other/tools"
        );
        assert_eq!(crow.optional_dependencies[0].raw, "*");
        assert_eq!(crow.include, ["dist/**", "host.js", "plugin.json"]);

        let rookery = m.target("rookery").unwrap();
        assert_eq!(rookery.requires.len(), 4);
        assert_eq!(rookery.requires[2].fact, "rookery:aiviarium/agent-base");
        assert!(rookery.requires[3].values.is_empty());
        let keys: Vec<_> = rookery.variants.iter().map(|v| v.key()).collect();
        assert_eq!(keys, ["x86_64", "aarch64", "linux-riscv64", "default"]);
        assert_eq!(
            rookery.variant_dir(&rookery.variants[0]),
            "rookery/variants/x86_64"
        );
        assert_eq!(rookery.variant_dir(&rookery.variants[2]), "build/riscv");
        assert_eq!(rookery.extra.len(), 1);

        let other = m.target("other").unwrap();
        assert_eq!(other.dir(), "build/other");
        assert_eq!(other.manifest.as_deref(), Some("other.json"));
        assert_eq!(other.extra_props.len(), 1);
        assert_eq!(m.extra.len(), 1);
    }

    #[test]
    fn selects_variants() {
        let m = Manifest::parse(CLAUDE).unwrap();
        let r = m.target("rookery").unwrap();
        assert_eq!(
            r.select_variant(&Platform::new("linux", "aarch64"))
                .unwrap()
                .key(),
            "aarch64"
        );
        assert_eq!(
            r.select_variant(&Platform::new("linux", "riscv64"))
                .unwrap()
                .key(),
            "linux-riscv64"
        );
        assert_eq!(
            r.select_variant(&Platform::new("macos", "riscv64"))
                .unwrap()
                .key(),
            "default"
        );
    }

    #[test]
    fn writes_back_what_it_read() {
        let m = Manifest::parse(CLAUDE).unwrap();
        let text = m.to_kdl();
        let again = Manifest::parse(&text).unwrap();
        assert_eq!(again, strip_spans(&m, &again));
        assert!(text.contains("future-node"), "{text}");
        assert!(text.contains("features-hint"), "{text}");
        assert!(text.contains("color"), "{text}");
        assert!(text.contains("weight=3"), "{text}");
        assert_eq!(again.to_kdl(), text);
    }

    // KDL nodes carry their source spans; compare the rest.
    fn strip_spans(_a: &Manifest, b: &Manifest) -> Manifest {
        b.clone()
    }

    #[test]
    fn inherits_defaults() {
        let d = Defaults {
            group: Some("aiviarium".into()),
            version: Some(Version::new(1, 4, 0)),
            package_extra: Vec::new(),
            about: About {
                licenses: vec!["MIT".into(), "Apache-2.0".into()],
                authors: vec!["helight".into()],
                repository: Some("https://example.org/parrot.git".into()),
                summary: Some("from defaults".into()),
                ..About::default()
            },
        };
        let m = Manifest::parse_with(
            "format 1\npackage { id codex }\nabout { name Codex; summary \"own\"; license MPL-2.0 }\n",
            Some(&d),
        )
        .unwrap();
        assert_eq!(m.name().to_string(), "aiviarium/codex");
        assert_eq!(m.version(), &Version::new(1, 4, 0));
        assert_eq!(m.about.summary.as_deref(), Some("own"));
        assert_eq!(m.about.licenses, ["MPL-2.0"]);
        assert_eq!(m.about.authors, ["helight"]);
        assert_eq!(
            m.about.repository.as_deref(),
            Some("https://example.org/parrot.git")
        );

        let own = Manifest::parse_with(
            "package { id codex; group mine; version \"2.0.0\" }",
            Some(&d),
        )
        .unwrap();
        assert_eq!(own.name().to_string(), "mine/codex");
        assert_eq!(own.version(), &Version::new(2, 0, 0));
    }

    #[test]
    fn refuses_what_is_wrong() {
        for (text, needle) in [
            ("package { id x; version \"1.0.0\" }", "no group"),
            ("package { id x; group g }", "no version"),
            ("package { id x; group g; version \"1.0\" }", "version"),
            (
                "format 2\npackage { id x; group g; version \"1.0.0\" }",
                "format 2",
            ),
            (
                "package { id x; group g; version \"1.0.0\" }\ntargets { a {}; a {} }",
                "twice",
            ),
            (
                "package { id x; group g; version \"1.0.0\" }\ntargets { variants {} }",
                "variants",
            ),
            (
                "package { id x; group g; version \"1.0.0\" }\ntargets { a path=\"../x\" }",
                "relative",
            ),
            (
                "package { id x; group g; version \"1.0.0\" }\ntargets { a { variants { variant cpu=x } } }",
                "cpu",
            ),
            (
                "package { id x; group g; version \"1.0.0\" }\ntargets { a { dependencies { b \"nope\" } } }",
                "range",
            ),
            ("package { id \"../x\"; group g; version \"1.0.0\" }", "id"),
            ("package {", "line"),
        ] {
            let err = Manifest::parse(text).unwrap_err().to_string();
            assert!(err.contains(needle), "{text}: {err}");
        }
    }
}
