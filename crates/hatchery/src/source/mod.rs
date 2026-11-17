//! Where packages come from. Git repositories, static registries, single git
//! folders, folders and files are built in; a tool adds a kind by
//! implementing [`Source`].

use bytes::Bytes;
use semver::{Version, VersionReq};

use crate::egg::Egg;
use crate::error::{Error, Result};
use crate::fetch::BoxFuture;
use crate::manifest::{About, Manifest, Requirement, variant_key};
use crate::name::Name;
use crate::time::Timestamp;

pub mod git;
pub mod git_folder;
pub mod local;
pub mod registry;

pub use git::GitRegistry;
pub use git_folder::GitFolder;
pub use local::{FileSource, FolderSource};
pub use registry::StaticRegistry;

/// A place packages are found and fetched.
pub trait Source: Send + Sync {
    /// A label for messages and the lock file: the registry's address.
    fn label(&self) -> &str;

    /// Whether the source is on this machine, which sets the default minimum
    /// release age to none.
    fn is_local(&self) -> bool {
        false
    }

    /// The packages the source offers, for listings and for finding a
    /// package by its id alone. A source that cannot be listed returns an
    /// empty list.
    fn names(&self) -> BoxFuture<'_, Result<Vec<Name>>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// Every version of a package the source has, withdrawn ones included,
    /// in any order. An empty list when it does not have the package.
    fn versions<'a>(&'a self, name: &'a Name) -> BoxFuture<'a, Result<Vec<Offer>>>;

    /// The targets of an offer that came without them. The default returns
    /// the details the offer carries.
    fn details<'a>(&'a self, offer: &'a Offer) -> BoxFuture<'a, Result<Details>> {
        Box::pin(async move {
            offer.details.clone().ok_or_else(|| {
                Error::other(format!(
                    "{} {}: no details from {}",
                    offer.name, offer.version, offer.registry
                ))
            })
        })
    }

    /// The base egg of an offer, with its checksum and signature verified.
    fn fetch<'a>(&'a self, offer: &'a Offer) -> BoxFuture<'a, Result<Egg>>;

    /// A variant egg of an offer, verified. `key` is the variant's key such
    /// as `x86_64`.
    fn fetch_variant<'a>(
        &'a self,
        offer: &'a Offer,
        target: &'a str,
        key: &'a str,
    ) -> BoxFuture<'a, Result<Egg>> {
        let _ = (target, key);
        Box::pin(async move {
            Err(Error::NotFound {
                at: crate::error::At::new(&offer.name)
                    .version(format!("{} {target}.{key}", offer.version))
                    .registry(&offer.registry),
            })
        })
    }
}

/// One version a source offers.
#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    pub name: Name,
    pub version: Version,
    /// The label of the source.
    pub registry: String,
    /// When the egg was published, as the signed index says. None for a
    /// version from a git tag.
    pub published: Option<Timestamp>,
    pub withdrawn: Option<Withdrawn>,
    /// The base egg's checksum, when the source knows it before fetching.
    pub sha256: Option<String>,
    pub signature: Option<String>,
    /// False for a version installed from source, such as a git tag
    /// without a built egg.
    pub built: bool,
    /// The commit, for git sources.
    pub commit: Option<String>,
    /// The tag or branch, for git sources.
    pub git_ref: Option<String>,
    /// The package's folder in a git repository.
    pub path: Option<String>,
    /// Display data and targets. None when the source must look them up
    /// with [`Source::details`].
    pub details: Option<Details>,
}

impl Offer {
    /// An offer with only its identity, to fill in.
    pub fn new(name: Name, version: Version, registry: impl Into<String>) -> Offer {
        Offer {
            name,
            version,
            registry: registry.into(),
            published: None,
            withdrawn: None,
            sha256: None,
            signature: None,
            built: true,
            commit: None,
            git_ref: None,
            path: None,
            details: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Withdrawn {
    pub reason: Option<String>,
}

/// What resolution needs to know about a version, without its files.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Details {
    pub about: About,
    pub targets: Vec<TargetInfo>,
}

impl Details {
    pub fn from_manifest(m: &Manifest) -> Details {
        let group = &m.package.group;
        let deps = |ds: &[crate::manifest::Dependency]| {
            ds.iter()
                .map(|d| (d.name(group), d.range.clone()))
                .collect()
        };
        Details {
            about: m.about.clone(),
            targets: m
                .targets
                .iter()
                .map(|t| TargetInfo {
                    name: t.name.clone(),
                    build: t.build.is_some() || t.variants.iter().any(|v| v.build.is_some()),
                    requires: t.requires.clone(),
                    dependencies: deps(&t.dependencies),
                    optional_dependencies: deps(&t.optional_dependencies),
                    variants: t
                        .variants
                        .iter()
                        .map(|v| VariantInfo {
                            os: v.os.clone(),
                            arch: v.arch.clone(),
                            sha256: None,
                            signature: None,
                            published: None,
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    pub fn target(&self, name: &str) -> Option<&TargetInfo> {
        self.targets.iter().find(|t| t.name == name)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TargetInfo {
    pub name: String,
    /// Whether the target, or one of its variants, has a build command.
    pub build: bool,
    pub requires: Vec<Requirement>,
    pub dependencies: Vec<(Name, VersionReq)>,
    pub optional_dependencies: Vec<(Name, VersionReq)>,
    pub variants: Vec<VariantInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantInfo {
    pub os: Option<String>,
    pub arch: Option<String>,
    /// The variant egg's checksum and signature, when it is published.
    pub sha256: Option<String>,
    pub signature: Option<String>,
    pub published: Option<Timestamp>,
}

impl VariantInfo {
    pub fn key(&self) -> String {
        variant_key(self.os.as_deref(), self.arch.as_deref())
    }
}

/// Reads the files of a static registry layout, wherever they are: a
/// folder, an HTTP server, or a branch of a git repository.
pub trait RegistryFiles: Send + Sync {
    /// The registry's address, for messages.
    fn label(&self) -> &str;

    fn is_local(&self) -> bool {
        false
    }

    /// The file at a `/`-separated path under the registry's base. None
    /// when there is no such file.
    fn read<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<Option<Bytes>>>;
}
