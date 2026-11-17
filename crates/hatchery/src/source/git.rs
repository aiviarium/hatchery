//! A git repository with a `packages.kdl`, used as a registry.
//!
//! Versions are tags, matched against each package's tag pattern. Built
//! eggs are read from the branch `release.eggs` names, which holds a static
//! registry. A version without a built egg is offered from its tag's source.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use semver::Version;

use crate::egg::Egg;
use crate::error::{At, Error, IoContext, Result};
use crate::fetch::BoxFuture;
use crate::git::{
    Git, Remote, RepoCache, Scratch, checkout_package, join_path, manifest_at, pack_checkout,
};
use crate::manifest::Manifest;
use crate::name::Name;
use crate::publish::{Published, publish_dir};
use crate::repo::{self, RepoManifest, version_from_tag};
use crate::sign::{SigningKey, VerifyingKey};
use crate::source::{Details, Offer, RegistryFiles, Source, StaticRegistry, Withdrawn};
use crate::time::Timestamp;

/// A git repository used as a registry.
pub struct GitRegistry {
    /// The address git is given. It may carry credentials, so it is never
    /// shown or recorded.
    url: String,
    /// The address without credentials: the label of the registry.
    label: String,
    cache: PathBuf,
    git: Git,
    key: Option<VerifyingKey>,
    unsigned_tags: bool,
    root: Mutex<Option<Arc<Root>>>,
    /// The details of a package at a commit.
    details: Mutex<HashMap<(String, Name), Details>>,
    /// Where a package is at a commit, and its manifest. None when the
    /// commit has no such package.
    found: Mutex<HashMap<(String, Name), Option<Found>>>,
}

/// A package's folder at a commit, and its manifest there.
type Found = (String, Manifest);

/// What `packages.kdl` of the default branch says, at one commit.
struct Root {
    commit: String,
    manifest: RepoManifest,
    packages: Vec<Pkg>,
    /// Package folders whose `package.kdl` does not parse.
    broken: Vec<Broken>,
}

struct Pkg {
    name: Name,
    path: String,
    tag: String,
}

struct Broken {
    path: String,
    what: String,
    message: String,
}

impl Broken {
    fn error(&self) -> Error {
        Error::invalid(&self.what, &self.message)
    }
}

impl GitRegistry {
    /// `cache` is the folder for this crate's git caches; each repository
    /// gets its own folder in it. With a `key`, built eggs must be signed by
    /// it, and versions that have only a tag are not installed unless
    /// [`GitRegistry::unsigned_tags`] allows them.
    pub fn new(
        url: impl Into<String>,
        cache: impl Into<PathBuf>,
        git: Git,
        key: Option<VerifyingKey>,
    ) -> GitRegistry {
        let url = url.into();
        GitRegistry {
            label: crate::fsx::redact_url(&url),
            url,
            cache: cache.into(),
            git,
            key,
            unsigned_tags: false,
            root: Mutex::new(None),
            details: Mutex::new(HashMap::new()),
            found: Mutex::new(HashMap::new()),
        }
    }

    /// Whether a registry with a key still installs versions that have only
    /// a tag, from source and unsigned. False by default. It is the
    /// `unsigned-tags` property of a registry line. Without a key it changes
    /// nothing.
    #[must_use]
    pub fn unsigned_tags(mut self, allow: bool) -> GitRegistry {
        self.unsigned_tags = allow;
        self
    }

    /// Whether a version that has only a tag is refused.
    fn tags_need_signature(&self) -> bool {
        self.key.is_some() && !self.unsigned_tags
    }

    /// The address without credentials, for messages.
    fn shown(&self) -> String {
        self.label.clone()
    }

    fn at(&self, name: &Name, version: impl std::fmt::Display) -> At {
        At::new(name).version(version).registry(self.shown())
    }

    /// The root manifest and the packages of the default branch. Reads the
    /// repository only when the branch has a new commit. A package whose
    /// `package.kdl` does not parse is set aside, so that one bad folder
    /// does not hide the others.
    fn root(&self, remote: &Remote) -> Result<Arc<Root>> {
        let branch = remote.default_branch.as_deref().ok_or_else(|| {
            Error::invalid(
                self.shown(),
                "has no default branch, so it has no packages.kdl",
            )
        })?;
        let tip = remote.branches.get(branch).ok_or_else(|| {
            Error::invalid(
                self.shown(),
                format!("default branch {branch} has no commit"),
            )
        })?;
        if let Some(r) = self.root.lock().unwrap().as_ref()
            && r.commit == *tip
        {
            return Ok(r.clone());
        }

        let cache = self.git.cache(&self.cache, &self.url)?;
        let commit = if cache.has_commit(tip) {
            tip.clone()
        } else {
            cache.fetch_branch(branch)?
        };
        let text = cache.show(&commit, repo::FILE)?.ok_or_else(|| {
            Error::invalid(
                self.shown(),
                format!(
                    "has no {} on branch {branch}; a repository used as a registry needs one",
                    repo::FILE
                ),
            )
        })?;
        let manifest =
            RepoManifest::parse(&String::from_utf8_lossy(&text)).map_err(|e| match e {
                Error::Invalid { message, .. } => {
                    Error::invalid(format!("{} of {}", repo::FILE, self.shown()), message)
                }
                e => e,
            })?;
        let dirs = cache.manifest_dirs(&commit)?;
        let mut packages = Vec::new();
        let mut broken = Vec::new();
        for p in manifest.package_dirs(&dirs) {
            match manifest_at(&cache, &commit, Some(&p.path)) {
                Ok((m, _)) => packages.push(Pkg {
                    name: m.name(),
                    path: p.path,
                    tag: p.tag,
                }),
                Err(Error::Invalid { what, message }) => broken.push(Broken {
                    path: p.path,
                    what,
                    message,
                }),
                Err(e) => return Err(e),
            }
        }
        drop(cache);
        let root = Arc::new(Root {
            commit,
            manifest,
            packages,
            broken,
        });
        *self.root.lock().unwrap() = Some(root.clone());
        Ok(root)
    }

    /// The static registry on the eggs branch, when the repository has one.
    fn eggs(&self, remote: &Remote, root: &Root) -> Result<Option<StaticRegistry>> {
        let Some(eggs) = &root.manifest.release.eggs else {
            return Ok(None);
        };
        let Some(tip) = remote.branches.get(&eggs.branch) else {
            return Ok(None);
        };
        let commit = {
            let cache = self.git.cache(&self.cache, &self.url)?;
            if cache.has_commit(tip) {
                tip.clone()
            } else {
                cache.fetch_branch(&eggs.branch)?
            }
        };
        let files = BranchFiles {
            url: self.url.clone(),
            label: self.label.clone(),
            cache: self.cache.clone(),
            git: self.git.clone(),
            commit,
            base: eggs.path.clone(),
        };
        Ok(Some(StaticRegistry::new(Arc::new(files), self.key)))
    }

    fn package<'a>(root: &'a Root, name: &Name) -> Option<&'a Pkg> {
        root.packages.iter().find(|p| p.name == *name)
    }

    /// The error of a set-aside package folder that looks like the one
    /// asked for: its folder has the package's id as its name.
    fn broken_for<'a>(root: &'a Root, name: &Name) -> Option<&'a Broken> {
        root.broken
            .iter()
            .find(|b| b.path.rsplit('/').next() == Some(name.id.as_str()))
    }

    fn moved(&self, offer: &Offer, tag: &str, locked: &str, now: Option<&String>) -> Error {
        Error::TagMoved {
            at: self.at(&offer.name, &offer.version),
            tag: tag.to_string(),
            locked: locked.to_string(),
            now: now.cloned().unwrap_or_else(|| "(no such tag)".into()),
        }
    }

    /// A built offer that came with a tag must still have the tag where it
    /// was.
    fn check_tag(&self, offer: &Offer) -> Result<()> {
        let (Some(tag), Some(commit)) = (&offer.git_ref, &offer.commit) else {
            return Ok(());
        };
        let remote = self.git.ls_remote(&self.url)?;
        match remote.tags.get(tag) {
            Some(now) if now == commit => Ok(()),
            other => Err(self.moved(offer, tag, commit, other)),
        }
    }

    /// Makes sure the cache has the commit of a tag, and that the tag still
    /// points at it.
    fn ensure_tag(&self, cache: &RepoCache, offer: &Offer, tag: &str, commit: &str) -> Result<()> {
        if cache.has_commit(commit) {
            return Ok(());
        }
        let got = cache.fetch_tag(tag)?;
        if got != commit {
            return Err(self.moved(offer, tag, commit, Some(&got)));
        }
        Ok(())
    }

    /// The folder and the manifest of a package at a commit the cache has.
    /// The folder `hint` is tried first; otherwise every package folder of
    /// the commit's own `packages.kdl` is read. None when the commit has no
    /// package of that name. A package can move between releases, so the
    /// default branch does not say where it was at a tag.
    fn find_at(
        &self,
        cache: &RepoCache,
        commit: &str,
        name: &Name,
        hint: Option<&str>,
    ) -> Result<Option<(String, Manifest)>> {
        let key = (commit.to_string(), name.clone());
        if let Some(found) = self.found.lock().unwrap().get(&key) {
            return Ok(found.clone());
        }
        let named = |path: &str| -> Result<Option<Manifest>> {
            match manifest_at(cache, commit, Some(path)) {
                Ok((m, _)) if m.name() == *name => Ok(Some(m)),
                Ok(_) | Err(Error::Invalid { .. }) => Ok(None),
                Err(e) => Err(e),
            }
        };
        let mut found = None;
        if let Some(h) = hint {
            found = named(h)?.map(|m| (h.to_string(), m));
        }
        if found.is_none()
            && let Some(text) = cache.show(commit, repo::FILE)?
            && let Ok(manifest) = RepoManifest::parse(&String::from_utf8_lossy(&text))
        {
            for p in manifest.package_dirs(&cache.manifest_dirs(commit)?) {
                if Some(p.path.as_str()) == hint {
                    continue;
                }
                if let Some(m) = named(&p.path)? {
                    found = Some((p.path, m));
                    break;
                }
            }
        }
        self.found.lock().unwrap().insert(key, found.clone());
        Ok(found)
    }

    /// Where the package of an offer from a tag is at its commit, and its
    /// manifest, checked against the offer's version.
    fn locate(&self, cache: &RepoCache, offer: &Offer) -> Result<(String, Manifest)> {
        let (Some(tag), Some(commit)) = (&offer.git_ref, &offer.commit) else {
            return Err(Error::other(format!(
                "{} {}: the offer has no tag or commit",
                offer.name, offer.version
            )));
        };
        self.ensure_tag(cache, offer, tag, commit)?;
        let (path, m) = self
            .find_at(cache, commit, &offer.name, offer.path.as_deref())?
            .ok_or_else(|| {
                Error::invalid(
                    self.at(&offer.name, &offer.version),
                    format!("tag {tag} has no package {}", offer.name),
                )
            })?;
        if m.package.version != offer.version {
            return Err(Error::invalid(
                join_path(Some(&path), crate::manifest::FILE),
                format!(
                    "tag {tag} is version {} of {}, but package.kdl at that commit says {} {}",
                    offer.version,
                    offer.name,
                    m.name(),
                    m.package.version
                ),
            ));
        }
        Ok((path, m))
    }

    fn fetch_source(&self, offer: &Offer, variant: Option<(&str, &str)>) -> Result<Egg> {
        let (Some(tag), Some(commit)) = (&offer.git_ref, &offer.commit) else {
            return Err(Error::other(format!(
                "{} {}: the offer has no tag or commit",
                offer.name, offer.version
            )));
        };
        if self.tags_need_signature() {
            return Err(Error::SignatureInvalid {
                at: self.at(&offer.name, &offer.version),
                reason: crate::source::TAG_ONLY.to_string(),
            });
        }
        let remote = self.git.ls_remote(&self.url)?;
        match remote.tags.get(tag) {
            Some(now) if now == commit => {}
            other => return Err(self.moved(offer, tag, commit, other)),
        }
        let (checkout, path) = {
            let cache = self.git.cache(&self.cache, &self.url)?;
            let got = cache.fetch_tag(tag)?;
            if got != *commit {
                return Err(self.moved(offer, tag, commit, Some(&got)));
            }
            let (path, _) = self.locate(&cache, offer)?;
            (checkout_package(&cache, commit, Some(&path))?, path)
        };
        let egg = pack_checkout(&checkout, &self.url, commit, variant)?;
        let m = egg.manifest();
        if m.name() != offer.name || m.package.version != offer.version {
            return Err(Error::invalid(
                join_path(Some(&path), crate::manifest::FILE),
                format!(
                    "tag {tag} is {} {}, but the package at that commit is {} {}",
                    offer.name,
                    offer.version,
                    m.name(),
                    m.package.version
                ),
            ));
        }
        Ok(egg)
    }

    /// Of tags that share one pattern among several packages, the ones at
    /// which this package has the tag's version. A tag `v2.0.0` made for
    /// one package says nothing of another.
    fn own_tags(
        &self,
        name: &Name,
        pkg: &Pkg,
        tags: &mut Vec<(String, String, Version)>,
    ) -> Result<()> {
        if tags.is_empty() {
            return Ok(());
        }
        let cache = self.git.cache(&self.cache, &self.url)?;
        let missing: Vec<&str> = tags
            .iter()
            .filter(|(_, c, _)| {
                !self
                    .found
                    .lock()
                    .unwrap()
                    .contains_key(&(c.clone(), name.clone()))
                    && !cache.has_commit(c)
            })
            .map(|(t, _, _)| t.as_str())
            .collect();
        cache.fetch_tags(&missing)?;
        let mut kept = Vec::new();
        for (tag, commit, version) in tags.drain(..) {
            // A tag that moved since the listing is left for the fetch to refuse.
            if !cache.has_commit(&commit) {
                kept.push((tag, commit, version));
                continue;
            }
            if let Some((_, m)) = self.find_at(&cache, &commit, name, Some(&pkg.path))?
                && m.package.version == version
            {
                kept.push((tag, commit, version));
            }
        }
        *tags = kept;
        Ok(())
    }
}

impl Source for GitRegistry {
    fn label(&self) -> &str {
        &self.label
    }

    fn is_local(&self) -> bool {
        crate::config::is_local_location(&self.url)
    }

    fn names(&self) -> BoxFuture<'_, Result<Vec<Name>>> {
        Box::pin(async move {
            let remote = self.git.ls_remote(&self.url)?;
            let root = self.root(&remote)?;
            Ok(root.packages.iter().map(|p| p.name.clone()).collect())
        })
    }

    fn warnings(&self) -> Vec<String> {
        let root = self.root.lock().unwrap().clone();
        root.map(|r| {
            r.broken
                .iter()
                .map(|b| format!("{}: skipped {}: {}", self.label, b.path, b.error()))
                .collect()
        })
        .unwrap_or_default()
    }

    fn versions<'a>(&'a self, name: &'a Name) -> BoxFuture<'a, Result<Vec<Offer>>> {
        Box::pin(async move {
            let remote = self.git.ls_remote(&self.url)?;
            let root = self.root(&remote)?;
            let pkg = Self::package(&root, name);
            if pkg.is_none()
                && let Some(b) = Self::broken_for(&root, name)
            {
                return Err(b.error());
            }

            let mut built: BTreeMap<Version, Offer> = BTreeMap::new();
            if let Some(eggs) = self.eggs(&remote, &root)? {
                for o in eggs.versions(name).await? {
                    built.insert(o.version.clone(), o);
                }
            }
            // A package that left the default branch keeps its built eggs.
            let Some(pkg) = pkg else {
                let mut out: Vec<Offer> = built.into_values().collect();
                for o in &mut out {
                    o.registry = self.label.clone();
                }
                self.mark_withdrawn(&root, name, &mut out);
                return Ok(out);
            };

            let mut tags: Vec<(String, String, Version)> = Vec::new();
            for (tag, commit) in &remote.tags {
                if let Some(version) = version_from_tag(&pkg.tag, &name.id, tag) {
                    tags.push((tag.clone(), commit.clone(), version));
                }
            }
            if !pkg.tag.contains("{id}") {
                let (mut alone, with_egg): (Vec<_>, Vec<_>) = tags
                    .into_iter()
                    .partition(|(_, _, v)| !built.contains_key(v));
                self.own_tags(name, pkg, &mut alone)?;
                tags = with_egg;
                tags.extend(alone);
            }

            let mut out = Vec::new();
            for (tag, commit, version) in tags {
                let mut offer = match built.remove(&version) {
                    Some(o) => o,
                    None => {
                        let mut o = Offer::new(name.clone(), version.clone(), &self.label);
                        o.built = false;
                        o.needs_signature = self.tags_need_signature();
                        o
                    }
                };
                // Where the package is at the tag, when that was looked up.
                let path = self
                    .found
                    .lock()
                    .unwrap()
                    .get(&(commit.clone(), name.clone()))
                    .cloned()
                    .flatten()
                    .map(|(p, _)| p);
                offer.registry = self.label.clone();
                offer.commit = Some(commit);
                offer.git_ref = Some(tag);
                offer.path = Some(path.unwrap_or_else(|| pkg.path.clone()));
                out.push(offer);
            }
            // Built eggs whose tag is gone or was never made.
            for (_, mut o) in built {
                o.registry = self.label.clone();
                o.path = Some(pkg.path.clone());
                out.push(o);
            }
            self.mark_withdrawn(&root, name, &mut out);
            Ok(out)
        })
    }

    fn details<'a>(&'a self, offer: &'a Offer) -> BoxFuture<'a, Result<Details>> {
        Box::pin(async move {
            if let Some(d) = &offer.details {
                return Ok(d.clone());
            }
            let Some(commit) = &offer.commit else {
                return Err(Error::other(format!(
                    "{} {}: no details from {}",
                    offer.name,
                    offer.version,
                    self.shown()
                )));
            };
            let key = (commit.clone(), offer.name.clone());
            if let Some(d) = self.details.lock().unwrap().get(&key) {
                return Ok(d.clone());
            }
            let manifest = {
                let cache = self.git.cache(&self.cache, &self.url)?;
                self.locate(&cache, offer)?.1
            };
            let d = Details::from_manifest(&manifest);
            self.details.lock().unwrap().insert(key, d.clone());
            Ok(d)
        })
    }

    fn fetch<'a>(&'a self, offer: &'a Offer) -> BoxFuture<'a, Result<Egg>> {
        Box::pin(async move {
            if offer.built {
                self.check_tag(offer)?;
                let eggs = self.built_registry(offer)?;
                return eggs.fetch(offer).await;
            }
            self.fetch_source(offer, None)
        })
    }

    fn fetch_variant<'a>(
        &'a self,
        offer: &'a Offer,
        target: &'a str,
        key: &'a str,
    ) -> BoxFuture<'a, Result<Egg>> {
        Box::pin(async move {
            if offer.built {
                self.check_tag(offer)?;
                let eggs = self.built_registry(offer)?;
                return eggs.fetch_variant(offer, target, key).await;
            }
            self.fetch_source(offer, Some((target, key)))
        })
    }
}

impl GitRegistry {
    fn mark_withdrawn(&self, root: &Root, name: &Name, offers: &mut [Offer]) {
        for o in offers {
            if let Some(w) = root.manifest.withdrawn(&name.group, &name.id, &o.version) {
                o.withdrawn = Some(Withdrawn {
                    reason: w.reason.clone(),
                });
            }
        }
    }

    fn built_registry(&self, offer: &Offer) -> Result<StaticRegistry> {
        let remote = self.git.ls_remote(&self.url)?;
        let root = self.root(&remote)?;
        self.eggs(&remote, &root)?.ok_or_else(|| Error::NotFound {
            at: self.at(&offer.name, &offer.version),
        })
    }
}

/// The files of a static registry on a branch of a git repository, read
/// from one commit of the cache.
struct BranchFiles {
    url: String,
    label: String,
    cache: PathBuf,
    git: Git,
    commit: String,
    base: Option<String>,
}

impl RegistryFiles for BranchFiles {
    fn label(&self) -> &str {
        &self.label
    }

    fn is_local(&self) -> bool {
        crate::config::is_local_location(&self.url)
    }

    fn read<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<Option<Bytes>>> {
        Box::pin(async move {
            let file = join_path(self.base.as_deref(), path);
            let cache = self.git.cache(&self.cache, &self.url)?;
            Ok(cache.show(&self.commit, &file)?.map(Bytes::from))
        })
    }
}

/// Publishes an egg into the static registry on a branch of a repository
/// and pushes it. The branch is created when it does not exist. When the
/// push is rejected because someone else pushed first (parallel CI jobs
/// publish variants at the same time), the branch is fetched again and the
/// egg published on top of it, up to 5 tries.
pub fn publish_branch(
    egg: &Egg,
    url: &str,
    branch: &str,
    path: Option<&str>,
    key: Option<&SigningKey>,
    now: Timestamp,
    git: &Git,
) -> Result<Published> {
    const TRIES: usize = 5;
    if let Some(p) = path {
        crate::manifest::validate_rel_path("path", p)?;
    }
    let mut last = None;
    for _ in 0..TRIES {
        match publish_once(egg, url, branch, path, key, now, git) {
            Err(e @ Error::Git { .. }) if is_rejected(&e) => last = Some(e),
            other => return other,
        }
    }
    Err(last.unwrap_or_else(|| Error::other("publishing gave up")))
}

fn is_rejected(e: &Error) -> bool {
    match e {
        Error::Git { args, message } => {
            args.starts_with("push")
                && (message.contains("rejected")
                    || message.contains("non-fast-forward")
                    || message.contains("fetch first")
                    || message.contains("stale info"))
        }
        _ => false,
    }
}

fn publish_once(
    egg: &Egg,
    url: &str,
    branch: &str,
    path: Option<&str>,
    key: Option<&SigningKey>,
    now: Timestamp,
    git: &Git,
) -> Result<Published> {
    crate::git::check_url(url)?;
    crate::git::check_ref(branch)?;
    let scratch = Scratch::new()?;
    let repo = scratch.path().join("repo");
    std::fs::create_dir_all(&repo).at_path(&repo)?;
    let head = format!("refs/heads/{branch}");

    let remote = git.ls_remote(url)?;
    git.run(Some(&repo), &["init", "--quiet"])?;
    git.run(Some(&repo), &["remote", "add", "origin", "--", url])?;
    if remote.branches.contains_key(branch) {
        let spec = format!("+{head}:refs/remotes/origin/{branch}");
        git.exec(
            Some(&repo),
            &["protocol.version=2"],
            &[
                "fetch",
                "--quiet",
                "--depth",
                "1",
                "--no-tags",
                "origin",
                &spec,
            ],
            &[],
        )?;
        git.run(
            Some(&repo),
            &[
                "checkout",
                "--quiet",
                "--force",
                "-B",
                branch,
                &format!("refs/remotes/origin/{branch}"),
            ],
        )?;
    } else {
        git.run(Some(&repo), &["symbolic-ref", "HEAD", &head])?;
    }

    let dir = match path {
        Some(p) => repo.join(p),
        None => repo.clone(),
    };
    std::fs::create_dir_all(&dir).at_path(&dir)?;
    let published = publish_dir(egg, &dir, key, now)?;

    git.run(Some(&repo), &["add", "--all"])?;
    if git
        .run_text(Some(&repo), &["status", "--porcelain"])?
        .is_empty()
    {
        // The same egg was published before.
        return Ok(published);
    }
    let mut message = format!(
        "Publish {} {}",
        egg.manifest().name(),
        egg.manifest().package.version
    );
    if let Some(v) = egg.variant() {
        message.push_str(&format!(" {}.{}", v.target, v.key()));
    }
    let mut config = vec!["commit.gpgsign=false"];
    let configured = |k: &str| {
        git.run_text(Some(&repo), &["config", k])
            .is_ok_and(|v| !v.is_empty())
    };
    if !configured("user.email") {
        config.push("user.email=hatchery@localhost");
    }
    if !configured("user.name") {
        config.push("user.name=Hatchery");
    }
    git.exec(
        Some(&repo),
        &config,
        &["commit", "--quiet", "--message", &message],
        &[],
    )?;
    git.run(
        Some(&repo),
        &["push", "--quiet", "origin", &format!("HEAD:{head}")],
    )?;
    Ok(published)
}
