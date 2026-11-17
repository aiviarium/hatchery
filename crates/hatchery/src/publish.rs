//! `hatchery publish`: an egg into a static registry folder or the eggs
//! branch of a git repository.

use std::fs;
use std::path::{Path, PathBuf};

use semver::Version;

use crate::egg::Egg;
use crate::error::{At, Error, IoContext, Result};
use crate::fsx::write_atomic;
use crate::index::{
    self, Catalog, IndexAbout, IndexVariant, IndexVersion, IndexWithdrawn, PackageIndex,
};
use crate::name::Name;
use crate::sign::SigningKey;
use crate::time::Timestamp;

/// Where to publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishTo {
    /// A static registry folder.
    Dir(PathBuf),
    /// A branch of a git repository, in the static registry layout, under
    /// `path` when given. The branch is created when it does not exist.
    Git {
        /// The repository to push to, as an address or a path that `git` takes.
        url: String,
        /// The branch that holds the eggs.
        branch: String,
        /// A folder of the branch that holds the registry. None puts it at the root of the branch.
        path: Option<String>,
    },
}

/// What was published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    /// The package the egg belongs to.
    pub name: Name,
    /// The version of that package.
    pub version: Version,
    /// The egg's file name in the registry.
    pub file: String,
    /// The SHA-256 of the egg that was published, in lowercase hex. For a variant egg it is the variant egg's own.
    pub sha256: String,
    /// Whether the index holds a signature for the egg.
    pub signed: bool,
    /// The publish time written into the index. [`publish`] uses the current time.
    pub published: Timestamp,
}

/// Publishes an egg (base or variant) with the current time.
pub fn publish(egg: &Egg, to: &PublishTo, key: Option<&SigningKey>) -> Result<Published> {
    publish_at(egg, to, key, Timestamp::now())
}

/// Publishes an egg with a given publish time.
pub fn publish_at(
    egg: &Egg,
    to: &PublishTo,
    key: Option<&SigningKey>,
    now: Timestamp,
) -> Result<Published> {
    match to {
        PublishTo::Dir(dir) => publish_dir(egg, dir, key, now),
        PublishTo::Git { url, branch, path } => crate::source::git::publish_branch(
            egg,
            url,
            branch,
            path.as_deref(),
            key,
            now,
            &crate::git::Git::default(),
        ),
    }
}

/// Publishes into a static registry folder. A fat egg publishes its base
/// egg and each variant egg inside it.
pub fn publish_dir(
    egg: &Egg,
    dir: &Path,
    key: Option<&SigningKey>,
    now: Timestamp,
) -> Result<Published> {
    if egg.variant().is_some() {
        return publish_variant(egg, dir, key, now);
    }
    let base = egg.base()?;
    let published = publish_base(&base, dir, key, now)?;
    for (target, k) in egg.embedded_variants() {
        if let Some(variant) = egg.embedded_variant(&target, &k)? {
            publish_variant(&variant, dir, key, now)?;
        }
    }
    Ok(published)
}

/// Marks a published version as withdrawn in a static registry folder.
pub fn withdraw_dir(
    dir: &Path,
    name: &Name,
    version: &Version,
    reason: Option<&str>,
) -> Result<()> {
    let path = index_path(dir, name);
    let not_found = || Error::NotFound {
        at: At::new(name)
            .version(version)
            .registry(dir.display().to_string()),
    };
    let mut index = read_json::<PackageIndex>(&path)?.ok_or_else(not_found)?;
    let entry = index
        .versions
        .get_mut(&version.to_string())
        .ok_or_else(not_found)?;
    entry.withdrawn = Some(IndexWithdrawn {
        reason: reason.map(str::to_string),
        extra: Default::default(),
    });
    write_json(&path, &index)
}

fn package_dir(dir: &Path, name: &Name) -> PathBuf {
    dir.join(&name.group).join(&name.id)
}

fn index_path(dir: &Path, name: &Name) -> PathBuf {
    package_dir(dir, name).join("index.json")
}

fn read_json<T: for<'de> serde::Deserialize<'de>>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => index::from_json(&path.display().to_string(), &bytes).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).at_path(path),
    }
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    write_atomic(path, &index::to_json(value)?)
}

fn read_index(dir: &Path, name: &Name) -> Result<PackageIndex> {
    Ok(read_json(&index_path(dir, name))?.unwrap_or_else(|| PackageIndex::new(name)))
}

fn signature(
    key: Option<&SigningKey>,
    name: &Name,
    version: &Version,
    file: &str,
    sha256: &str,
    now: &Timestamp,
) -> Option<String> {
    key.map(|k| crate::sign::sign(k, &crate::sign::message(name, version, file, sha256, now)))
}

fn publish_base(
    egg: &Egg,
    dir: &Path,
    key: Option<&SigningKey>,
    now: Timestamp,
) -> Result<Published> {
    let manifest = egg.manifest();
    let name = manifest.name();
    let version = manifest.version().clone();
    let file = egg.file_name();
    let sha256 = egg.sha256().to_string();
    let mut index = read_index(dir, &name)?;
    let text = version.to_string();

    if let Some(old) = index.versions.get(&text)
        && let Some(old_sha) = &old.sha256
    {
        if old_sha.eq_ignore_ascii_case(&sha256) {
            return Ok(Published {
                name,
                version,
                file,
                sha256,
                signed: old.signature.is_some(),
                published: old.published.unwrap_or(now),
            });
        }
        return Err(Error::invalid(
            format!("{name} {version}"),
            "already published with different contents; versions cannot change",
        ));
    }

    let pkg_dir = package_dir(dir, &name);
    write_atomic(&pkg_dir.join(&file), egg.bytes())?;
    if let Some(icon) = &manifest.about.icon
        && let Some(entry) = egg.entries().iter().find(|e| e.path == *icon)
    {
        write_atomic(&pkg_dir.join(index::file_name_of(icon)), &entry.data)?;
    }

    let signature = signature(key, &name, &version, &file, &sha256, &now);
    let mut about = IndexAbout::from_about(&manifest.about);
    about.extra = std::mem::take(&mut index.about.extra);
    index.about = about;

    let mut entry = index.versions.remove(&text).unwrap_or_default();
    let mut fresh = IndexVersion::from_manifest(manifest);
    // Variant eggs published before the base keep their checksums.
    for (target, t) in fresh.targets.iter_mut() {
        let Some(old) = entry.targets.get(target) else {
            continue;
        };
        for v in t.variants.iter_mut() {
            if let Some(o) = old.variant(&v.key()) {
                *v = o.clone();
            }
        }
        t.extra = old.extra.clone();
    }
    entry.targets = fresh.targets;
    entry.sha256 = Some(sha256.clone());
    entry.signature = signature;
    entry.published = Some(now);
    let signed = entry.signature.is_some();
    index.versions.insert(text, entry);
    write_json(&index_path(dir, &name), &index)?;

    let catalog_path = dir.join("index.json");
    let mut catalog: Catalog = read_json(&catalog_path)?.unwrap_or_default();
    if catalog.add(&name) {
        write_json(&catalog_path, &catalog)?;
    }

    Ok(Published {
        name,
        version,
        file,
        sha256,
        signed,
        published: now,
    })
}

fn publish_variant(
    egg: &Egg,
    dir: &Path,
    key: Option<&SigningKey>,
    now: Timestamp,
) -> Result<Published> {
    let header = egg
        .variant()
        .ok_or_else(|| Error::other("not a variant egg"))?;
    let name = header.name.clone();
    let version = header.version.clone();
    let vkey = header.key();
    let file = egg.file_name();
    let sha256 = egg.sha256().to_string();
    let mut index = read_index(dir, &name)?;
    let text = version.to_string();

    let entry = match index.versions.entry(text) {
        std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
        std::collections::btree_map::Entry::Vacant(e) => {
            e.insert(IndexVersion::from_manifest(egg.manifest()))
        }
    };
    let target = entry.targets.entry(header.target.clone()).or_default();
    let pos = match target.variants.iter().position(|v| v.key() == vkey) {
        Some(p) => p,
        None => {
            target.variants.push(IndexVariant {
                os: header.os.clone(),
                arch: header.arch.clone(),
                ..IndexVariant::default()
            });
            target.variants.len() - 1
        }
    };
    let slot = &mut target.variants[pos];
    if let Some(old_sha) = &slot.sha256 {
        if old_sha.eq_ignore_ascii_case(&sha256) {
            return Ok(Published {
                name,
                version,
                file,
                sha256,
                signed: slot.signature.is_some(),
                published: slot.published.unwrap_or(now),
            });
        }
        return Err(Error::invalid(
            format!("{name} {version} {} {vkey}", header.target),
            "already published with different contents; versions cannot change",
        ));
    }

    write_atomic(&package_dir(dir, &name).join(&file), egg.bytes())?;
    slot.sha256 = Some(sha256.clone());
    slot.signature = signature(key, &name, &version, &file, &sha256, &now);
    slot.published = Some(now);
    let signed = slot.signature.is_some();
    write_json(&index_path(dir, &name), &index)?;

    Ok(Published {
        name,
        version,
        file,
        sha256,
        signed,
        published: now,
    })
}
