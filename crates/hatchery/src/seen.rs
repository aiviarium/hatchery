//! First-seen records: when this installation first saw each version of a
//! package in each registry. The minimum release age counts from them for
//! versions without a signed publish time, and they stop a version that
//! shows up late from claiming an old publish time.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};
use crate::fsx::write_atomic;
use crate::name::Name;
use crate::time::Timestamp;

/// The folder in an install folder that holds Hatchery's own state.
pub const DIR: &str = ".hatchery";
pub const FILE: &str = "seen.toml";

/// What is known about one version in one registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub first_seen: Timestamp,
    /// Whether it was in the first listing of the package from the
    /// registry. Those versions were there before this installation looked.
    pub first_listing: bool,
}

type Versions = BTreeMap<String, Record>;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    /// Registry label, then package name, then version.
    #[serde(default)]
    registry: BTreeMap<String, BTreeMap<String, Versions>>,
    #[serde(skip)]
    dirty: bool,
}

impl Seen {
    /// The path of the records in an install folder.
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(DIR).join(FILE)
    }

    /// Reads the records of an install folder; none when there are none yet.
    pub fn load(dir: &Path) -> Result<Seen> {
        let path = Seen::path(dir);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Seen::default()),
            Err(e) => return Err(e).at_path(&path),
        };
        toml::from_str(&text).map_err(|e| Error::invalid(path.display(), e.to_string().trim()))
    }

    /// Writes the records when they changed since they were read.
    pub fn save(&mut self, dir: &Path) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let text = toml::to_string(self)
            .map_err(|e| Error::invalid(FILE, format!("cannot write: {e}")))?;
        write_atomic(&Seen::path(dir), text.as_bytes())?;
        self.dirty = false;
        Ok(())
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Notes the versions a registry listed for a package. Versions not
    /// seen before get `now`; when the package was not seen at all, they
    /// are the first listing.
    pub fn record<'a>(
        &mut self,
        registry: &str,
        name: &Name,
        versions: impl IntoIterator<Item = &'a Version>,
        now: Timestamp,
    ) {
        let mut versions = versions.into_iter().peekable();
        if versions.peek().is_none() {
            return;
        }
        let packages = self.registry.entry(registry.to_string()).or_default();
        let first = !packages.contains_key(&name.to_string());
        let known = packages.entry(name.to_string()).or_default();
        for v in versions {
            if let std::collections::btree_map::Entry::Vacant(e) = known.entry(v.to_string()) {
                e.insert(Record {
                    first_seen: now,
                    first_listing: first,
                });
                self.dirty = true;
            }
        }
    }

    pub fn get(&self, registry: &str, name: &Name, version: &Version) -> Option<&Record> {
        self.registry
            .get(registry)?
            .get(&name.to_string())?
            .get(&version.to_string())
    }

    /// The time a version counts from for the minimum release age. A
    /// version in the first listing counts from its publish time. A later
    /// one counts from the later of its publish time and when it was first
    /// seen, so that an old publish time cannot skip the wait. A version
    /// without a publish time counts from when it was first seen.
    pub fn counts_from(
        &self,
        registry: &str,
        name: &Name,
        version: &Version,
        published: Option<Timestamp>,
        now: Timestamp,
    ) -> Timestamp {
        let rec = self.get(registry, name, version);
        match (published, rec) {
            (Some(p), Some(r)) if r.first_listing => p,
            (Some(p), Some(r)) => p.max(r.first_seen),
            (Some(p), None) => p,
            (None, Some(r)) => r.first_seen,
            (None, None) => now,
        }
    }

    /// When a version becomes installable, or None when nothing holds it
    /// (a zero minimum age).
    pub fn held_until(
        &self,
        registry: &str,
        name: &Name,
        version: &Version,
        published: Option<Timestamp>,
        min_age: Duration,
        now: Timestamp,
    ) -> Option<Timestamp> {
        if min_age.is_zero() {
            return None;
        }
        Some(
            self.counts_from(registry, name, version, published, now)
                .plus(min_age),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: Duration = Duration::from_secs(86_400);

    fn t(s: &str) -> Timestamp {
        Timestamp::parse(s).unwrap()
    }

    #[test]
    fn counts_from_the_publish_time_in_the_first_listing() {
        let name = Name::parse("a/b").unwrap();
        let v1 = Version::new(1, 0, 0);
        let mut seen = Seen::default();
        seen.record("r", &name, [&v1], t("2026-11-10T00:00:00Z"));
        let published = Some(t("2026-01-01T00:00:00Z"));
        let now = t("2026-11-10T00:00:00Z");
        assert_eq!(
            seen.counts_from("r", &name, &v1, published, now),
            t("2026-01-01T00:00:00Z")
        );
        assert_eq!(
            seen.held_until("r", &name, &v1, published, 3 * DAY, now),
            Some(t("2026-01-04T00:00:00Z"))
        );
        assert_eq!(
            seen.held_until("r", &name, &v1, published, Duration::ZERO, now),
            None
        );
    }

    #[test]
    fn a_late_version_with_an_old_publish_time_counts_from_first_seen() {
        let name = Name::parse("a/b").unwrap();
        let (v1, v2) = (Version::new(1, 0, 0), Version::new(1, 1, 0));
        let mut seen = Seen::default();
        seen.record("r", &name, [&v1], t("2026-11-10T00:00:00Z"));
        seen.record("r", &name, [&v1, &v2], t("2026-11-12T00:00:00Z"));
        let old = Some(t("2026-01-01T00:00:00Z"));
        assert_eq!(
            seen.counts_from("r", &name, &v2, old, t("2026-11-12T06:00:00Z")),
            t("2026-11-12T00:00:00Z")
        );
        // A publish time after first seen cannot be earlier than the signed time.
        let later = Some(t("2026-11-13T00:00:00Z"));
        assert_eq!(
            seen.counts_from("r", &name, &v2, later, t("2026-11-12T06:00:00Z")),
            t("2026-11-13T00:00:00Z")
        );
        // The first record is kept.
        seen.record("r", &name, [&v2], t("2026-11-20T00:00:00Z"));
        assert_eq!(
            seen.get("r", &name, &v2).unwrap().first_seen,
            t("2026-11-12T00:00:00Z")
        );
    }

    #[test]
    fn a_version_without_publish_time_counts_from_first_seen() {
        let name = Name::parse("a/b").unwrap();
        let v1 = Version::new(1, 0, 0);
        let mut seen = Seen::default();
        seen.record("r", &name, [&v1], t("2026-11-10T00:00:00Z"));
        assert_eq!(
            seen.counts_from("r", &name, &v1, None, t("2026-11-11T00:00:00Z")),
            t("2026-11-10T00:00:00Z")
        );
    }

    #[test]
    fn saves_and_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let name = Name::parse("aiviarium/claude").unwrap();
        let v = Version::parse("1.4.0-rc.1").unwrap();
        let mut seen = Seen::load(tmp.path()).unwrap();
        assert!(!seen.is_dirty());
        seen.record(
            "https://eggs.example.org",
            &name,
            [&v],
            t("2026-11-10T00:00:00Z"),
        );
        assert!(seen.is_dirty());
        seen.save(tmp.path()).unwrap();
        assert!(!seen.is_dirty());
        let back = Seen::load(tmp.path()).unwrap();
        assert_eq!(back, seen);
        assert!(
            back.get("https://eggs.example.org", &name, &v)
                .unwrap()
                .first_listing
        );
        std::fs::write(Seen::path(tmp.path()), "registry = 3").unwrap();
        assert!(Seen::load(tmp.path()).is_err());
    }
}
