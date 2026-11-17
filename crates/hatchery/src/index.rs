//! The JSON files of a static registry: `<base>/index.json` lists the
//! packages, and `<base>/<group>/<id>/index.json` lists one package's
//! versions. Unknown keys are kept when a file is written again.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::manifest::{About, Manifest, Requirement, variant_key};
use crate::name::{Name, parse_range, parse_version};
use crate::source::{Details, Offer, TargetInfo, VariantInfo, Withdrawn};
use crate::time::Timestamp;

/// `<base>/index.json`: the packages the registry has.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    #[serde(default)]
    pub packages: Vec<Name>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Catalog {
    /// Adds a name, keeping the list sorted. False when it was there.
    pub fn add(&mut self, name: &Name) -> bool {
        if self.packages.contains(name) {
            return false;
        }
        self.packages.push(name.clone());
        self.packages.sort();
        true
    }
}

/// `<base>/<group>/<id>/index.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackageIndex {
    pub group: String,
    pub id: String,
    #[serde(default)]
    pub about: IndexAbout,
    #[serde(default)]
    pub versions: BTreeMap<String, IndexVersion>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl PackageIndex {
    pub fn new(name: &Name) -> PackageIndex {
        PackageIndex {
            group: name.group.clone(),
            id: name.id.clone(),
            about: IndexAbout::default(),
            versions: BTreeMap::new(),
            extra: Map::new(),
        }
    }

    pub fn name(&self) -> Result<Name> {
        Name::new(&self.group, &self.id)
    }

    /// The offers of the versions that have a base egg. A version with
    /// only variant eggs is not offered.
    pub fn offers(&self, registry: &str) -> Result<Vec<Offer>> {
        let name = self.name()?;
        let about = self.about.to_about();
        let mut out = Vec::new();
        for (text, v) in &self.versions {
            let Some(sha256) = &v.sha256 else { continue };
            let version = parse_version(text).map_err(|e| {
                Error::invalid(format!("index of {name}"), format!("version {text}: {e}"))
            })?;
            let mut offer = Offer::new(name.clone(), version, registry);
            offer.published = v.published;
            offer.withdrawn = v.withdrawn.as_ref().map(|w| Withdrawn {
                reason: w.reason.clone(),
            });
            offer.sha256 = Some(sha256.clone());
            offer.signature = v.signature.clone();
            offer.built = true;
            offer.details = Some(Details {
                about: about.clone(),
                targets: v.target_infos(&name)?,
            });
            out.push(offer);
        }
        Ok(out)
    }
}

/// What the index says about a package, for display.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IndexAbout {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The file name of the icon copy beside the index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub licenses: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl IndexAbout {
    /// The index's about from a manifest's. The icon is the file name of
    /// the copy kept beside the index.
    pub fn from_about(a: &About) -> IndexAbout {
        IndexAbout {
            name: a.name.clone(),
            summary: a.summary.clone(),
            icon: a.icon.as_deref().map(file_name_of),
            licenses: a.licenses.clone(),
            authors: a.authors.clone(),
            homepage: a.homepage.clone(),
            repository: a.repository.clone(),
            keywords: a.keywords.clone(),
            extra: Map::new(),
        }
    }

    pub fn to_about(&self) -> About {
        About {
            name: self.name.clone(),
            summary: self.summary.clone(),
            icon: self.icon.clone(),
            licenses: self.licenses.clone(),
            authors: self.authors.clone(),
            homepage: self.homepage.clone(),
            repository: self.repository.clone(),
            keywords: self.keywords.clone(),
            ..About::default()
        }
    }
}

/// The last `/`-separated part of a path.
pub fn file_name_of(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// One version of a package.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IndexVersion {
    /// The base egg's checksum. None until the base egg is published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawn: Option<IndexWithdrawn>,
    #[serde(default)]
    pub targets: BTreeMap<String, IndexTarget>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl IndexVersion {
    /// An entry for a manifest's targets, without checksum, signature or
    /// publish time.
    pub fn from_manifest(m: &Manifest) -> IndexVersion {
        let details = Details::from_manifest(m);
        IndexVersion {
            targets: details
                .targets
                .iter()
                .map(|t| (t.name.clone(), IndexTarget::from_info(t)))
                .collect(),
            ..IndexVersion::default()
        }
    }

    fn target_infos(&self, name: &Name) -> Result<Vec<TargetInfo>> {
        self.targets
            .iter()
            .map(|(target, t)| t.to_info(name, target))
            .collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexWithdrawn {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A `requires` value: one value as a string, several as an array, none
/// as `true`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IndexRequirement {
    One(String),
    Many(Vec<String>),
    Flag(bool),
}

impl IndexRequirement {
    fn from_values(values: &[String]) -> IndexRequirement {
        match values {
            [] => IndexRequirement::Flag(true),
            [one] => IndexRequirement::One(one.clone()),
            many => IndexRequirement::Many(many.to_vec()),
        }
    }

    fn values(&self) -> Vec<String> {
        match self {
            IndexRequirement::One(v) => vec![v.clone()],
            IndexRequirement::Many(v) => v.clone(),
            IndexRequirement::Flag(_) => Vec::new(),
        }
    }
}

/// What a version says about one target.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IndexTarget {
    #[serde(default, skip_serializing_if = "is_false")]
    pub build: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub requires: BTreeMap<String, IndexRequirement>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, String>,
    #[serde(
        default,
        rename = "optional-dependencies",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub optional_dependencies: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<IndexVariant>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl IndexTarget {
    pub fn from_info(t: &TargetInfo) -> IndexTarget {
        let deps = |ds: &[(Name, semver::VersionReq)]| {
            ds.iter()
                .map(|(n, r)| (n.to_string(), r.to_string()))
                .collect::<BTreeMap<_, _>>()
        };
        IndexTarget {
            build: t.build,
            requires: t
                .requires
                .iter()
                .map(|r| (r.fact.clone(), IndexRequirement::from_values(&r.values)))
                .collect(),
            dependencies: deps(&t.dependencies),
            optional_dependencies: deps(&t.optional_dependencies),
            variants: t.variants.iter().map(IndexVariant::from_info).collect(),
            extra: Map::new(),
        }
    }

    fn to_info(&self, name: &Name, target: &str) -> Result<TargetInfo> {
        let what = format!("index of {name}, target {target}");
        let deps = |m: &BTreeMap<String, String>| -> Result<Vec<(Name, semver::VersionReq)>> {
            m.iter()
                .map(|(n, r)| {
                    let dep = Name::parse(n).map_err(|e| Error::invalid(&what, e))?;
                    let range = parse_range(r).map_err(|e| Error::invalid(&what, e))?;
                    Ok((dep, range))
                })
                .collect()
        };
        Ok(TargetInfo {
            name: target.to_string(),
            build: self.build,
            requires: self
                .requires
                .iter()
                .map(|(fact, v)| Requirement {
                    fact: fact.clone(),
                    values: v.values(),
                })
                .collect(),
            dependencies: deps(&self.dependencies)?,
            optional_dependencies: deps(&self.optional_dependencies)?,
            variants: self.variants.iter().map(IndexVariant::to_info).collect(),
        })
    }

    /// The entry of the variant with this key.
    pub fn variant(&self, key: &str) -> Option<&IndexVariant> {
        self.variants.iter().find(|v| v.key() == key)
    }
}

/// One variant of a target, with its egg's checksum once published.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IndexVariant {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<Timestamp>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl IndexVariant {
    pub fn from_info(v: &VariantInfo) -> IndexVariant {
        IndexVariant {
            os: v.os.clone(),
            arch: v.arch.clone(),
            sha256: v.sha256.clone(),
            signature: v.signature.clone(),
            published: v.published,
            extra: Map::new(),
        }
    }

    pub fn to_info(&self) -> VariantInfo {
        VariantInfo {
            os: self.os.clone(),
            arch: self.arch.clone(),
            sha256: self.sha256.clone(),
            signature: self.signature.clone(),
            published: self.published,
        }
    }

    pub fn key(&self) -> String {
        variant_key(self.os.as_deref(), self.arch.as_deref())
    }
}

/// Pretty JSON with a trailing newline.
pub fn to_json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut out = serde_json::to_vec_pretty(value)
        .map_err(|e| Error::other(format!("writing an index: {e}")))?;
    out.push(b'\n');
    Ok(out)
}

/// Parses an index file. `what` names it in the error.
pub fn from_json<T: for<'de> Deserialize<'de>>(what: &str, bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| Error::invalid(what, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
  "group": "example",
  "id": "tool",
  "about": { "name": "Tool", "icon": "icon.svg", "licenses": ["MIT"], "x-new": 1 },
  "future": { "a": [1, 2] },
  "versions": {
    "1.4.0": {
      "sha256": "ab",
      "signature": null,
      "published": "2026-11-10T09:12:00Z",
      "withdrawn": null,
      "targets": {
        "crow": { "build": true, "requires": { "crow": "^1" },
                  "dependencies": { "example/agents": "^1.2" },
                  "optional-dependencies": {} },
        "rookery": { "requires": { "rookery": ">=0.7", "os": ["linux", "macos"], "gpu": true },
                     "variants": [ { "arch": "x86_64", "sha256": "cd", "z": 2 }, { "arch": "aarch64" } ] }
      },
      "note": "kept"
    },
    "1.5.0": { "targets": {} }
  }
}"#;

    #[test]
    fn reads_and_keeps_unknown_keys() {
        let index: PackageIndex = from_json("t", SAMPLE.as_bytes()).unwrap();
        assert_eq!(index.extra["future"]["a"][1], 2);
        assert_eq!(index.about.extra["x-new"], 1);
        let again: PackageIndex = from_json("t", &to_json(&index).unwrap()).unwrap();
        assert_eq!(again, index);
        let text = String::from_utf8(to_json(&index).unwrap()).unwrap();
        assert!(text.ends_with("}\n"));
        assert!(text.contains("\"note\": \"kept\""));
        assert!(text.contains("\"z\": 2"));
    }

    #[test]
    fn offers_only_versions_with_a_checksum() {
        let index: PackageIndex = from_json("t", SAMPLE.as_bytes()).unwrap();
        let offers = index.offers("reg").unwrap();
        assert_eq!(offers.len(), 1);
        let o = &offers[0];
        assert_eq!(o.version.to_string(), "1.4.0");
        assert_eq!(o.sha256.as_deref(), Some("ab"));
        let d = o.details.as_ref().unwrap();
        assert_eq!(d.about.icon.as_deref(), Some("icon.svg"));
        let rookery = d.target("rookery").unwrap();
        assert_eq!(rookery.variants.len(), 2);
        assert_eq!(rookery.variants[0].sha256.as_deref(), Some("cd"));
        let os = rookery.requires.iter().find(|r| r.fact == "os").unwrap();
        assert_eq!(os.values, ["linux", "macos"]);
        let gpu = rookery.requires.iter().find(|r| r.fact == "gpu").unwrap();
        assert!(gpu.values.is_empty());
        assert!(d.target("crow").unwrap().build);
    }

    #[test]
    fn requirement_values_round_trip() {
        for (values, json) in [
            (vec![], "true"),
            (vec!["a".to_string()], "\"a\""),
            (vec!["a".to_string(), "b".to_string()], "[\"a\",\"b\"]"),
        ] {
            let r = IndexRequirement::from_values(&values);
            assert_eq!(serde_json::to_string(&r).unwrap(), json);
            assert_eq!(r.values(), values);
        }
    }
}
