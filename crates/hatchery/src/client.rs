//! The client: what a tool uses to list, install, update and remove.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use semver::Version;

use crate::config::{Config, RegistryConfig};
use crate::egg::Egg;
use crate::error::Result;
use crate::fetch::Fetch;
use crate::git::Git;
use crate::lock::LockedPackage;
use crate::manifest::{About, Manifest};
use crate::name::{Name, Spec};
use crate::platform::Platform;
use crate::source::{Source, Withdrawn};
use crate::target::Target;
use crate::time::{Clock, Timestamp};

/// Makes a [`Source`] for a registry line of a kind a tool adds.
pub type SourceFactory =
    Arc<dyn Fn(&RegistryConfig, &SourceContext) -> Result<Arc<dyn Source>> + Send + Sync>;

/// What a [`SourceFactory`] may use.
#[derive(Clone)]
pub struct SourceContext {
    pub fetch: Option<Arc<dyn Fetch>>,
    pub cache_dir: PathBuf,
    pub git: Git,
}

pub struct ClientBuilder {
    _private: (),
}

impl ClientBuilder {
    pub fn config(self, config: Config) -> Self {
        let _ = config;
        todo!()
    }

    /// A tool this client installs for. Call it once per target.
    pub fn target(self, target: impl Target + 'static) -> Self {
        let _ = target;
        todo!()
    }

    pub fn target_arc(self, target: Arc<dyn Target>) -> Self {
        let _ = target;
        todo!()
    }

    /// How HTTP registries and egg URLs are fetched. Without one, they fail.
    pub fn fetch(self, fetch: impl Fetch + 'static) -> Self {
        let _ = fetch;
        todo!()
    }

    pub fn fetch_arc(self, fetch: Arc<dyn Fetch>) -> Self {
        let _ = fetch;
        todo!()
    }

    /// Where git checkouts are cached. The user's cache folder by default.
    pub fn cache_dir(self, dir: impl Into<PathBuf>) -> Self {
        let _ = dir.into();
        todo!()
    }

    pub fn clock(self, clock: Clock) -> Self {
        let _ = clock;
        todo!()
    }

    pub fn git(self, git: Git) -> Self {
        let _ = git;
        todo!()
    }

    /// Registry lines of this node name are served by the factory's sources.
    pub fn kind(self, name: &str, factory: SourceFactory) -> Self {
        let _ = (name, factory);
        todo!()
    }

    pub fn build(self) -> Result<Client> {
        todo!()
    }
}

/// Changes how one `add` or `update` resolves.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Install the named package even if it is younger than the minimum
    /// release age.
    pub allow_young: bool,
}

/// What an `add`, `update` or `install` did.
#[derive(Debug, Clone)]
pub struct Report {
    pub dir: PathBuf,
    /// The packages installed or changed, dependencies first.
    pub packages: Vec<Change>,
    /// Withdrawn versions that stay installed, and similar notes.
    pub warnings: Vec<String>,
}

/// One package an operation installed.
#[derive(Debug, Clone)]
pub struct Change {
    pub name: Name,
    pub version: Version,
    /// The version that was installed before, if any.
    pub previous: Option<Version>,
    pub sha256: String,
    /// The label of the source it came from.
    pub registry: String,
    pub egg: Egg,
}

/// A package installed in a folder.
#[derive(Debug, Clone)]
pub struct Installed {
    pub name: Name,
    pub version: Version,
    pub sha256: String,
    pub targets: Vec<String>,
    /// The package's folder, `<dir>/<group>/<id>`.
    pub dir: PathBuf,
    pub manifest: Manifest,
    pub lock: LockedPackage,
}

/// A package the registries offer, for listings.
#[derive(Debug, Clone)]
pub struct Offered {
    pub name: Name,
    pub about: About,
    /// The registry that serves it: the first that has it.
    pub registry: String,
    /// Newest first.
    pub versions: Vec<OfferedVersion>,
    pub installed: Option<Version>,
    /// The newest version an update would install, when newer than the
    /// installed one.
    pub updatable: Option<Version>,
    /// When the newest version becomes installable, if it is held.
    pub held_until: Option<Timestamp>,
}

#[derive(Debug, Clone)]
pub struct OfferedVersion {
    pub version: Version,
    pub published: Option<Timestamp>,
    pub withdrawn: Option<Withdrawn>,
    pub held_until: Option<Timestamp>,
    /// Why this version cannot be installed here, if it cannot.
    pub problem: Option<String>,
}

/// The eggs `fetch` got.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub base: Egg,
    pub variants: Vec<Egg>,
}

pub struct Client {
    _private: (),
}

impl Client {
    pub fn builder() -> ClientBuilder {
        todo!()
    }

    /// Every package the registries offer: about, versions, installed,
    /// updatable, held until.
    pub async fn offered(&self, dir: &Path) -> Result<Vec<Offered>> {
        let _ = dir;
        todo!()
    }

    /// Resolves, verifies, unpacks and locks a package and its dependencies.
    pub async fn add(&self, spec: &Spec, dir: &Path) -> Result<Report> {
        self.add_with(spec, dir, &Options::default()).await
    }

    pub async fn add_with(&self, spec: &Spec, dir: &Path, opts: &Options) -> Result<Report> {
        let _ = (spec, dir, opts);
        todo!()
    }

    /// Updates one package, or all, to the newest allowed versions.
    pub async fn update(&self, name: Option<&Name>, dir: &Path) -> Result<Report> {
        let _ = (name, dir);
        todo!()
    }

    /// Installs exactly what `eggs.lock` lists, checking checksums and
    /// commits. Packages already installed at the locked version are left.
    pub async fn install(&self, dir: &Path) -> Result<Report> {
        let _ = dir;
        todo!()
    }

    /// Removes a package. Refuses while a dependent is installed.
    pub fn remove(&self, name: &Name, dir: &Path) -> Result<()> {
        let _ = (name, dir);
        todo!()
    }

    pub fn installed(&self, dir: &Path) -> Result<Vec<Installed>> {
        let _ = dir;
        todo!()
    }

    /// The files of one variant. Downloads and verifies its egg on first use.
    pub async fn variant(&self, pkg: &Installed, platform: &Platform) -> Result<PathBuf> {
        let _ = (pkg, platform);
        todo!()
    }

    /// Fetches a package's eggs without installing: the base egg, and the
    /// variant eggs named as `<target>.<key>` (all when None).
    pub async fn fetch(
        &self,
        spec: &Spec,
        variants: Option<&[String]>,
        opts: &Options,
    ) -> Result<Fetched> {
        let _ = (spec, variants, opts);
        todo!()
    }
}
