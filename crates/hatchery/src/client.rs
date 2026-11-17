//! The client: what a tool uses to list, install, update and remove.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use semver::{Version, VersionReq};

use crate::config::{Config, RegistryConfig};
use crate::egg::Egg;
use crate::error::{At, Error, IoContext, Result};
use crate::fetch::{Fetch, Token, Url};
use crate::git::Git;
use crate::lock::{Lock, LockedPackage};
use crate::manifest::{self, About, Manifest};
use crate::name::{Name, Spec, Wanted};
use crate::platform::{Platform, Variants};
use crate::resolve::{Constraints, InstalledPkg, Origin, Pick, Plan, Resolver, SourceEntry, at_of};
use crate::seen::Seen;
use crate::sign;
use crate::source::{
    FileSource, FolderSource, GitFolder, GitRegistry, Offer, Source, StaticRegistry, Withdrawn,
};
use crate::target::{self, Target};
use crate::time::{self, Clock, Timestamp};

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

#[derive(Default)]
pub struct ClientBuilder {
    config: Option<Config>,
    targets: Vec<Arc<dyn Target>>,
    fetch: Option<Arc<dyn Fetch>>,
    cache_dir: Option<PathBuf>,
    clock: Option<Clock>,
    git: Option<Git>,
    kinds: BTreeMap<String, SourceFactory>,
}

/// `$XDG_CACHE_HOME/hatchery`, else `~/.cache/hatchery`; on Windows
/// `%LOCALAPPDATA%\hatchery`.
fn default_cache_dir() -> PathBuf {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(x) = var("XDG_CACHE_HOME") {
        return x.join("hatchery");
    }
    if cfg!(windows)
        && let Some(l) = var("LOCALAPPDATA")
    {
        return l.join("hatchery");
    }
    if let Some(h) = var("HOME").or_else(|| var("USERPROFILE")) {
        return h.join(".cache").join("hatchery");
    }
    std::env::temp_dir().join("hatchery-cache")
}

impl ClientBuilder {
    pub fn config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }

    /// A tool this client installs for. Call it once per target.
    pub fn target(self, target: impl Target + 'static) -> Self {
        self.target_arc(Arc::new(target))
    }

    pub fn target_arc(mut self, target: Arc<dyn Target>) -> Self {
        self.targets.push(target);
        self
    }

    /// How HTTP registries and egg URLs are fetched. Without one, they fail.
    pub fn fetch(self, fetch: impl Fetch + 'static) -> Self {
        self.fetch_arc(Arc::new(fetch))
    }

    pub fn fetch_arc(mut self, fetch: Arc<dyn Fetch>) -> Self {
        self.fetch = Some(fetch);
        self
    }

    /// Where git checkouts are cached. The user's cache folder by default.
    pub fn cache_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cache_dir = Some(dir.into());
        self
    }

    pub fn clock(mut self, clock: Clock) -> Self {
        self.clock = Some(clock);
        self
    }

    pub fn git(mut self, git: Git) -> Self {
        self.git = Some(git);
        self
    }

    /// Registry lines of this node name are served by the factory's sources.
    pub fn kind(mut self, name: &str, factory: SourceFactory) -> Self {
        self.kinds.insert(name.to_string(), factory);
        self
    }

    pub fn build(self) -> Result<Client> {
        let config = self.config.unwrap_or_else(Config::empty);
        let cache_dir = self.cache_dir.unwrap_or_else(default_cache_dir);
        let git = self.git.unwrap_or_default();
        let ctx = SourceContext {
            fetch: self.fetch.clone(),
            cache_dir: cache_dir.clone(),
            git: git.clone(),
        };
        let mut registries = Vec::new();
        for cfg in &config.registries {
            let line = format!("registry {} {}", cfg.kind, cfg.location);
            let key = cfg
                .key
                .as_deref()
                .map(sign::parse_verifying_key)
                .transpose()
                .map_err(|e| Error::invalid(&line, e))?;
            let source: Arc<dyn Source> = match cfg.kind.as_str() {
                "git" => Arc::new(GitRegistry::new(
                    cfg.location.clone(),
                    cache_dir.join("git"),
                    git.clone(),
                    key,
                )),
                "http" => {
                    let fetch = self.fetch.clone().ok_or_else(|| {
                        Error::invalid(&line, "an http registry needs a Fetch, and none was given")
                    })?;
                    let url = Url::parse(&cfg.location)
                        .map_err(|e| Error::invalid(&line, format!("not an address: {e}")))?;
                    let token = cfg.token_env.as_deref().and_then(Token::from_env);
                    Arc::new(StaticRegistry::http(url, fetch, token, key))
                }
                "dir" => Arc::new(StaticRegistry::dir(&cfg.location, key)),
                other => match self.kinds.get(other) {
                    Some(factory) => factory(cfg, &ctx)?,
                    None => {
                        return Err(Error::invalid(
                            &line,
                            format!("{other:?} is not a kind of registry this client knows"),
                        ));
                    }
                },
            };
            registries.push(Registry {
                source,
                min_age: config.min_age_for(cfg),
                config: cfg.clone(),
            });
        }
        Ok(Client {
            targets: self.targets,
            fetch: self.fetch,
            cache_dir,
            clock: self.clock.unwrap_or_else(time::system_clock),
            git,
            registries,
        })
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

struct Registry {
    source: Arc<dyn Source>,
    config: RegistryConfig,
    min_age: Duration,
}

pub struct Client {
    targets: Vec<Arc<dyn Target>>,
    fetch: Option<Arc<dyn Fetch>>,
    cache_dir: PathBuf,
    clock: Clock,
    git: Git,
    registries: Vec<Registry>,
}

/// How many of the newest versions of a package a listing looks into for
/// problems; the rest are listed without checking.
const CHECKED_VERSIONS: usize = 5;

/// An egg that was fetched and checked, ready to be written.
struct Prepared {
    name: Name,
    offer: Offer,
    origin: Origin,
    /// The label of the source it came from.
    label: String,
    egg: Egg,
    base_sha: String,
    /// Variant eggs fetched apart from `egg`.
    variant_eggs: Vec<Egg>,
    /// `"<target>.<key>" = sha256` of every variant egg that is installed.
    variants: BTreeMap<String, String>,
    /// The client's targets the package has.
    targets: Vec<String>,
    /// What those targets require.
    deps: Vec<Name>,
}

/// Where a lock entry's package comes from.
enum LockSource {
    Registry(usize),
    Single(Arc<dyn Source>, Origin),
}

fn at_name(name: &Name, version: &Version) -> At {
    At::new(name).version(version)
}

fn check_variant(offer: &Offer, target: &str, key: &str, egg: &Egg) -> Result<()> {
    let fits = egg.variant().is_some_and(|h| {
        h.name == offer.name && h.version == offer.version && h.target == target && h.key() == key
    });
    if fits {
        Ok(())
    } else {
        Err(Error::invalid(
            at_of(offer),
            format!("the egg is not the {target}.{key} variant of this version"),
        ))
    }
}

fn split_variant(s: &str) -> Result<(&str, &str)> {
    s.split_once('.')
        .filter(|(t, k)| !t.is_empty() && !k.is_empty())
        .ok_or_else(|| Error::invalid("variant", format!("{s:?} is not <target>.<key>")))
}

fn withdrawn_note(name: &Name, version: &Version, w: &Withdrawn) -> String {
    match &w.reason {
        Some(r) => format!("{name} {version} is withdrawn ({r})"),
        None => format!("{name} {version} is withdrawn"),
    }
}

/// The packages in the order their dependencies need: dependencies first.
fn order(prepared: Vec<Prepared>) -> Vec<Prepared> {
    fn visit(name: &Name, by_name: &mut BTreeMap<Name, Prepared>, out: &mut Vec<Prepared>) {
        let Some(p) = by_name.remove(name) else {
            return;
        };
        for d in p.deps.clone() {
            visit(&d, by_name, out);
        }
        out.push(p);
    }
    let names: Vec<Name> = prepared.iter().map(|p| p.name.clone()).collect();
    let mut by_name: BTreeMap<Name, Prepared> =
        prepared.into_iter().map(|p| (p.name.clone(), p)).collect();
    let mut out = Vec::new();
    for n in names {
        visit(&n, &mut by_name, &mut out);
    }
    out
}

impl Prepared {
    fn locked(&self) -> LockedPackage {
        let o = &self.offer;
        let mut l = LockedPackage {
            name: self.name.clone(),
            version: o.version.clone(),
            targets: self.targets.clone(),
            registry: None,
            git: None,
            path: o.path.clone(),
            git_ref: o.git_ref.clone(),
            commit: o.commit.clone(),
            file: None,
            folder: None,
            sha256: self.base_sha.clone(),
            variants: self.variants.clone(),
        };
        match &self.origin {
            Origin::Registry => l.registry = Some(self.label.clone()),
            Origin::Git { url, path, git_ref } => {
                l.git = Some(url.clone());
                l.path = l.path.take().or_else(|| path.clone());
                l.git_ref = l.git_ref.take().or_else(|| git_ref.clone());
            }
            Origin::File { location } => l.file = Some(location.clone()),
            Origin::Folder { path } => l.folder = Some(path.clone()),
        }
        l
    }
}

impl Client {
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    fn now(&self) -> Timestamp {
        (self.clock)()
    }

    fn need_targets(&self) -> Result<()> {
        if self.targets.is_empty() {
            return Err(Error::invalid(
                "client",
                "it has no target; add one with ClientBuilder::target",
            ));
        }
        Ok(())
    }

    fn git_cache(&self) -> PathBuf {
        self.cache_dir.join("git")
    }

    fn git_folder(
        &self,
        url: &str,
        path: Option<String>,
        git_ref: Option<String>,
    ) -> Arc<dyn Source> {
        Arc::new(GitFolder::new(
            url,
            path,
            git_ref,
            self.git_cache(),
            self.git.clone(),
        ))
    }

    /// The installed packages of a folder, from the lock and each
    /// package's `package.kdl`. A lock entry whose folder is gone is left out.
    fn read_installed(&self, dir: &Path, lock: &Lock) -> Result<Vec<Installed>> {
        let mut out = Vec::new();
        for lp in &lock.packages {
            let pkg_dir = dir.join(lp.name.dir());
            let path = pkg_dir.join(manifest::FILE);
            let text = match fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e).at_path(&path),
            };
            let manifest = Manifest::parse(&text).map_err(|e| match e {
                Error::Invalid { message, .. } => Error::invalid(path.display(), message),
                e => e,
            })?;
            out.push(Installed {
                name: lp.name.clone(),
                version: lp.version.clone(),
                sha256: lp.sha256.clone(),
                targets: lp.targets.clone(),
                dir: pkg_dir,
                manifest,
                lock: lp.clone(),
            });
        }
        Ok(out)
    }

    fn installed_deps(i: &Installed) -> Vec<(Name, VersionReq)> {
        i.manifest
            .targets
            .iter()
            .filter(|t| i.targets.contains(&t.name))
            .flat_map(|t| &t.dependencies)
            .map(|d| (d.name(&i.manifest.package.group), d.range.clone()))
            .collect()
    }

    fn new_resolver(&self, seen: Seen, installed: &[Installed]) -> Resolver {
        let sources = self
            .registries
            .iter()
            .map(|r| SourceEntry::registry(r.source.clone(), r.config.only.clone(), r.min_age))
            .collect();
        let mut r = Resolver::new(sources, self.targets.clone(), self.now(), seen);
        r.set_installed(
            installed
                .iter()
                .map(|i| {
                    (
                        i.name.clone(),
                        InstalledPkg {
                            version: i.version.clone(),
                            dependencies: Client::installed_deps(i),
                        },
                    )
                })
                .collect(),
        );
        r
    }

    fn registry_index(&self, label: &str) -> Option<usize> {
        self.registries
            .iter()
            .position(|r| r.source.label() == label)
    }

    /// The source that installed a locked package.
    fn lock_source(&self, lp: &LockedPackage) -> Result<LockSource> {
        if let Some(label) = &lp.registry {
            return self
                .registry_index(label)
                .map(LockSource::Registry)
                .ok_or_else(|| {
                    Error::invalid(
                        crate::lock::FILE,
                        format!("{}: the registry {label} is not configured", lp.name),
                    )
                });
        }
        if let Some(url) = &lp.git {
            return Ok(LockSource::Single(
                self.git_folder(url, lp.path.clone(), lp.git_ref.clone()),
                Origin::Git {
                    url: url.clone(),
                    path: lp.path.clone(),
                    git_ref: lp.git_ref.clone(),
                },
            ));
        }
        if let Some(file) = &lp.file {
            return Ok(LockSource::Single(
                Arc::new(FileSource::new(file.clone(), self.fetch.clone())),
                Origin::File {
                    location: file.clone(),
                },
            ));
        }
        if let Some(folder) = &lp.folder {
            return Ok(LockSource::Single(
                Arc::new(FolderSource::new(folder.clone(), self.targets.clone())),
                Origin::Folder {
                    path: folder.clone(),
                },
            ));
        }
        Err(Error::invalid(
            crate::lock::FILE,
            format!(
                "{}: it says nothing of where the package came from",
                lp.name
            ),
        ))
    }

    fn source_of(&self, ls: &LockSource) -> (Arc<dyn Source>, Origin) {
        match ls {
            LockSource::Registry(i) => (self.registries[*i].source.clone(), Origin::Registry),
            LockSource::Single(s, o) => (s.clone(), o.clone()),
        }
    }

    /// The offer a lock entry names, with the locked commit set so that the
    /// source refuses a tag that moved.
    async fn lock_offer(&self, source: &Arc<dyn Source>, lp: &LockedPackage) -> Result<Offer> {
        let offers = source.versions(&lp.name).await?;
        let mut offer = offers
            .into_iter()
            .find(|o| o.version == lp.version)
            .ok_or_else(|| Error::NotFound {
                at: At::new(&lp.name)
                    .version(&lp.version)
                    .registry(source.label()),
            })?;
        if lp.commit.is_some() {
            offer.commit = lp.commit.clone();
        }
        Ok(offer)
    }

    /// Makes a source for one package the request names: its name, and the
    /// range to resolve.
    async fn single_root(
        &self,
        resolver: &mut Resolver,
        source: Arc<dyn Source>,
        origin: Origin,
    ) -> Result<(Name, VersionReq)> {
        let name = source
            .names()
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound {
                at: At::new(source.label()),
            })?;
        let idx = resolver.add_source(SourceEntry::single(source, origin));
        resolver.pin(name.clone(), idx);
        Ok((name, VersionReq::STAR))
    }

    /// A package of a git registry at a ref, through a folder of its
    /// repository.
    async fn git_ref_root(
        &self,
        resolver: &mut Resolver,
        w: &Wanted,
        git_ref: &str,
    ) -> Result<(Name, VersionReq)> {
        for (idx, reg) in self.registries.iter().enumerate() {
            if reg.config.kind != "git" {
                continue;
            }
            let name = match &w.group {
                Some(g) if reg.config.serves(g) => Name::new(g, &w.id)?,
                Some(_) => continue,
                None => {
                    let mut groups: Vec<String> = reg
                        .source
                        .names()
                        .await?
                        .into_iter()
                        .filter(|n| n.id == w.id && reg.config.serves(&n.group))
                        .map(|n| n.group)
                        .collect();
                    groups.sort();
                    groups.dedup();
                    match groups.len() {
                        0 => continue,
                        1 => Name::new(groups.remove(0), &w.id)?,
                        _ => {
                            return Err(Error::Ambiguous {
                                id: w.id.clone(),
                                groups,
                            });
                        }
                    }
                }
            };
            let offers = resolver.listing(idx, &name).await?;
            if offers.is_empty() {
                continue;
            }
            let path = offers.iter().find_map(|o| o.path.clone());
            let url = reg.config.location.clone();
            let source = self.git_folder(&url, path.clone(), Some(git_ref.to_string()));
            let origin = Origin::Git {
                url,
                path,
                git_ref: Some(git_ref.to_string()),
            };
            let (name, _) = self.single_root(resolver, source, origin).await?;
            return Ok((name, w.range()));
        }
        Err(Error::NotFound {
            at: At::new(w.label())
                .version(format!("ref {git_ref}"))
                .registry("git registries"),
        })
    }

    /// The package a spec names, and the range it must satisfy. Sources
    /// for a single package are added to the resolver and pinned.
    async fn root_of(&self, resolver: &mut Resolver, spec: &Spec) -> Result<(Name, VersionReq)> {
        match spec {
            Spec::Registry(w) => {
                if let Some(r) = &w.git_ref {
                    return self.git_ref_root(resolver, w, r).await;
                }
                let name = match &w.group {
                    Some(g) => Name::new(g, &w.id)?,
                    None => resolver.find_name(&w.id).await?,
                };
                Ok((name, w.range()))
            }
            Spec::Git { url, path, git_ref } => {
                let source = self.git_folder(url, path.clone(), git_ref.clone());
                let origin = Origin::Git {
                    url: url.clone(),
                    path: path.clone(),
                    git_ref: git_ref.clone(),
                };
                self.single_root(resolver, source, origin).await
            }
            Spec::Folder(p) => {
                let source: Arc<dyn Source> =
                    Arc::new(FolderSource::new(p.clone(), self.targets.clone()));
                let abs = fs::canonicalize(p).unwrap_or_else(|_| p.clone());
                let origin = Origin::Folder {
                    path: abs.display().to_string(),
                };
                self.single_root(resolver, source, origin).await
            }
            Spec::File(f) => {
                let source: Arc<dyn Source> =
                    Arc::new(FileSource::new(f.clone(), self.fetch.clone()));
                let origin = Origin::File {
                    location: f.clone(),
                };
                self.single_root(resolver, source, origin).await
            }
        }
    }

    /// Fetches an egg and checks it against what was resolved, before
    /// anything is written: the name and version, the requirements again,
    /// each target's own check and the variants.
    async fn prepare(
        &self,
        source: &Arc<dyn Source>,
        origin: &Origin,
        offer: &Offer,
        wanted: &[String],
        expected: Option<&LockedPackage>,
    ) -> Result<Prepared> {
        let at = at_of(offer);
        let egg = source.fetch(offer).await?;
        let m = egg.manifest();
        if m.name() != offer.name || *m.version() != offer.version {
            return Err(Error::invalid(
                &at,
                format!("the egg holds {} {}", m.name(), m.version()),
            ));
        }
        let base_sha = if egg.is_fat() {
            egg.base()?.sha256().to_string()
        } else {
            egg.sha256().to_string()
        };
        if let Some(l) = expected
            && l.sha256 != base_sha
        {
            return Err(Error::ChecksumMismatch {
                at,
                expected: l.sha256.clone(),
                actual: base_sha,
            });
        }

        let mut variants = BTreeMap::new();
        let mut variant_eggs: Vec<Egg> = Vec::new();
        for (t, k) in egg.embedded_variants() {
            if let Some(ve) = egg.embedded_variant(&t, &k)? {
                variants.insert(format!("{t}.{k}"), ve.sha256().to_string());
            }
        }

        let mut names = Vec::new();
        let mut deps = Vec::new();
        for target in &self.targets {
            let tname = target.name();
            if !wanted.iter().any(|w| w == tname) {
                continue;
            }
            let spec = m
                .target(tname)
                .ok_or_else(|| Error::invalid(&at, format!("the egg has no target {tname}")))?;
            if let Some((req, found)) = target::unmet(target.as_ref(), &spec.requires) {
                return Err(Error::Unmet {
                    at,
                    target: tname.to_string(),
                    fact: req.fact.clone(),
                    wanted: req.values.clone(),
                    found,
                });
            }
            let files = egg
                .target(tname)
                .ok_or_else(|| Error::invalid(&at, format!("the egg has no files of {tname}")))?;
            target.check(&files).map_err(|problems| Error::Check {
                target: tname.to_string(),
                problems,
            })?;
            deps.extend(spec.dependencies.iter().map(|d| d.name(&m.package.group)));
            names.push(tname.to_string());

            if let Variants::Matching(p) = target.variants()
                && !spec.variants.is_empty()
            {
                let v = spec.select_variant(&p).ok_or_else(|| Error::NoVariant {
                    at: at.clone(),
                    target: tname.to_string(),
                    platform: p.clone(),
                })?;
                let key = v.key();
                let id = format!("{tname}.{key}");
                if variants.contains_key(&id) {
                    continue;
                }
                let ve = source.fetch_variant(offer, tname, &key).await?;
                check_variant(offer, tname, &key, &ve)?;
                variants.insert(id, ve.sha256().to_string());
                variant_eggs.push(ve);
            }
        }

        // Variants the lock lists are fetched and checked too.
        if let Some(l) = expected {
            for (id, sha) in &l.variants {
                let (t, k) = split_variant(id)?;
                let actual = match variants.get(id) {
                    Some(a) => a.clone(),
                    None => {
                        let ve = source.fetch_variant(offer, t, k).await?;
                        check_variant(offer, t, k, &ve)?;
                        let a = ve.sha256().to_string();
                        variants.insert(id.clone(), a.clone());
                        variant_eggs.push(ve);
                        a
                    }
                };
                if &actual != sha {
                    return Err(Error::ChecksumMismatch {
                        at: at_of(offer).version(format!("{} {id}", offer.version)),
                        expected: sha.clone(),
                        actual,
                    });
                }
            }
        }

        Ok(Prepared {
            name: offer.name.clone(),
            offer: offer.clone(),
            origin: origin.clone(),
            label: source.label().to_string(),
            egg,
            base_sha,
            variant_eggs,
            variants,
            targets: names,
            deps,
        })
    }

    /// Unpacks what was prepared, dependencies first, and writes the lock
    /// after each package.
    fn write(
        &self,
        dir: &Path,
        lock: &mut Lock,
        prepared: Vec<Prepared>,
        previous: &BTreeMap<Name, Version>,
        keep_lock_entry: bool,
    ) -> Result<Vec<Change>> {
        fs::create_dir_all(dir).at_path(dir)?;
        let mut changes = Vec::new();
        for p in order(prepared) {
            let pkg_dir = dir.join(p.name.dir());
            p.egg.unpack(&pkg_dir)?;
            for ve in &p.variant_eggs {
                ve.unpack(&pkg_dir)?;
            }
            let mut entry = p.locked();
            if keep_lock_entry && let Some(old) = lock.get(&p.name) {
                // An install from the lock keeps what the lock says of the source.
                let mut kept = old.clone();
                kept.variants = entry.variants;
                entry = kept;
            }
            lock.set(entry);
            lock.save(dir)?;
            changes.push(Change {
                previous: previous.get(&p.name).cloned(),
                name: p.name,
                version: p.offer.version.clone(),
                sha256: p.base_sha,
                registry: p.offer.registry.clone(),
                egg: p.egg,
            });
        }
        Ok(changes)
    }

    /// A registry package at the same version whose commit is no longer the
    /// locked one.
    fn check_moved(
        &self,
        lock: &Lock,
        resolver: &Resolver,
        name: &Name,
        pick: &Pick,
    ) -> Result<()> {
        if !resolver.source(pick.source).is_registry() {
            return Ok(());
        }
        if let Some(l) = lock.get(name)
            && l.version == pick.offer.version
            && let Some(locked) = &l.commit
            && let Some(now) = &pick.offer.commit
            && locked != now
        {
            return Err(Error::TagMoved {
                at: at_of(&pick.offer),
                tag: pick
                    .offer
                    .git_ref
                    .clone()
                    .unwrap_or_else(|| pick.offer.version.to_string()),
                locked: locked.clone(),
                now: now.clone(),
            });
        }
        Ok(())
    }

    /// Notes for installed versions that are now withdrawn.
    async fn withdrawn_notes(&self, resolver: &Resolver, lock: &Lock, plan: &Plan) -> Vec<String> {
        let mut notes = Vec::new();
        for (name, version) in &plan.kept {
            let Some(idx) = lock
                .get(name)
                .and_then(|l| l.registry.as_deref())
                .and_then(|label| self.registry_index(label))
            else {
                continue;
            };
            let Ok(offers) = resolver.listing(idx, name).await else {
                continue;
            };
            if let Some(w) = offers
                .iter()
                .find(|o| &o.version == version)
                .and_then(|o| o.withdrawn.as_ref())
            {
                notes.push(withdrawn_note(name, version, w));
            }
        }
        notes
    }

    /// Fetches and checks everything in the plan, then writes it.
    async fn apply(
        &self,
        dir: &Path,
        mut lock: Lock,
        resolver: &Resolver,
        plan: Plan,
    ) -> Result<Report> {
        let previous: BTreeMap<Name, Version> = lock
            .packages
            .iter()
            .map(|p| (p.name.clone(), p.version.clone()))
            .collect();
        for (name, pick) in &plan.picks {
            self.check_moved(&lock, resolver, name, pick)?;
        }
        let warnings = self.withdrawn_notes(resolver, &lock, &plan).await;
        let mut prepared = Vec::new();
        for pick in plan.picks.values() {
            let entry = resolver.source(pick.source);
            prepared.push(
                self.prepare(
                    &entry.source,
                    &entry.origin,
                    &pick.offer,
                    &pick.targets,
                    None,
                )
                .await?,
            );
        }
        let packages = self.write(dir, &mut lock, prepared, &previous, false)?;
        Ok(Report {
            dir: dir.to_path_buf(),
            packages,
            warnings,
        })
    }

    /// Every package the registries offer: about, versions, installed,
    /// updatable, held until.
    pub async fn offered(&self, dir: &Path) -> Result<Vec<Offered>> {
        let lock = Lock::load(dir)?;
        let installed = self.read_installed(dir, &lock)?;
        let resolver = self.new_resolver(Seen::load(dir)?, &installed);
        let now = self.now();

        let mut found: Vec<(usize, Name)> = Vec::new();
        let mut listed = BTreeSet::new();
        let mut errors = Vec::new();
        let mut reached = 0;
        for (idx, reg) in self.registries.iter().enumerate() {
            match reg.source.names().await {
                Ok(names) => {
                    reached += 1;
                    for n in names {
                        if reg.config.serves(&n.group) && listed.insert(n.clone()) {
                            found.push((idx, n));
                        }
                    }
                }
                Err(e) => errors.push(e),
            }
        }
        if reached == 0 && !errors.is_empty() {
            return Err(errors.remove(0));
        }

        let mut out = Vec::new();
        for (idx, name) in found {
            let Ok(offers) = resolver.listing(idx, &name).await else {
                continue;
            };
            if offers.is_empty() {
                continue;
            }
            let mut about = About::default();
            let mut versions = Vec::new();
            for (i, offer) in offers.iter().enumerate() {
                let mut problem = None;
                if i < CHECKED_VERSIONS {
                    match resolver.details(idx, offer).await {
                        Ok(d) => {
                            if i == 0 {
                                about = d.about.clone();
                            }
                            problem = resolver.check(offer, &d).err().map(|e| e.to_string());
                        }
                        Err(e) => problem = Some(e.to_string()),
                    }
                }
                versions.push(OfferedVersion {
                    version: offer.version.clone(),
                    published: offer.published,
                    withdrawn: offer.withdrawn.clone(),
                    held_until: resolver.held_until(idx, offer).filter(|u| *u > now),
                    problem,
                });
            }
            let installed_version = lock.get(&name).map(|l| l.version.clone());
            let updatable = installed_version.as_ref().and_then(|cur| {
                versions
                    .iter()
                    .take(CHECKED_VERSIONS)
                    .find(|v| {
                        v.withdrawn.is_none() && v.held_until.is_none() && v.problem.is_none()
                    })
                    .map(|v| v.version.clone())
                    .filter(|v| v > cur)
            });
            out.push(Offered {
                registry: self.registries[idx].source.label().to_string(),
                held_until: versions.first().and_then(|v| v.held_until),
                name,
                about,
                versions,
                installed: installed_version,
                updatable,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        // The records help later runs; a folder that cannot be written does not stop a listing.
        let _ = resolver.save_seen(dir);
        Ok(out)
    }

    /// Resolves, verifies, unpacks and locks a package and its dependencies.
    pub async fn add(&self, spec: &Spec, dir: &Path) -> Result<Report> {
        self.add_with(spec, dir, &Options::default()).await
    }

    pub async fn add_with(&self, spec: &Spec, dir: &Path, opts: &Options) -> Result<Report> {
        self.need_targets()?;
        let lock = Lock::load(dir)?;
        let installed = self.read_installed(dir, &lock)?;
        let mut resolver = self.new_resolver(Seen::load(dir)?, &installed);
        let (root, range) = self.root_of(&mut resolver, spec).await?;
        resolver.force(root.clone());
        if opts.allow_young {
            resolver.allow_young(root.clone());
        }
        let mut roots = Constraints::new();
        roots.insert(root, vec![("request".to_string(), range)]);
        let plan = resolver.resolve(roots).await;
        resolver.save_seen(dir)?;
        self.apply(dir, lock, &resolver, plan?).await
    }

    /// Updates one package, or all, to the newest allowed versions.
    pub async fn update(&self, name: Option<&Name>, dir: &Path) -> Result<Report> {
        self.need_targets()?;
        let lock = Lock::load(dir)?;
        let installed = self.read_installed(dir, &lock)?;
        let mut resolver = self.new_resolver(Seen::load(dir)?, &installed);

        let wanted: Vec<Name> = match name {
            Some(n) => {
                if lock.get(n).is_none() {
                    return Err(Error::NotFound { at: At::new(n) });
                }
                vec![n.clone()]
            }
            None => installed.iter().map(|i| i.name.clone()).collect(),
        };
        let mut notes = Vec::new();
        let mut targets: Vec<Name> = Vec::new();
        for n in wanted {
            let Some(lp) = lock.get(&n) else { continue };
            if lp.file.is_some() || lp.folder.is_some() {
                if name.is_some() {
                    notes.push(format!(
                        "{n} came from a file or folder and is left as it is"
                    ));
                }
                continue;
            }
            let idx = match self.lock_source(lp) {
                Ok(LockSource::Registry(i)) => i,
                Ok(LockSource::Single(source, origin)) => {
                    resolver.add_source(SourceEntry::single(source, origin))
                }
                Err(e) if name.is_none() => {
                    notes.push(e.to_string());
                    continue;
                }
                Err(e) => return Err(e),
            };
            resolver.pin(n.clone(), idx);
            if resolver.locate(&n).await?.is_none() {
                if name.is_some() {
                    return Err(Error::NotFound {
                        at: At::new(&n).registry(resolver.source(idx).source.label()),
                    });
                }
                notes.push(format!("{n} is no longer offered by its source"));
                continue;
            }
            resolver.force(n.clone());
            targets.push(n);
        }

        // What is not updated holds the ranges of the rest.
        let updating: BTreeSet<&Name> = targets.iter().collect();
        let mut roots = Constraints::new();
        for n in &targets {
            let mut reqs = Vec::new();
            for i in installed.iter().filter(|i| !updating.contains(&i.name)) {
                for (dep, range) in Client::installed_deps(i) {
                    if &dep == n {
                        reqs.push((i.name.to_string(), range));
                    }
                }
            }
            if reqs.is_empty() {
                reqs.push(("update".to_string(), VersionReq::STAR));
            }
            roots.insert(n.clone(), reqs);
        }
        if roots.is_empty() {
            resolver.save_seen(dir)?;
            return Ok(Report {
                dir: dir.to_path_buf(),
                packages: Vec::new(),
                warnings: notes,
            });
        }

        let plan = resolver.resolve(roots).await;
        resolver.save_seen(dir)?;
        let mut plan = plan?;
        // The same version at the same commit is no change. A moved tag of a
        // registry is refused when the plan is applied.
        plan.picks.retain(|n, pick| {
            !lock
                .get(n)
                .is_some_and(|l| l.version == pick.offer.version && l.commit == pick.offer.commit)
        });
        let mut report = self.apply(dir, lock, &resolver, plan).await?;
        notes.append(&mut report.warnings);
        report.warnings = notes;
        Ok(report)
    }

    /// Installs exactly what `eggs.lock` lists, checking checksums and
    /// commits. Packages already installed at the locked version are left.
    pub async fn install(&self, dir: &Path) -> Result<Report> {
        self.need_targets()?;
        let mut lock = Lock::load(dir)?;
        let installed = self.read_installed(dir, &lock)?;
        let previous: BTreeMap<Name, Version> = installed
            .iter()
            .map(|i| (i.name.clone(), i.version.clone()))
            .collect();
        let mut warnings = Vec::new();
        let mut prepared = Vec::new();
        for lp in &lock.packages {
            if previous.get(&lp.name) == Some(&lp.version) {
                continue;
            }
            let wanted: Vec<String> = self
                .targets
                .iter()
                .map(|t| t.name().to_string())
                .filter(|t| lp.targets.contains(t))
                .collect();
            if wanted.is_empty() {
                warnings.push(format!(
                    "{} is locked for {}, none of this client's targets",
                    lp.name,
                    lp.targets.join(", ")
                ));
                continue;
            }
            let (source, origin) = self.source_of(&self.lock_source(lp)?);
            let offer = self.lock_offer(&source, lp).await?;
            if let Some(w) = &offer.withdrawn {
                warnings.push(withdrawn_note(&lp.name, &lp.version, w));
            }
            prepared.push(
                self.prepare(&source, &origin, &offer, &wanted, Some(lp))
                    .await?,
            );
        }
        let packages = self.write(dir, &mut lock, prepared, &previous, true)?;
        Ok(Report {
            dir: dir.to_path_buf(),
            packages,
            warnings,
        })
    }

    /// Removes a package. Refuses while a dependent is installed.
    pub fn remove(&self, name: &Name, dir: &Path) -> Result<()> {
        let mut lock = Lock::load(dir)?;
        if lock.get(name).is_none() {
            return Err(Error::NotFound { at: At::new(name) });
        }
        let mut dependents = Vec::new();
        for i in self.read_installed(dir, &lock)? {
            if &i.name == name {
                continue;
            }
            for t in i
                .manifest
                .targets
                .iter()
                .filter(|t| i.targets.contains(&t.name))
            {
                for d in &t.dependencies {
                    if &d.name(&i.manifest.package.group) == name {
                        dependents.push(format!("{} {}", i.name, d.raw));
                    }
                }
            }
        }
        if !dependents.is_empty() {
            dependents.dedup();
            return Err(Error::RequiredBy {
                at: At::new(name),
                dependents,
            });
        }
        let pkg_dir = dir.join(name.dir());
        match fs::remove_dir_all(&pkg_dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).at_path(&pkg_dir),
        }
        lock.remove(name);
        lock.save(dir)
    }

    pub fn installed(&self, dir: &Path) -> Result<Vec<Installed>> {
        let lock = Lock::load(dir)?;
        self.read_installed(dir, &lock)
    }

    /// The files of one variant. Downloads and verifies its egg on first use.
    pub async fn variant(&self, pkg: &Installed, platform: &Platform) -> Result<PathBuf> {
        let at = at_name(&pkg.name, &pkg.version);
        let no_variant = |target: &str| Error::NoVariant {
            at: at.clone(),
            target: target.to_string(),
            platform: platform.clone(),
        };
        let mut first_target = None;
        let mut found = None;
        for t in &self.targets {
            if !pkg.targets.iter().any(|n| n == t.name()) {
                continue;
            }
            first_target.get_or_insert(t.name().to_string());
            if let Some(spec) = pkg.manifest.target(t.name())
                && !spec.variants.is_empty()
            {
                found = Some((t.name().to_string(), spec));
                break;
            }
        }
        let (tname, spec) =
            found.ok_or_else(|| no_variant(first_target.as_deref().unwrap_or("")))?;
        let key = spec
            .select_variant(platform)
            .ok_or_else(|| no_variant(&tname))?
            .key();
        let id = format!("{tname}.{key}");
        let folder = pkg.dir.join(&tname).join("variants").join(&key);

        let dir = pkg
            .dir
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| Error::invalid(pkg.dir.display(), "not a package folder"))?;
        let mut lock = Lock::load(dir)?;
        let mut lp = lock
            .get(&pkg.name)
            .cloned()
            .ok_or_else(|| Error::NotFound { at: at.clone() })?;
        if folder.is_dir() && lp.variants.contains_key(&id) {
            return Ok(folder);
        }

        let (source, _) = self.source_of(&self.lock_source(&lp)?);
        let offer = self.lock_offer(&source, &lp).await?;
        let from_fat = if lp.file.is_some() {
            source.fetch(&offer).await?.embedded_variant(&tname, &key)?
        } else {
            None
        };
        let egg = match from_fat {
            Some(e) => e,
            None => source.fetch_variant(&offer, &tname, &key).await?,
        };
        check_variant(&offer, &tname, &key, &egg)?;
        egg.unpack(&pkg.dir)?;
        lp.variants.insert(id, egg.sha256().to_string());
        lock.set(lp);
        lock.save(dir)?;
        Ok(folder)
    }

    /// Fetches a package's eggs without installing: the base egg, and the
    /// variant eggs named as `<target>.<key>` (all when None).
    pub async fn fetch(
        &self,
        spec: &Spec,
        variants: Option<&[String]>,
        opts: &Options,
    ) -> Result<Fetched> {
        let mut resolver = self.new_resolver(Seen::default(), &[]);
        let (name, range) = self.root_of(&mut resolver, spec).await?;
        if opts.allow_young {
            resolver.allow_young(name.clone());
        }
        let (idx, offer) = resolver.newest(&name, &range).await?;
        let source = resolver.source(idx).source.clone();
        let egg = source.fetch(&offer).await?;
        let base = egg.base()?;

        let wanted: Vec<(String, String)> = match variants {
            Some(list) => {
                let mut out = Vec::new();
                for s in list {
                    let (t, k) = split_variant(s)?;
                    out.push((t.to_string(), k.to_string()));
                }
                out
            }
            None => {
                let mut all = egg.embedded_variants();
                if let Ok(d) = resolver.details(idx, &offer).await {
                    for t in &d.targets {
                        for v in &t.variants {
                            let pair = (t.name.clone(), v.key());
                            if !all.contains(&pair) {
                                all.push(pair);
                            }
                        }
                    }
                }
                all
            }
        };
        let mut eggs = Vec::new();
        for (t, k) in wanted {
            let ve = match egg.embedded_variant(&t, &k)? {
                Some(e) => e,
                None => source.fetch_variant(&offer, &t, &k).await?,
            };
            check_variant(&offer, &t, &k, &ve)?;
            eggs.push(ve);
        }
        Ok(Fetched {
            base,
            variants: eggs,
        })
    }
}
