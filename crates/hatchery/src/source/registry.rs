//! Static registries: the layout of index files and eggs, read from a
//! folder, over HTTP, or from a branch of a git repository.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use semver::Version;

use crate::digest::sha256_hex;
use crate::egg::Egg;
use crate::error::{At, Error, IoContext, Result};
use crate::fetch::{BoxFuture, Fetch, Token, Url};
use crate::index::{self, Catalog, PackageIndex};
use crate::name::Name;
use crate::sign::{self, VerifyingKey};
use crate::source::{Offer, RegistryFiles, Source};
use crate::time::Timestamp;

/// The largest egg a registry hands out.
pub const MAX_EGG_SIZE: usize = 512 * 1024 * 1024;

/// A static registry over any [`RegistryFiles`].
pub struct StaticRegistry {
    files: Arc<dyn RegistryFiles>,
    key: Option<VerifyingKey>,
}

impl StaticRegistry {
    /// A registry over any [`RegistryFiles`]. With a `key`, an egg must be signed by it.
    pub fn new(files: Arc<dyn RegistryFiles>, key: Option<VerifyingKey>) -> StaticRegistry {
        StaticRegistry { files, key }
    }

    /// A registry folder.
    pub fn dir(path: impl Into<PathBuf>, key: Option<VerifyingKey>) -> StaticRegistry {
        StaticRegistry::new(Arc::new(DirFiles::new(path)), key)
    }

    /// A registry over HTTP.
    pub fn http(
        base: Url,
        fetch: Arc<dyn Fetch>,
        token: Option<Token>,
        key: Option<VerifyingKey>,
    ) -> StaticRegistry {
        StaticRegistry::new(Arc::new(HttpFiles::new(base, fetch, token)), key)
    }

    /// The index of a package, None when the registry has none.
    pub fn index<'a>(&'a self, name: &'a Name) -> BoxFuture<'a, Result<Option<PackageIndex>>> {
        Box::pin(async move { self.read_index(name).await })
    }

    async fn read_index(&self, name: &Name) -> Result<Option<PackageIndex>> {
        let path = format!("{}/{}/index.json", name.group, name.id);
        let Some(bytes) = self.files.read(&path).await? else {
            return Ok(None);
        };
        let what = format!("{}/{path}", self.files.label());
        let index: PackageIndex = index::from_json(&what, &bytes)?;
        if index.group != name.group || index.id != name.id {
            return Err(Error::invalid(
                what,
                format!("the index is for {}/{}, not {name}", index.group, index.id),
            ));
        }
        Ok(Some(index))
    }

    fn at(&self, name: &Name, version: impl std::fmt::Display) -> At {
        At::new(name).version(version).registry(self.files.label())
    }

    /// Reads an egg file, checks its size and checksum and, when the
    /// registry has a key, its signature.
    async fn read_egg(
        &self,
        at: &At,
        name: &Name,
        version: &Version,
        file: &str,
        entry: Entry<'_>,
    ) -> Result<Egg> {
        let path = format!("{}/{}/{file}", name.group, name.id);
        let bytes = self
            .files
            .read(&path)
            .await?
            .ok_or_else(|| Error::NotFound { at: at.clone() })?;
        if bytes.len() > MAX_EGG_SIZE {
            return Err(Error::invalid(
                format!("{}/{path}", self.files.label()),
                "the egg is larger than 512 MiB",
            ));
        }
        let actual = sha256_hex(&bytes);
        let Some(expected) = entry.sha256 else {
            return Err(Error::NotFound { at: at.clone() });
        };
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(Error::ChecksumMismatch {
                at: at.clone(),
                expected: expected.to_string(),
                actual,
            });
        }
        if let Some(key) = &self.key {
            let invalid = |reason: &str| Error::SignatureInvalid {
                at: at.clone(),
                reason: reason.to_string(),
            };
            let signature = entry
                .signature
                .ok_or_else(|| invalid("the egg is not signed"))?;
            let published = entry
                .published
                .ok_or_else(|| invalid("the index has no publish time to check against"))?;
            let message = sign::message(name, version, file, &actual, &published);
            sign::verify(key, &message, signature).map_err(|reason| invalid(&reason))?;
        }
        Egg::open(bytes)
    }
}

/// What the index says about one egg.
struct Entry<'a> {
    sha256: Option<&'a str>,
    signature: Option<&'a str>,
    published: Option<Timestamp>,
}

impl Source for StaticRegistry {
    fn label(&self) -> &str {
        self.files.label()
    }

    fn is_local(&self) -> bool {
        self.files.is_local()
    }

    fn names(&self) -> BoxFuture<'_, Result<Vec<Name>>> {
        Box::pin(async move {
            let Some(bytes) = self.files.read("index.json").await? else {
                return Ok(Vec::new());
            };
            let what = format!("{}/index.json", self.files.label());
            let catalog: Catalog = index::from_json(&what, &bytes)?;
            Ok(catalog.packages)
        })
    }

    fn versions<'a>(&'a self, name: &'a Name) -> BoxFuture<'a, Result<Vec<Offer>>> {
        Box::pin(async move {
            match self.read_index(name).await? {
                Some(index) => index.offers(self.files.label()),
                None => Ok(Vec::new()),
            }
        })
    }

    fn fetch<'a>(&'a self, offer: &'a Offer) -> BoxFuture<'a, Result<Egg>> {
        Box::pin(async move {
            let name = &offer.name;
            let at = self.at(name, &offer.version);
            let file = format!("{}-{}.egg", name.id, offer.version);
            // The offer carries the index entry it came from; ask the index
            // again only for an offer made some other way.
            let index;
            let entry = if offer.sha256.is_some() {
                Entry {
                    sha256: offer.sha256.as_deref(),
                    signature: offer.signature.as_deref(),
                    published: offer.published,
                }
            } else {
                index = self.read_index(name).await?;
                let v = index
                    .as_ref()
                    .and_then(|i| i.versions.get(&offer.version.to_string()))
                    .ok_or_else(|| Error::NotFound { at: at.clone() })?;
                Entry {
                    sha256: v.sha256.as_deref(),
                    signature: v.signature.as_deref(),
                    published: v.published,
                }
            };
            let egg = self
                .read_egg(&at, name, &offer.version, &file, entry)
                .await?;
            let m = egg.manifest();
            if egg.variant().is_some() || m.name() != *name || *m.version() != offer.version {
                return Err(Error::invalid(
                    format!("{} {file}", self.files.label()),
                    format!("the egg is not {name} {}", offer.version),
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
            let name = &offer.name;
            let at = self.at(name, format!("{} {target}.{key}", offer.version));
            let file = crate::manifest::variant_egg_name(&name.id, &offer.version, target, key);
            let from_offer = offer
                .details
                .as_ref()
                .and_then(|d| d.target(target))
                .and_then(|t| t.variants.iter().find(|v| v.key() == key))
                .filter(|v| v.sha256.is_some());
            let index;
            let entry = match from_offer {
                Some(v) => Entry {
                    sha256: v.sha256.as_deref(),
                    signature: v.signature.as_deref(),
                    published: v.published,
                },
                None => {
                    index = self.read_index(name).await?;
                    let v = index
                        .as_ref()
                        .and_then(|i| i.versions.get(&offer.version.to_string()))
                        .and_then(|v| v.targets.get(target))
                        .and_then(|t| t.variant(key))
                        .ok_or_else(|| Error::NotFound { at: at.clone() })?;
                    Entry {
                        sha256: v.sha256.as_deref(),
                        signature: v.signature.as_deref(),
                        published: v.published,
                    }
                }
            };
            let egg = self
                .read_egg(&at, name, &offer.version, &file, entry)
                .await?;
            let ok = egg.variant().is_some_and(|h| {
                h.name == *name
                    && h.version == offer.version
                    && h.target == target
                    && h.key() == key
            });
            if !ok {
                return Err(Error::invalid(
                    format!("{} {file}", self.files.label()),
                    format!(
                        "the egg is not the {target} {key} variant of {name} {}",
                        offer.version
                    ),
                ));
            }
            Ok(egg)
        })
    }
}

/// Rejects a path that leaves the registry.
fn check_path(path: &str) -> Result<()> {
    let bad = path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.split('/').any(|p| p.is_empty() || p == "..");
    if bad {
        return Err(Error::invalid(
            "registry path",
            format!("{path:?} is not allowed"),
        ));
    }
    Ok(())
}

/// The files of a registry folder.
pub struct DirFiles {
    root: PathBuf,
    label: String,
}

impl DirFiles {
    /// A registry folder at `path`. The path is its label.
    pub fn new(path: impl Into<PathBuf>) -> DirFiles {
        let root = path.into();
        let label = root.display().to_string();
        DirFiles { root, label }
    }

    /// The registry folder.
    pub fn path(&self) -> &Path {
        &self.root
    }
}

impl RegistryFiles for DirFiles {
    fn label(&self) -> &str {
        &self.label
    }

    fn is_local(&self) -> bool {
        true
    }

    fn read<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<Option<Bytes>>> {
        Box::pin(async move {
            check_path(path)?;
            let file = path.split('/').fold(self.root.clone(), |p, s| p.join(s));
            match std::fs::read(&file) {
                Ok(bytes) => Ok(Some(Bytes::from(bytes))),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e).at_path(file),
            }
        })
    }
}

/// The files of a registry over HTTP.
pub struct HttpFiles {
    /// The address files are read under, which may carry credentials.
    base: String,
    /// The address without credentials, for messages and records.
    label: String,
    fetch: Arc<dyn Fetch>,
    token: Option<Token>,
}

impl HttpFiles {
    /// The files under `base`. `fetch` gets each of them, with `token` as the bearer token when there is one. A trailing `/` of `base` is dropped, and the label is `base` without a user or password.
    pub fn new(base: Url, fetch: Arc<dyn Fetch>, token: Option<Token>) -> HttpFiles {
        let base = base.as_str().trim_end_matches('/').to_string();
        HttpFiles {
            label: crate::fsx::redact_url(&base),
            base,
            fetch,
            token,
        }
    }
}

impl RegistryFiles for HttpFiles {
    fn label(&self) -> &str {
        &self.label
    }

    fn read<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<Option<Bytes>>> {
        Box::pin(async move {
            check_path(path)?;
            let url = Url::parse(&format!("{}/{path}", self.base))
                .map_err(|e| Error::invalid(&self.label, e))?;
            self.fetch.get(&url, self.token.as_ref()).await
        })
    }
}
