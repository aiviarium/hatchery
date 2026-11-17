//! A package folder, for development, and an egg file, by path or URL.
//! Neither is held by the minimum release age: you chose the exact one.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::egg::Egg;
use crate::error::{At, Error, IoContext, Result};
use crate::fetch::{BoxFuture, Fetch, Url};
use crate::manifest::Manifest;
use crate::name::Name;
use crate::pack::{PackOptions, VariantPick, pack};
use crate::source::{Details, Offer, Source};
use crate::target::Target;

/// A package folder, packed as it is, without a build.
pub struct FolderSource {
    dir: PathBuf,
    label: String,
    targets: Vec<Arc<dyn Target>>,
}

impl FolderSource {
    /// `targets` are asked to `derive` a manifest when the folder has no
    /// `package.kdl`.
    pub fn new(dir: impl Into<PathBuf>, targets: Vec<Arc<dyn Target>>) -> FolderSource {
        let dir = dir.into();
        let label = dir.display().to_string();
        FolderSource {
            dir,
            label,
            targets,
        }
    }

    /// Read on every call, so that edits to the folder show.
    fn manifest(&self) -> Result<Manifest> {
        if self.dir.join("package.kdl").is_file() {
            return Manifest::load(&self.dir);
        }
        for t in &self.targets {
            if let Some(m) = t.derive(&self.dir)? {
                return Ok(m);
            }
        }
        Err(Error::invalid(
            &self.label,
            "the folder has no package.kdl, and no target can derive a manifest from it",
        ))
    }

    fn check_offer(&self, m: &Manifest, offer: &Offer) -> Result<()> {
        if m.name() != offer.name || *m.version() != offer.version {
            return Err(Error::invalid(
                &self.label,
                format!(
                    "the folder is {} {}, not {} {}",
                    m.name(),
                    m.version(),
                    offer.name,
                    offer.version
                ),
            ));
        }
        Ok(())
    }
}

impl Source for FolderSource {
    fn label(&self) -> &str {
        &self.label
    }

    fn is_local(&self) -> bool {
        true
    }

    fn names(&self) -> BoxFuture<'_, Result<Vec<Name>>> {
        Box::pin(async move { Ok(vec![self.manifest()?.name()]) })
    }

    fn versions<'a>(&'a self, name: &'a Name) -> BoxFuture<'a, Result<Vec<Offer>>> {
        Box::pin(async move {
            let m = self.manifest()?;
            if m.name() != *name {
                return Ok(Vec::new());
            }
            let mut offer = Offer::new(m.name(), m.version().clone(), &self.label);
            offer.built = true;
            offer.details = Some(Details::from_manifest(&m));
            Ok(vec![offer])
        })
    }

    fn fetch<'a>(&'a self, offer: &'a Offer) -> BoxFuture<'a, Result<Egg>> {
        Box::pin(async move {
            self.check_offer(&self.manifest()?, offer)?;
            let opts = PackOptions {
                base: true,
                no_build: true,
                ..PackOptions::default()
            };
            pack_one(&self.dir, &opts, |egg| egg.variant().is_none())
        })
    }

    fn fetch_variant<'a>(
        &'a self,
        offer: &'a Offer,
        target: &'a str,
        key: &'a str,
    ) -> BoxFuture<'a, Result<Egg>> {
        Box::pin(async move {
            let m = self.manifest()?;
            self.check_offer(&m, offer)?;
            let at = || {
                At::new(&offer.name)
                    .version(format!("{} {target}.{key}", offer.version))
                    .registry(&self.label)
            };
            let spec = m
                .target(target)
                .and_then(|t| t.variant(key))
                .ok_or_else(|| Error::NotFound { at: at() })?;
            let opts = PackOptions {
                variants: vec![VariantPick {
                    target: target.to_string(),
                    os: spec.os.clone(),
                    arch: spec.arch.clone(),
                }],
                no_build: true,
                ..PackOptions::default()
            };
            pack_one(&self.dir, &opts, |egg| egg.variant().is_some())
        })
    }
}

fn pack_one(dir: &Path, opts: &PackOptions<'_>, wanted: impl Fn(&Egg) -> bool) -> Result<Egg> {
    pack(dir, opts)?
        .into_iter()
        .find(|e| wanted(e))
        .ok_or_else(|| Error::other(format!("{}: packing made no egg", dir.display())))
}

/// An egg file on disk or at an `http(s)://` address. A fat egg needs no
/// registry for its variants.
pub struct FileSource {
    location: String,
    fetch: Option<Arc<dyn Fetch>>,
    egg: Mutex<Option<Egg>>,
}

impl FileSource {
    pub fn new(location: impl Into<String>, fetch: Option<Arc<dyn Fetch>>) -> FileSource {
        FileSource {
            location: location.into(),
            fetch,
            egg: Mutex::new(None),
        }
    }

    fn is_url(&self) -> bool {
        self.location.starts_with("http://") || self.location.starts_with("https://")
    }

    /// The egg, read the first time it is asked for.
    async fn egg(&self) -> Result<Egg> {
        let held = self.egg.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(egg) = held {
            return Ok(egg);
        }
        let egg = if self.is_url() {
            let fetch = self.fetch.as_ref().ok_or_else(|| {
                Error::other(format!(
                    "{}: reading an egg from a URL needs a Fetch, which the tool must pass",
                    self.location
                ))
            })?;
            let url = Url::parse(&self.location).map_err(|e| Error::invalid(&self.location, e))?;
            let bytes = fetch
                .get(&url, None)
                .await?
                .ok_or_else(|| Error::NotFound {
                    at: At::new(&self.location),
                })?;
            Egg::open(bytes)?
        } else {
            let path = Path::new(&self.location);
            let bytes = std::fs::read(path).at_path(path)?;
            Egg::open(bytes)?
        };
        if egg.variant().is_some() {
            return Err(Error::invalid(
                &self.location,
                "this is a variant egg; give the base egg or a fat egg",
            ));
        }
        *self.egg.lock().unwrap_or_else(|e| e.into_inner()) = Some(egg.clone());
        Ok(egg)
    }
}

impl Source for FileSource {
    fn label(&self) -> &str {
        &self.location
    }

    fn is_local(&self) -> bool {
        !self.is_url()
    }

    fn names(&self) -> BoxFuture<'_, Result<Vec<Name>>> {
        Box::pin(async move { Ok(vec![self.egg().await?.manifest().name()]) })
    }

    fn versions<'a>(&'a self, name: &'a Name) -> BoxFuture<'a, Result<Vec<Offer>>> {
        Box::pin(async move {
            let egg = self.egg().await?;
            let m = egg.manifest();
            if m.name() != *name {
                return Ok(Vec::new());
            }
            let mut offer = Offer::new(m.name(), m.version().clone(), &self.location);
            // The checksum is of the base egg, which a fat egg contains.
            offer.sha256 = Some(egg.base()?.sha256().to_string());
            offer.details = Some(Details::from_manifest(m));
            Ok(vec![offer])
        })
    }

    fn fetch<'a>(&'a self, offer: &'a Offer) -> BoxFuture<'a, Result<Egg>> {
        Box::pin(async move {
            let egg = self.egg().await?;
            let m = egg.manifest();
            if m.name() != offer.name || *m.version() != offer.version {
                return Err(Error::invalid(
                    &self.location,
                    format!(
                        "the egg is {} {}, not {} {}",
                        m.name(),
                        m.version(),
                        offer.name,
                        offer.version
                    ),
                ));
            }
            Ok(egg)
        })
    }

    fn fetch_variant<'a>(
        &'a self,
        offer: &'a Offer,
        target: &'a str,
        key: &'a str,
    ) -> BoxFuture<'a, Result<Egg>> {
        Box::pin(async move {
            let egg = self.fetch(offer).await?;
            egg.embedded_variant(target, key)?
                .ok_or_else(|| Error::NotFound {
                    at: At::new(&offer.name)
                        .version(format!("{} {target}.{key}", offer.version))
                        .registry(&self.location),
                })
        })
    }
}
