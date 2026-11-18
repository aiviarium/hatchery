//! `packages.kdl`: the root manifest of a repository that holds packages.

use std::fs;
use std::path::Path;

use kdl::KdlNode;
use semver::Version;

use crate::error::{Error, IoContext, Result};
use crate::glob::Glob;
use crate::kdlx;
use crate::manifest::{self, Defaults, FORMAT, validate_rel_path};
use crate::name::{parse_version, validate_segment};

/// The name of the file, at the root of a repository.
pub const FILE: &str = "packages.kdl";
/// The tag pattern when `release.tag` is not given.
pub const DEFAULT_TAG: &str = "v{version}";

/// The root manifest of a repository that holds packages: the defaults they share, where they are and how they are released.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RepoManifest {
    /// The `format` number of the file.
    pub format: u32,
    /// The values every package of the repository takes when its own `package.kdl` leaves them out.
    pub defaults: Defaults,
    /// The `find` and `package` lines, in file order.
    pub packages: Vec<PackageEntry>,
    /// How releases are tagged, where built eggs go and which versions are withdrawn.
    pub release: Release,
    /// Nodes Hatchery does not know, in file order.
    pub extra: Vec<KdlNode>,
}

/// A line of the `packages` block.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PackageEntry {
    /// Every folder matching the glob that has a `package.kdl`.
    Find {
        /// The glob, relative to the root of the repository and `/`-separated.
        glob: String,
        /// The tag pattern of the packages it finds. None means the release's one.
        tag: Option<String>,
    },
    /// One folder.
    One {
        /// The package's folder, relative to the root of the repository.
        path: String,
        /// The tag pattern of the package. None means the release's one.
        tag: Option<String>,
    },
}

/// The `release` block of `packages.kdl`.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct Release {
    /// The tag pattern for all packages, with `{version}` (and optionally
    /// `{id}`) in it.
    pub tag: Option<String>,
    /// Where built eggs are published. None when the repository publishes none.
    pub eggs: Option<EggsBranch>,
    /// The versions that were released and must not be installed.
    pub withdrawn: Vec<Withdrawal>,
    /// Nodes of the `release` block that Hatchery does not know, in file order.
    pub extra: Vec<KdlNode>,
}

/// Where built eggs are published: a branch, and a folder in it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EggsBranch {
    /// The branch that holds the eggs.
    pub branch: String,
    /// A folder of the branch; its root when None.
    pub path: Option<String>,
}

/// A released version that must not be installed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Withdrawal {
    /// The package's id, or `group/id`.
    pub package: String,
    /// The withdrawn version, a complete version.
    pub version: Version,
    /// Why it was withdrawn. None when no reason was given.
    pub reason: Option<String>,
}

/// A package folder of a repository, relative to its root, and the tag
/// pattern of its releases.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RepoPackage {
    /// The package's folder, relative to the root of the repository and `/`-separated.
    pub path: String,
    /// The tag pattern of the package's releases, its own or the release's one.
    pub tag: String,
}

impl RepoManifest {
    /// Reads the text of a `packages.kdl`. Fails with [`Error::Invalid`] when the KDL is wrong, a tag pattern does not hold `{version}` once, or a `packages` line is neither `find` nor `package`.
    pub fn parse(text: &str) -> Result<RepoManifest> {
        let what = FILE;
        let doc = kdlx::parse(text, what)?;
        let mut r = RepoManifest {
            format: FORMAT,
            defaults: Defaults::default(),
            packages: Vec::new(),
            release: Release::default(),
            extra: Vec::new(),
        };
        for n in doc.nodes() {
            match kdlx::name(n) {
                "format" => r.format = manifest::parse_format(n, what)?,
                "defaults" => r.defaults = parse_defaults(n, what)?,
                "packages" => {
                    for p in kdlx::children(n) {
                        kdlx::only_props(p, &["tag"], what)?;
                        let tag = kdlx::prop_string(p, "tag", what)?;
                        if let Some(t) = &tag {
                            check_pattern(t)?;
                        }
                        let arg = kdlx::one_string(p, what)?;
                        validate_rel_path(what, arg.trim_end_matches('/'))?;
                        match kdlx::name(p) {
                            "find" => r.packages.push(PackageEntry::Find { glob: arg, tag }),
                            "package" => r.packages.push(PackageEntry::One { path: arg, tag }),
                            other => {
                                return Err(Error::invalid(
                                    what,
                                    format!("packages holds find and package lines, not {other}"),
                                ));
                            }
                        }
                    }
                }
                "release" => r.release = parse_release(n, what)?,
                _ => r.extra.push(n.clone()),
            }
        }
        Ok(r)
    }

    /// Reads a `packages.kdl` from a path. Fails with [`Error::Io`] when it cannot be read, and with [`Error::Invalid`] naming the path when it is not valid.
    pub fn load(path: &Path) -> Result<RepoManifest> {
        let text = fs::read_to_string(path).at_path(path)?;
        RepoManifest::parse(&text).map_err(|e| match e {
            Error::Invalid { message, .. } => Error::invalid(path.display(), message),
            e => e,
        })
    }

    /// The tag pattern for packages without their own.
    pub fn tag_pattern(&self) -> &str {
        self.release.tag.as_deref().unwrap_or(DEFAULT_TAG)
    }

    /// The package folders, given every folder of the repository that holds
    /// a `package.kdl` (relative, `/`-separated). Folders named by a
    /// `package` line come first, then those found, each once.
    pub fn package_dirs<S: AsRef<str>>(&self, with_manifest: &[S]) -> Vec<RepoPackage> {
        let mut out: Vec<RepoPackage> = Vec::new();
        let mut push = |path: &str, tag: &Option<String>| {
            let path = path
                .strip_prefix("./")
                .unwrap_or(path)
                .trim_end_matches('/')
                .to_string();
            if !out.iter().any(|p| p.path == path) {
                out.push(RepoPackage {
                    path,
                    tag: tag
                        .clone()
                        .unwrap_or_else(|| self.tag_pattern().to_string()),
                });
            }
        };
        for e in &self.packages {
            if let PackageEntry::One { path, tag } = e {
                push(path, tag);
            }
        }
        for e in &self.packages {
            if let PackageEntry::Find { glob, tag } = e {
                let g = Glob::new(
                    glob.strip_prefix("./")
                        .unwrap_or(glob)
                        .trim_end_matches('/'),
                );
                let mut found: Vec<&str> = with_manifest
                    .iter()
                    .map(AsRef::as_ref)
                    .filter(|d| g.matches(d))
                    .collect();
                found.sort();
                for d in found {
                    push(d, tag);
                }
            }
        }
        out
    }

    /// The package folders of a repository on disk.
    pub fn find_packages(&self, root: &Path) -> Result<Vec<RepoPackage>> {
        let mut dirs = Vec::new();
        collect_manifest_dirs(root, root, &mut dirs, 0)?;
        Ok(self.package_dirs(&dirs))
    }

    /// The withdrawal of a version, if any. `package` matches either the id
    /// or `group/id`.
    pub fn withdrawn(&self, group: &str, id: &str, version: &Version) -> Option<&Withdrawal> {
        let full = format!("{group}/{id}");
        self.release
            .withdrawn
            .iter()
            .find(|w| (w.package == id || w.package == full) && &w.version == version)
    }
}

fn collect_manifest_dirs(
    root: &Path,
    dir: &Path,
    out: &mut Vec<String>,
    depth: usize,
) -> Result<()> {
    if depth > 16 {
        return Ok(());
    }
    let mut entries: Vec<_> = fs::read_dir(dir)
        .at_path(dir)?
        .collect::<std::io::Result<Vec<_>>>()
        .at_path(dir)?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "node_modules" || name == "target" {
            continue;
        }
        let path = e.path();
        let ft = e.file_type().at_path(&path)?;
        if ft.is_dir() {
            if path.join(manifest::FILE).is_file()
                && let Ok(rel) = path.strip_prefix(root)
            {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
            collect_manifest_dirs(root, &path, out, depth + 1)?;
        }
    }
    Ok(())
}

/// Checks a tag pattern: it must hold `{version}` once.
pub fn check_pattern(p: &str) -> Result<()> {
    if p.matches("{version}").count() != 1 {
        return Err(Error::invalid(
            FILE,
            format!("tag pattern {p:?} must hold {{version}} once"),
        ));
    }
    Ok(())
}

/// The tag of a version: the pattern with `{version}` and `{id}` filled in.
pub fn tag_for(pattern: &str, id: &str, version: &Version) -> String {
    pattern
        .replace("{id}", id)
        .replace("{version}", &version.to_string())
}

/// The version a tag names under a pattern, if it matches.
pub fn version_from_tag(pattern: &str, id: &str, tag: &str) -> Option<Version> {
    let pattern = pattern.replace("{id}", id);
    let (before, after) = pattern.split_once("{version}")?;
    let v = tag.strip_prefix(before)?.strip_suffix(after)?;
    Version::parse(v).ok()
}

fn parse_defaults(node: &KdlNode, what: &str) -> Result<Defaults> {
    let mut d = Defaults::default();
    for n in kdlx::children(node) {
        match kdlx::name(n) {
            "package" => {
                for p in kdlx::children(n) {
                    match kdlx::name(p) {
                        "group" => {
                            let g = kdlx::one_string(p, what)?;
                            validate_segment("group", &g)?;
                            d.group = Some(g);
                        }
                        "version" => d.version = Some(parse_version(&kdlx::one_string(p, what)?)?),
                        "id" => return Err(Error::invalid(what, "defaults cannot give an id")),
                        _ => d.package_extra.push(p.clone()),
                    }
                }
            }
            "about" => d.about = manifest::parse_about(n, what)?,
            other => {
                return Err(Error::invalid(
                    what,
                    format!("defaults holds package and about, not {other}"),
                ));
            }
        }
    }
    Ok(d)
}

fn parse_release(node: &KdlNode, what: &str) -> Result<Release> {
    let mut r = Release::default();
    for n in kdlx::children(node) {
        match kdlx::name(n) {
            "tag" => {
                let t = kdlx::one_string(n, what)?;
                check_pattern(&t)?;
                r.tag = Some(t);
            }
            "eggs" => {
                kdlx::only_props(n, &["branch", "path"], what)?;
                let branch =
                    kdlx::prop_string(n, "branch", what)?.unwrap_or_else(|| "eggs".to_string());
                let path = kdlx::prop_string(n, "path", what)?;
                if let Some(p) = &path {
                    validate_rel_path(what, p)?;
                }
                r.eggs = Some(EggsBranch { branch, path });
            }
            "withdrawn" => {
                kdlx::only_props(n, &["reason"], what)?;
                let args = kdlx::strings(n, what)?;
                let [package, version] = <[String; 2]>::try_from(args)
                    .map_err(|_| Error::invalid(what, "withdrawn takes a package and a version"))?;
                r.withdrawn.push(Withdrawal {
                    package,
                    version: parse_version(&version)?,
                    reason: kdlx::prop_string(n, "reason", what)?,
                });
            }
            _ => r.extra.push(n.clone()),
        }
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = r#"
// packages.kdl
format 1

defaults {
  package {
    group aiviarium
    version "1.4.0"
  }
  about {
    license MIT
    license Apache-2.0
    author "helight"
    repository "https://example.org/parrot.git"
  }
}

packages {
  find "agents/*"                                          // every folder there with a package.kdl
  package "shared/agent-base" tag="agent-base-v{version}"   // one folder, with its own tags
}

release {
  tag "v{version}"            // how a version is tagged
  eggs branch="eggs"          // where built eggs are published
  withdrawn claude "1.3.2" reason="broke sign-in on Ubuntu 26.04"
}
"#;

    #[test]
    fn parses_the_root_manifest() {
        let r = RepoManifest::parse(ROOT).unwrap();
        assert_eq!(r.defaults.group.as_deref(), Some("aiviarium"));
        assert_eq!(r.defaults.about.licenses, ["MIT", "Apache-2.0"]);
        assert_eq!(
            r.release.eggs,
            Some(EggsBranch {
                branch: "eggs".into(),
                path: None
            })
        );
        let w = r
            .withdrawn("aiviarium", "claude", &Version::new(1, 3, 2))
            .unwrap();
        assert_eq!(w.reason.as_deref(), Some("broke sign-in on Ubuntu 26.04"));
        assert!(
            r.withdrawn("aiviarium", "claude", &Version::new(1, 3, 3))
                .is_none()
        );

        let dirs = r.package_dirs(&[
            "agents/claude",
            "agents/codex",
            "agents/deep/x",
            "shared/agent-base",
            "other",
        ]);
        assert_eq!(
            dirs,
            [
                RepoPackage {
                    path: "shared/agent-base".into(),
                    tag: "agent-base-v{version}".into()
                },
                RepoPackage {
                    path: "agents/claude".into(),
                    tag: "v{version}".into()
                },
                RepoPackage {
                    path: "agents/codex".into(),
                    tag: "v{version}".into()
                },
            ]
        );
    }

    #[test]
    fn maps_tags_and_versions() {
        let v = Version::new(1, 4, 0);
        assert_eq!(tag_for("v{version}", "claude", &v), "v1.4.0");
        assert_eq!(tag_for("{id}-v{version}", "claude", &v), "claude-v1.4.0");
        assert_eq!(
            version_from_tag("v{version}", "x", "v1.4.0"),
            Some(v.clone())
        );
        assert_eq!(
            version_from_tag("agent-base-v{version}", "x", "agent-base-v1.4.0"),
            Some(v)
        );
        assert_eq!(
            version_from_tag("v{version}", "x", "agent-base-v1.4.0"),
            None
        );
        assert_eq!(version_from_tag("v{version}", "x", "v1.4"), None);
        assert!(check_pattern("release").is_err());
    }

    #[test]
    fn finds_packages_on_disk() {
        let t = tempfile::tempdir().unwrap();
        for d in ["agents/claude", "agents/codex", "shared/agent-base"] {
            fs::create_dir_all(t.path().join(d)).unwrap();
            fs::write(t.path().join(d).join("package.kdl"), "").unwrap();
        }
        fs::create_dir_all(t.path().join("agents/empty")).unwrap();
        let r = RepoManifest::parse(ROOT).unwrap();
        let found: Vec<_> = r
            .find_packages(t.path())
            .unwrap()
            .into_iter()
            .map(|p| p.path)
            .collect();
        assert_eq!(
            found,
            ["shared/agent-base", "agents/claude", "agents/codex"]
        );
    }
}
