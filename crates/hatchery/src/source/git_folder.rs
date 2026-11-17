//! One folder of a git repository at one tag, branch or commit.

use std::path::PathBuf;

use crate::egg::Egg;
use crate::error::{At, Error, Result};
use crate::fetch::BoxFuture;
use crate::git::{Git, Remote, RepoCache, checkout_package, join_path, manifest_at, pack_checkout};
use crate::name::Name;
use crate::source::{Details, Offer, Source};

/// One package folder of a git repository at one ref. The repository needs
/// no `packages.kdl`; when it has one, its defaults apply.
pub struct GitFolder {
    url: String,
    path: Option<String>,
    git_ref: Option<String>,
    cache: PathBuf,
    git: Git,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Tag,
    Branch,
    Commit,
}

struct Resolved {
    commit: String,
    /// The ref as given, or the default branch.
    name: String,
    kind: Kind,
}

impl GitFolder {
    /// One package folder of the repository at `url`. `path` is the folder in the repository, the root when None, and `git_ref` is a tag, branch or commit, the default branch when None. `cache` is the folder for cached checkouts. Nothing is read until the source is used.
    pub fn new(
        url: impl Into<String>,
        path: Option<String>,
        git_ref: Option<String>,
        cache: impl Into<PathBuf>,
        git: Git,
    ) -> GitFolder {
        GitFolder {
            url: url.into(),
            path: path.map(|p| p.trim_end_matches('/').to_string()),
            git_ref,
            cache: cache.into(),
            git,
        }
    }

    fn shown(&self) -> String {
        crate::fsx::redact_url(&self.url)
    }

    fn resolve(&self, remote: &Remote) -> Result<Resolved> {
        let (name, kind, commit) = match &self.git_ref {
            None => {
                let b = remote.default_branch.clone().ok_or_else(|| {
                    Error::other(format!("{}: has no default branch", self.shown()))
                })?;
                let c = remote.branches.get(&b).cloned().ok_or_else(|| {
                    Error::other(format!(
                        "{}: default branch {b} has no commit",
                        self.shown()
                    ))
                })?;
                (b, Kind::Branch, c)
            }
            Some(r) => {
                if let Some(c) = remote.tags.get(r) {
                    (r.clone(), Kind::Tag, c.clone())
                } else if let Some(c) = remote.branches.get(r) {
                    (r.clone(), Kind::Branch, c.clone())
                } else if r.len() == 40 && r.bytes().all(|b| b.is_ascii_hexdigit()) {
                    (r.clone(), Kind::Commit, r.to_ascii_lowercase())
                } else {
                    return Err(Error::other(format!(
                        "{}: no tag, branch or commit {r}",
                        self.shown()
                    )));
                }
            }
        };
        Ok(Resolved { commit, name, kind })
    }

    /// Makes sure the cache has the commit, fetching it the way its ref
    /// allows. Returns the commit the fetch gave.
    fn ensure(&self, cache: &RepoCache, r: &Resolved) -> Result<String> {
        if cache.has_commit(&r.commit) {
            return Ok(r.commit.clone());
        }
        match r.kind {
            Kind::Tag => cache.fetch_tag(&r.name),
            Kind::Branch => cache.fetch_branch(&r.name),
            Kind::Commit => cache.fetch_commit(&r.commit).map(|()| r.commit.clone()),
        }
    }

    /// Whether the ref names a tag now, which the client uses to refuse a
    /// moved tag on update.
    pub fn ref_is_tag(&self) -> BoxFuture<'_, Result<bool>> {
        Box::pin(async move {
            let remote = self.git.ls_remote(&self.url)?;
            Ok(self.resolve(&remote)?.kind == Kind::Tag)
        })
    }

    fn offer(&self) -> Result<Offer> {
        let remote = self.git.ls_remote(&self.url)?;
        let r = self.resolve(&remote)?;
        let (m, commit) = {
            let cache = self.git.cache(&self.cache, &self.url)?;
            // The fetch may give a newer commit than ls-remote saw.
            let commit = self.ensure(&cache, &r)?;
            let (m, _) = manifest_at(&cache, &commit, self.path.as_deref())?;
            (m, commit)
        };
        let mut o = Offer::new(m.name(), m.package.version.clone(), &self.url);
        o.built = false;
        o.commit = Some(commit);
        o.git_ref = Some(r.name);
        o.path = self.path.clone();
        o.details = Some(Details::from_manifest(&m));
        Ok(o)
    }

    fn at(&self, offer: &Offer) -> At {
        At::new(&offer.name)
            .version(&offer.version)
            .registry(self.shown())
    }

    fn fetch_folder(&self, offer: &Offer, variant: Option<(&str, &str)>) -> Result<Egg> {
        let remote = self.git.ls_remote(&self.url)?;
        let commit = match &offer.commit {
            Some(c) => c.clone(),
            None => self.resolve(&remote)?.commit,
        };
        let ref_name = offer
            .git_ref
            .clone()
            .or_else(|| self.git_ref.clone())
            .unwrap_or_default();
        if let Some(now) = remote.tags.get(&ref_name)
            && *now != commit
        {
            return Err(Error::TagMoved {
                at: self.at(offer),
                tag: ref_name,
                locked: commit,
                now: now.clone(),
            });
        }
        let checkout = {
            let cache = self.git.cache(&self.cache, &self.url)?;
            if !cache.has_commit(&commit) {
                if remote.tags.contains_key(&ref_name) {
                    cache.fetch_tag(&ref_name)?;
                } else if let Err(e) = cache.fetch_commit(&commit) {
                    // A server that refuses a commit by its id may still
                    // have it as the tip of the branch.
                    let tip = remote.branches.get(&ref_name);
                    if tip == Some(&commit) {
                        cache.fetch_branch(&ref_name)?;
                    } else {
                        return Err(e);
                    }
                }
            }
            checkout_package(&cache, &commit, self.path.as_deref())?
        };
        let egg = pack_checkout(&checkout, &self.url, &commit, variant)?;
        let m = egg.manifest();
        if m.name() != offer.name || m.package.version != offer.version {
            return Err(Error::invalid(
                join_path(self.path.as_deref(), crate::manifest::FILE),
                format!(
                    "expected {} {}, but the package at {commit} is {} {}",
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

impl Source for GitFolder {
    fn label(&self) -> &str {
        &self.url
    }

    fn is_local(&self) -> bool {
        crate::config::is_local_location(&self.url)
    }

    fn names(&self) -> BoxFuture<'_, Result<Vec<Name>>> {
        Box::pin(async move { Ok(vec![self.offer()?.name]) })
    }

    fn versions<'a>(&'a self, name: &'a Name) -> BoxFuture<'a, Result<Vec<Offer>>> {
        Box::pin(async move {
            let o = self.offer()?;
            Ok(if o.name == *name { vec![o] } else { Vec::new() })
        })
    }

    fn fetch<'a>(&'a self, offer: &'a Offer) -> BoxFuture<'a, Result<Egg>> {
        Box::pin(async move { self.fetch_folder(offer, None) })
    }

    fn fetch_variant<'a>(
        &'a self,
        offer: &'a Offer,
        target: &'a str,
        key: &'a str,
    ) -> BoxFuture<'a, Result<Egg>> {
        Box::pin(async move { self.fetch_folder(offer, Some((target, key))) })
    }
}
