//! `eggs.lock`: what is installed in a folder and from where.

use std::collections::BTreeMap;
use std::path::Path;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};
use crate::fsx::write_atomic;
use crate::name::Name;

/// The name of the lock file in an install folder.
pub const FILE: &str = "eggs.lock";

/// The format of `eggs.lock` and of the files in `.hatchery/` this crate writes.
pub const FORMAT: u32 = 1;

/// The packages installed in a folder. It is the content of `eggs.lock`.
/// Keys this version does not know are kept when the file is written again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lock {
    /// The format of the file, [`FORMAT`]. A file without it is format 1.
    #[serde(default = "format_one")]
    pub format: u32,
    /// The installed packages. [`Lock::save`] writes them sorted by name.
    #[serde(default, rename = "package")]
    pub packages: Vec<LockedPackage>,
    #[serde(flatten)]
    extra: BTreeMap<String, toml::Value>,
}

impl Default for Lock {
    fn default() -> Lock {
        Lock {
            format: FORMAT,
            packages: Vec::new(),
            extra: BTreeMap::new(),
        }
    }
}

/// Refuses a file of a format this version cannot read.
pub(crate) fn check_format(what: &str, format: u32) -> Result<()> {
    if format != FORMAT {
        return Err(Error::invalid(
            what,
            format!("format {format} is not supported; this Hatchery reads format {FORMAT}"),
        ));
    }
    Ok(())
}

/// One installed package. Keys this version does not know are kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LockedPackage {
    /// The package.
    pub name: Name,
    /// The installed version.
    pub version: Version,
    /// The targets installed from it.
    pub targets: Vec<String>,
    /// A static registry or a git registry, by its address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    /// A git repository the package came from (a git registry or one folder).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    /// The package's folder in the repository. Set for a git registry and for one git folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The tag or branch the version was taken from. Set for a git registry and for one git folder.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    /// The commit the version was taken from. A later install checks that the tag still points at it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// An egg file or URL it was installed from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// A package folder it was installed from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// The base egg's checksum.
    pub sha256: String,
    /// The variant eggs fetched so far, `"<target>.<key>" = sha256`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variants: BTreeMap<String, String>,
    #[serde(flatten)]
    pub(crate) extra: BTreeMap<String, toml::Value>,
}

impl Lock {
    /// Reads `eggs.lock` in a folder; an empty lock when there is none.
    pub fn load(dir: &Path) -> Result<Lock> {
        let path = dir.join(FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Lock::default()),
            Err(e) => return Err(e).at_path(&path),
        };
        let lock: Lock = toml::from_str(&text)
            .map_err(|e| Error::invalid(path.display(), e.to_string().trim()))?;
        check_format(&path.display().to_string(), lock.format)?;
        Ok(lock)
    }

    /// Writes `eggs.lock` in a folder, sorted by name.
    pub fn save(&self, dir: &Path) -> Result<()> {
        let mut sorted = self.clone();
        sorted.format = FORMAT;
        sorted.packages.sort_by(|a, b| a.name.cmp(&b.name));
        let text = toml::to_string(&sorted)
            .map_err(|e| Error::invalid(FILE, format!("cannot write: {e}")))?;
        write_atomic(&dir.join(FILE), text.as_bytes())
    }

    /// The entry of a package. None when it is not installed.
    pub fn get(&self, name: &Name) -> Option<&LockedPackage> {
        self.packages.iter().find(|p| &p.name == name)
    }

    /// The entry of a package, to change. None when it is not installed.
    pub fn get_mut(&mut self, name: &Name) -> Option<&mut LockedPackage> {
        self.packages.iter_mut().find(|p| &p.name == name)
    }

    /// Adds a package, replacing the entry of the same name.
    pub fn set(&mut self, pkg: LockedPackage) {
        match self.get_mut(&pkg.name) {
            Some(slot) => *slot = pkg,
            None => self.packages.push(pkg),
        }
    }

    /// Removes a package; whether it was there.
    pub fn remove(&mut self, name: &Name) -> bool {
        let before = self.packages.len();
        self.packages.retain(|p| &p.name != name);
        self.packages.len() != before
    }
}

/// The base egg each package folder was unpacked from, kept in
/// `.hatchery/unpacked.toml`. `install` compares it with the lock, because
/// the lock says what should be installed and the folder may hold something
/// older.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Unpacked {
    #[serde(default = "format_one")]
    format: u32,
    /// `group/id` to the SHA-256 of the base egg.
    #[serde(default)]
    packages: BTreeMap<String, String>,
}

fn format_one() -> u32 {
    FORMAT
}

impl Unpacked {
    const FILE: &'static str = "unpacked.toml";

    fn path(dir: &Path) -> std::path::PathBuf {
        dir.join(crate::seen::DIR).join(Self::FILE)
    }

    /// The records of an install folder. None of them when the file is
    /// missing or does not parse: a package is then unpacked again.
    pub(crate) fn load(dir: &Path) -> Unpacked {
        std::fs::read_to_string(Self::path(dir))
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub(crate) fn save(&self, dir: &Path) -> Result<()> {
        let text = toml::to_string(self)
            .map_err(|e| Error::invalid(Self::FILE, format!("cannot write: {e}")))?;
        write_atomic(&Self::path(dir), text.as_bytes())
    }

    pub(crate) fn get(&self, name: &Name) -> Option<&str> {
        self.packages.get(&name.to_string()).map(String::as_str)
    }

    pub(crate) fn set(&mut self, name: &Name, sha256: &str) {
        self.packages.insert(name.to_string(), sha256.to_string());
    }

    pub(crate) fn remove(&mut self, name: &Name) {
        self.packages.remove(&name.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str, version: &str) -> LockedPackage {
        LockedPackage {
            name: Name::parse(name).unwrap(),
            version: Version::parse(version).unwrap(),
            targets: vec!["crow".into()],
            registry: Some("https://eggs.example.org".into()),
            git: None,
            path: None,
            git_ref: None,
            commit: None,
            file: None,
            folder: None,
            sha256: "ab".repeat(32),
            variants: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn saves_sorted_and_loads_back() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(Lock::load(tmp.path()).unwrap(), Lock::default());

        let mut lock = Lock::default();
        let mut b = pkg("aiviarium/zeta", "1.0.0");
        b.git = Some("https://example.org/p.git".into());
        b.git_ref = Some("v1.0.0".into());
        b.commit = Some("4be1c0d".into());
        b.variants.insert("rookery.x86_64".into(), "cd".repeat(32));
        lock.set(b);
        lock.set(pkg("aiviarium/alpha", "2.1.0"));
        lock.save(tmp.path()).unwrap();

        let text = std::fs::read_to_string(tmp.path().join(FILE)).unwrap();
        assert!(text.find("aiviarium/alpha").unwrap() < text.find("aiviarium/zeta").unwrap());
        assert!(text.contains("[[package]]"));
        assert!(text.contains("ref = \"v1.0.0\""));

        let back = Lock::load(tmp.path()).unwrap();
        assert_eq!(back.packages.len(), 2);
        assert_eq!(back.packages[0].name.id, "alpha");
        assert_eq!(back.packages[1].variants.len(), 1);
        assert_eq!(
            back.get(&Name::parse("aiviarium/alpha").unwrap())
                .unwrap()
                .version,
            Version::new(2, 1, 0)
        );
    }

    #[test]
    fn a_file_that_does_not_parse_is_invalid() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(FILE), "[[package]]\nname = 3\n").unwrap();
        assert!(matches!(Lock::load(tmp.path()), Err(Error::Invalid { .. })));
        std::fs::write(tmp.path().join(FILE), "not toml at all =").unwrap();
        assert!(matches!(Lock::load(tmp.path()), Err(Error::Invalid { .. })));
    }

    #[test]
    fn writes_the_format_and_keeps_unknown_keys() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(FILE),
            "format = 1\nnote = \"kept\"\n\n[[package]]\nname = \"a/b\"\nversion = \"1.0.0\"\n\
             targets = [\"crow\"]\nsha256 = \"ab\"\nmirror = \"https://example.org\"\n",
        )
        .unwrap();
        let mut lock = Lock::load(tmp.path()).unwrap();
        lock.set(pkg("a/c", "2.0.0"));
        lock.save(tmp.path()).unwrap();
        let text = std::fs::read_to_string(tmp.path().join(FILE)).unwrap();
        assert!(text.starts_with("format = 1\n"), "{text}");
        assert!(text.contains("note = \"kept\""), "{text}");
        assert!(text.contains("mirror = \"https://example.org\""), "{text}");
        let fresh = tempfile::tempdir().unwrap();
        Lock::default().save(fresh.path()).unwrap();
        let text = std::fs::read_to_string(fresh.path().join(FILE)).unwrap();
        assert!(text.contains("format = 1"), "{text}");
        std::fs::write(fresh.path().join(FILE), "format = 2\n").unwrap();
        let err = Lock::load(fresh.path()).unwrap_err();
        assert!(err.to_string().contains("format 2"), "{err}");
    }

    #[test]
    fn set_replaces_and_remove_removes() {
        let mut lock = Lock::default();
        lock.set(pkg("a/b", "1.0.0"));
        lock.set(pkg("a/b", "1.1.0"));
        assert_eq!(lock.packages.len(), 1);
        assert_eq!(lock.packages[0].version, Version::new(1, 1, 0));
        assert!(lock.remove(&Name::parse("a/b").unwrap()));
        assert!(!lock.remove(&Name::parse("a/b").unwrap()));
    }
}
