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
use crate::git::{Git, Remote, Scratch, checkout_package, join_path, manifest_at, pack_checkout};
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
    root: Mutex<Option<Arc<Root>>>,
    details: Mutex<HashMap<(String, String), Details>>,
}

/// What `packages.kdl` of the default branch says, at one commit.
struct Root {
    commit: String,
    manifest: RepoManifest,
    packages: Vec<Pkg>,
}

struct Pkg {
    name: Name,
    path: String,
    tag: String,
}

impl GitRegistry {
    /// `cache` is the folder for this crate's git caches; each repository
    /// gets its own folder in it. `key` is required of built eggs.
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
            root: Mutex::new(None),
            details: Mutex::new(HashMap::new()),
        }
    }

    /// The address without credentials, for messages.
    fn shown(&self) -> String {
        self.label.clone()
    }

    fn at(&self, name: &Name, version: impl std::fmt::Display) -> At {
        At::new(name).version(version).registry(self.shown())
    }

    /// The root manifest and the packages of the default branch. Reads the
    /// repository only when the branch has a new commit.
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
        for p in manifest.package_dirs(&dirs) {
            let (m, _) = manifest_at(&cache, &commit, Some(&p.path))?;
            packages.push(Pkg {
                name: m.name(),
                path: p.path,
                tag: p.tag,
            });
        }
        drop(cache);
        let root = Arc::new(Root {
            commit,
            manifest,
            packages,
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

    /// A built offer that came with a tag must still have the tag where it
    /// was.
    fn check_tag(&self, offer: &Offer) -> Result<()> {
        let (Some(tag), Some(commit)) = (&offer.git_ref, &offer.commit) else {
            return Ok(());
        };
        let remote = self.git.ls_remote(&self.url)?;
        match remote.tags.get(tag) {
            Some(now) if now == commit => Ok(()),
            other => Err(Error::TagMoved {
                at: self.at(&offer.name, &offer.version),
                tag: tag.clone(),
                locked: commit.clone(),
                now: other.cloned().unwrap_or_else(|| "(no such tag)".into()),
            }),
        }
    }

    fn fetch_source(&self, offer: &Offer, variant: Option<(&str, &str)>) -> Result<Egg> {
        let (Some(tag), Some(commit), Some(path)) = (&offer.git_ref, &offer.commit, &offer.path)
        else {
            return Err(Error::other(format!(
                "{} {}: the offer has no tag, commit or folder",
                offer.name, offer.version
            )));
        };
        let remote = self.git.ls_remote(&self.url)?;
        match remote.tags.get(tag) {
            Some(now) if now == commit => {}
            other => {
                return Err(Error::TagMoved {
                    at: self.at(&offer.name, &offer.version),
                    tag: tag.clone(),
                    locked: commit.clone(),
                    now: other.cloned().unwrap_or_else(|| "(no such tag)".into()),
                });
            }
        }
        let checkout = {
            let cache = self.git.cache(&self.cache, &self.url)?;
            let got = cache.fetch_tag(tag)?;
            if got != *commit {
                return Err(Error::TagMoved {
                    at: self.at(&offer.name, &offer.version),
                    tag: tag.clone(),
                    locked: commit.clone(),
                    now: got,
                });
            }
            checkout_package(&cache, commit, Some(path))?
        };
        let egg = pack_checkout(&checkout, &self.url, commit, variant)?;
        let m = egg.manifest();
        if m.name() != offer.name || m.package.version != offer.version {
            return Err(Error::invalid(
                join_path(Some(path), crate::manifest::FILE),
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

    fn versions<'a>(&'a self, name: &'a Name) -> BoxFuture<'a, Result<Vec<Offer>>> {
        Box::pin(async move {
            let remote = self.git.ls_remote(&self.url)?;
            let root = self.root(&remote)?;
            let Some(pkg) = Self::package(&root, name) else {
                return Ok(Vec::new());
            };

            let mut built: BTreeMap<Version, Offer> = BTreeMap::new();
            if let Some(eggs) = self.eggs(&remote, &root)? {
                for o in eggs.versions(name).await? {
                    built.insert(o.version.clone(), o);
                }
            }

            let mut out = Vec::new();
            for (tag, commit) in &remote.tags {
                let Some(version) = version_from_tag(&pkg.tag, &name.id, tag) else {
                    continue;
                };
                let mut offer = match built.remove(&version) {
                    Some(o) => o,
                    None => {
                        let mut o = Offer::new(name.clone(), version.clone(), &self.label);
                        o.built = false;
                        o
                    }
                };
                offer.registry = self.label.clone();
                offer.commit = Some(commit.clone());
                offer.git_ref = Some(tag.clone());
                offer.path = Some(pkg.path.clone());
                out.push(offer);
            }
            // Built eggs whose tag is gone or was never made.
            for (_, mut o) in built {
                o.registry = self.label.clone();
                o.path = Some(pkg.path.clone());
                out.push(o);
            }
            for o in &mut out {
                if let Some(w) = root.manifest.withdrawn(&name.group, &name.id, &o.version) {
                    o.withdrawn = Some(Withdrawn {
                        reason: w.reason.clone(),
                    });
                }
            }
            Ok(out)
        })
    }

    fn details<'a>(&'a self, offer: &'a Offer) -> BoxFuture<'a, Result<Details>> {
        Box::pin(async move {
            if let Some(d) = &offer.details {
                return Ok(d.clone());
            }
            let (Some(tag), Some(commit), Some(path)) =
                (&offer.git_ref, &offer.commit, &offer.path)
            else {
                return Err(Error::other(format!(
                    "{} {}: no details from {}",
                    offer.name,
                    offer.version,
                    self.shown()
                )));
            };
            let key = (commit.clone(), path.clone());
            if let Some(d) = self.details.lock().unwrap().get(&key) {
                return Ok(d.clone());
            }
            let manifest = {
                let cache = self.git.cache(&self.cache, &self.url)?;
                if !cache.has_commit(commit) {
                    let got = cache.fetch_tag(tag)?;
                    if got != *commit {
                        return Err(Error::TagMoved {
                            at: self.at(&offer.name, &offer.version),
                            tag: tag.clone(),
                            locked: commit.clone(),
                            now: got,
                        });
                    }
                }
                manifest_at(&cache, commit, Some(path))?.0
            };
            if manifest.name() != offer.name || manifest.package.version != offer.version {
                return Err(Error::invalid(
                    join_path(Some(path), crate::manifest::FILE),
                    format!(
                        "tag {tag} is version {} of {}, but package.kdl at that commit says {} {}",
                        offer.version,
                        offer.name,
                        manifest.name(),
                        manifest.package.version
                    ),
                ));
            }
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
