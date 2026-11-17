//! Static registries in a folder and over a test `Fetch`, and the folder and
//! file sources.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use hatchery::manifest::Origin;
use hatchery::publish::{publish_dir, withdraw_dir};
use hatchery::source::{FileSource, FolderSource, RegistryFiles, StaticRegistry};
use hatchery::{
    BoxFuture, Egg, Error, Fetch, Name, Offer, PackOptions, SigningKey, Source, Timestamp, Token,
    Url, block_on, pack,
};
use semver::Version;
use serde_json::Value;
use tempfile::TempDir;

const MANIFEST: &str = r#"format 1

package {
  group example
  id tool
  version "VERSION"
}

about {
  name "Tool"
  summary "A tool."
  icon "assets/icon.svg"
  license MIT
  keywords agent
}

targets {
  crow {
    requires {
      crow "^1"
    }
    dependencies {
      helper "^1.2"
    }
  }
  rookery {
    requires {
      rookery ">=0.7"
      os linux
    }
    variants {
      variant arch=x86_64
      variant arch=aarch64
    }
  }
}
"#;

fn t(s: &str) -> Timestamp {
    Timestamp::parse(s).unwrap()
}

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn name() -> Name {
    Name::parse("example/tool").unwrap()
}

/// A package folder for `example/tool` at a version; `salt` changes the
/// shared crow files.
fn package(root: &Path, version: &str, salt: &str) -> PathBuf {
    let dir = root.join(format!("pkg-{version}-{salt}"));
    common::package(
        &dir,
        &MANIFEST.replace("VERSION", version),
        &[
            ("assets/icon.svg", "<svg/>"),
            ("crow/plugin.json", salt),
            ("rookery/rookery.json", "{}"),
            ("rookery/variants/x86_64/bin", "x86"),
            ("rookery/variants/aarch64/bin", "arm"),
        ],
    );
    dir
}

fn options() -> PackOptions<'static> {
    PackOptions {
        no_build: true,
        origin: Some(Origin {
            built: Some(t("2026-11-01T00:00:00Z")),
            ..Origin::default()
        }),
        now: Some(t("2026-11-01T00:00:00Z")),
        ..PackOptions::default()
    }
}

/// The base egg and the variant eggs of a folder.
fn eggs(dir: &Path) -> (Egg, Vec<Egg>) {
    let mut all = pack(dir, &options()).unwrap();
    let base = all.pop().unwrap();
    assert!(base.variant().is_none());
    (base, all)
}

fn fat(dir: &Path) -> Egg {
    let opts = PackOptions {
        fat: true,
        ..options()
    };
    pack(dir, &opts).unwrap().pop().unwrap()
}

fn json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn offer(source: &dyn Source, version: &str) -> Offer {
    block_on(source.versions(&name()))
        .unwrap()
        .into_iter()
        .find(|o| o.version == Version::parse(version).unwrap())
        .unwrap_or_else(|| panic!("no offer {version}"))
}

struct Fixture {
    tmp: TempDir,
    reg: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let reg = tmp.path().join("registry");
    Fixture { tmp, reg }
}

const NOW: &str = "2026-11-10T09:12:00Z";

#[test]
fn publishes_and_fetches_an_unsigned_base_egg() {
    let f = fixture();
    let (base, _) = eggs(&package(f.tmp.path(), "1.4.0", "a"));
    let p = publish_dir(&base, &f.reg, None, t(NOW)).unwrap();
    assert_eq!(p.file, "tool-1.4.0.egg");
    assert_eq!(p.sha256, base.sha256());
    assert!(!p.signed);
    assert_eq!(p.published, t(NOW));

    assert_eq!(
        json(&f.reg.join("index.json"))["packages"][0],
        "example/tool"
    );
    let dir = f.reg.join("example/tool");
    assert_eq!(
        fs::read(dir.join("tool-1.4.0.egg")).unwrap(),
        base.bytes().as_ref()
    );
    assert_eq!(fs::read(dir.join("icon.svg")).unwrap(), b"<svg/>");
    let text = fs::read_to_string(dir.join("index.json")).unwrap();
    assert!(text.ends_with('\n'));
    let index: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(index["about"]["name"], "Tool");
    assert_eq!(index["about"]["icon"], "icon.svg");
    let v = &index["versions"]["1.4.0"];
    assert_eq!(v["sha256"], base.sha256());
    assert_eq!(v["published"], NOW);
    assert!(v.get("signature").is_none());
    assert_eq!(v["targets"]["crow"]["requires"]["crow"], "^1");
    assert_eq!(
        v["targets"]["crow"]["dependencies"]["example/helper"],
        "^1.2"
    );
    assert_eq!(v["targets"]["rookery"]["requires"]["os"], "linux");
    assert_eq!(v["targets"]["rookery"]["variants"][0]["arch"], "x86_64");

    let reg = StaticRegistry::dir(&f.reg, None);
    assert!(reg.is_local());
    assert_eq!(reg.label(), f.reg.display().to_string());
    assert_eq!(block_on(reg.names()).unwrap(), vec![name()]);
    let o = offer(&reg, "1.4.0");
    assert_eq!(o.published, Some(t(NOW)));
    assert!(o.built);
    let details = o.details.as_ref().unwrap();
    assert_eq!(details.about.name.as_deref(), Some("Tool"));
    assert_eq!(details.target("rookery").unwrap().variants.len(), 2);
    let egg = block_on(reg.fetch(&o)).unwrap();
    assert_eq!(egg.bytes(), base.bytes());
}

#[test]
fn an_empty_registry_offers_nothing() {
    let f = fixture();
    let reg = StaticRegistry::dir(&f.reg, None);
    assert!(block_on(reg.names()).unwrap().is_empty());
    assert!(block_on(reg.versions(&name())).unwrap().is_empty());
    assert!(block_on(reg.index(&name())).unwrap().is_none());
}

#[test]
fn signed_eggs_verify_against_the_registry_key() {
    let f = fixture();
    let (base, variants) = eggs(&package(f.tmp.path(), "1.4.0", "a"));
    let p = publish_dir(&base, &f.reg, Some(&key(7)), t(NOW)).unwrap();
    assert!(p.signed);
    for v in &variants {
        publish_dir(v, &f.reg, Some(&key(7)), t(NOW)).unwrap();
    }

    let good = StaticRegistry::dir(&f.reg, Some(key(7).verifying_key()));
    let o = offer(&good, "1.4.0");
    assert!(o.signature.is_some());
    assert_eq!(block_on(good.fetch(&o)).unwrap().bytes(), base.bytes());
    let v = block_on(good.fetch_variant(&o, "rookery", "x86_64")).unwrap();
    assert_eq!(v.variant().unwrap().key(), "x86_64");

    let wrong = StaticRegistry::dir(&f.reg, Some(key(9).verifying_key()));
    let o = offer(&wrong, "1.4.0");
    assert!(matches!(
        block_on(wrong.fetch(&o)),
        Err(Error::SignatureInvalid { .. })
    ));
    assert!(matches!(
        block_on(wrong.fetch_variant(&o, "rookery", "x86_64")),
        Err(Error::SignatureInvalid { .. })
    ));

    // A registry without a key does not look at signatures.
    let open = StaticRegistry::dir(&f.reg, None);
    let o = offer(&open, "1.4.0");
    block_on(open.fetch(&o)).unwrap();
}

#[test]
fn a_registry_with_a_key_refuses_unsigned_eggs() {
    let f = fixture();
    let (base, variants) = eggs(&package(f.tmp.path(), "1.4.0", "a"));
    publish_dir(&base, &f.reg, None, t(NOW)).unwrap();
    publish_dir(&variants[0], &f.reg, None, t(NOW)).unwrap();
    let reg = StaticRegistry::dir(&f.reg, Some(key(7).verifying_key()));
    let o = offer(&reg, "1.4.0");
    assert!(matches!(
        block_on(reg.fetch(&o)),
        Err(Error::SignatureInvalid { .. })
    ));
    let key_of = variants[0].variant().unwrap().key();
    assert!(matches!(
        block_on(reg.fetch_variant(&o, "rookery", &key_of)),
        Err(Error::SignatureInvalid { .. })
    ));
}

#[test]
fn a_signature_covers_the_publish_time() {
    let f = fixture();
    let (base, _) = eggs(&package(f.tmp.path(), "1.4.0", "a"));
    publish_dir(&base, &f.reg, Some(&key(7)), t(NOW)).unwrap();
    let path = f.reg.join("example/tool/index.json");
    let mut index = json(&path);
    index["versions"]["1.4.0"]["published"] = "2020-01-01T00:00:00Z".into();
    fs::write(&path, serde_json::to_vec(&index).unwrap()).unwrap();
    let reg = StaticRegistry::dir(&f.reg, Some(key(7).verifying_key()));
    let o = offer(&reg, "1.4.0");
    assert!(matches!(
        block_on(reg.fetch(&o)),
        Err(Error::SignatureInvalid { .. })
    ));
}

#[test]
fn a_variant_published_before_the_base_stays_invisible() {
    let f = fixture();
    let (base, variants) = eggs(&package(f.tmp.path(), "1.4.0", "a"));
    let x86 = variants
        .iter()
        .find(|v| v.variant().unwrap().key() == "x86_64")
        .unwrap();
    let p = publish_dir(x86, &f.reg, None, t(NOW)).unwrap();
    assert_eq!(p.file, "tool-1.4.0.rookery.x86_64.egg");

    let reg = StaticRegistry::dir(&f.reg, None);
    assert!(block_on(reg.versions(&name())).unwrap().is_empty());
    let index: Value = json(&f.reg.join("example/tool/index.json"));
    let v = &index["versions"]["1.4.0"];
    assert!(v.get("sha256").is_none());
    assert_eq!(
        v["targets"]["rookery"]["variants"][0]["sha256"],
        x86.sha256()
    );

    let later = "2026-11-11T00:00:00Z";
    publish_dir(&base, &f.reg, None, t(later)).unwrap();
    let o = offer(&reg, "1.4.0");
    assert_eq!(o.published, Some(t(later)));
    let infos = &o
        .details
        .as_ref()
        .unwrap()
        .target("rookery")
        .unwrap()
        .variants;
    assert_eq!(infos.len(), 2);
    let x = infos.iter().find(|v| v.key() == "x86_64").unwrap();
    assert_eq!(x.sha256.as_deref(), Some(x86.sha256()));
    assert_eq!(x.published, Some(t(NOW)));
    let a = infos.iter().find(|v| v.key() == "aarch64").unwrap();
    assert!(a.sha256.is_none());
    let got = block_on(reg.fetch_variant(&o, "rookery", "x86_64")).unwrap();
    assert_eq!(got.bytes(), x86.bytes());
}

#[test]
fn a_variant_can_be_added_to_a_released_version() {
    let f = fixture();
    let (base, variants) = eggs(&package(f.tmp.path(), "1.4.0", "a"));
    publish_dir(&base, &f.reg, None, t(NOW)).unwrap();
    let reg = StaticRegistry::dir(&f.reg, None);
    let o = offer(&reg, "1.4.0");
    assert!(matches!(
        block_on(reg.fetch_variant(&o, "rookery", "aarch64")),
        Err(Error::NotFound { .. })
    ));
    let arm = variants
        .iter()
        .find(|v| v.variant().unwrap().key() == "aarch64")
        .unwrap();
    publish_dir(arm, &f.reg, None, t("2026-11-12T00:00:00Z")).unwrap();
    // The base entry is untouched.
    let o2 = offer(&reg, "1.4.0");
    assert_eq!(o2.sha256, o.sha256);
    assert_eq!(o2.published, o.published);
    let got = block_on(reg.fetch_variant(&o2, "rookery", "aarch64")).unwrap();
    assert_eq!(got.bytes(), arm.bytes());
    // An offer made before the variant was published still finds it.
    let got = block_on(reg.fetch_variant(&o, "rookery", "aarch64")).unwrap();
    assert_eq!(got.bytes(), arm.bytes());
}

#[test]
fn publishing_the_same_bytes_again_changes_nothing() {
    let f = fixture();
    let dir = package(f.tmp.path(), "1.4.0", "a");
    let (base, variants) = eggs(&dir);
    let first = publish_dir(&base, &f.reg, Some(&key(7)), t(NOW)).unwrap();
    publish_dir(&variants[0], &f.reg, Some(&key(7)), t(NOW)).unwrap();
    let before = fs::read(f.reg.join("example/tool/index.json")).unwrap();

    let again = publish_dir(&base, &f.reg, Some(&key(7)), t("2026-12-01T00:00:00Z")).unwrap();
    assert_eq!(again, first);
    let v = publish_dir(
        &variants[0],
        &f.reg,
        Some(&key(7)),
        t("2026-12-01T00:00:00Z"),
    )
    .unwrap();
    assert_eq!(v.published, t(NOW));
    assert_eq!(
        fs::read(f.reg.join("example/tool/index.json")).unwrap(),
        before
    );
}

#[test]
fn publishing_different_bytes_for_a_version_is_refused() {
    let f = fixture();
    let (base, variants) = eggs(&package(f.tmp.path(), "1.4.0", "a"));
    publish_dir(&base, &f.reg, None, t(NOW)).unwrap();
    publish_dir(&variants[0], &f.reg, None, t(NOW)).unwrap();

    let (other, other_variants) = eggs(&package(f.tmp.path(), "1.4.0", "b"));
    assert_ne!(other.sha256(), base.sha256());
    assert!(matches!(
        publish_dir(&other, &f.reg, None, t(NOW)),
        Err(Error::Invalid { .. })
    ));
    let key_of = variants[0].variant().unwrap().key();
    let same_key = other_variants
        .iter()
        .find(|v| v.variant().unwrap().key() == key_of)
        .unwrap();
    // The variant's own files are unchanged, so the egg is the same; change them.
    let changed = {
        let dir = package(f.tmp.path(), "1.4.0", "c");
        fs::write(
            dir.join(format!("rookery/variants/{key_of}/bin")),
            "changed",
        )
        .unwrap();
        let (_, vs) = eggs(&dir);
        vs.into_iter()
            .find(|v| v.variant().unwrap().key() == key_of)
            .unwrap()
    };
    assert_ne!(changed.sha256(), variants[0].sha256());
    assert!(matches!(
        publish_dir(&changed, &f.reg, None, t(NOW)),
        Err(Error::Invalid { .. })
    ));
    let _ = same_key;
    // The registry still serves the first bytes.
    let reg = StaticRegistry::dir(&f.reg, None);
    let o = offer(&reg, "1.4.0");
    assert_eq!(block_on(reg.fetch(&o)).unwrap().bytes(), base.bytes());
}

#[test]
fn a_fat_egg_publishes_its_variants() {
    let f = fixture();
    let dir = package(f.tmp.path(), "1.4.0", "a");
    let (base, variants) = eggs(&dir);
    let fat = fat(&dir);
    assert!(fat.is_fat());
    let p = publish_dir(&fat, &f.reg, None, t(NOW)).unwrap();
    assert_eq!(p.sha256, base.sha256());
    let reg = StaticRegistry::dir(&f.reg, None);
    let o = offer(&reg, "1.4.0");
    assert_eq!(block_on(reg.fetch(&o)).unwrap().bytes(), base.bytes());
    for v in &variants {
        let k = v.variant().unwrap().key();
        let got = block_on(reg.fetch_variant(&o, "rookery", &k)).unwrap();
        assert_eq!(got.bytes(), v.bytes());
    }
}

#[test]
fn unknown_keys_survive_a_rewrite() {
    let f = fixture();
    let (base, _) = eggs(&package(f.tmp.path(), "1.4.0", "a"));
    publish_dir(&base, &f.reg, None, t(NOW)).unwrap();

    let cat = f.reg.join("index.json");
    let mut c = json(&cat);
    c["motd"] = "hello".into();
    fs::write(&cat, serde_json::to_vec(&c).unwrap()).unwrap();
    let idx = f.reg.join("example/tool/index.json");
    let mut i = json(&idx);
    i["owner"] = serde_json::json!({"team": "x"});
    i["about"]["badge"] = "gold".into();
    i["versions"]["1.4.0"]["note"] = "kept".into();
    i["versions"]["1.4.0"]["targets"]["crow"]["tag"] = 5.into();
    i["versions"]["1.4.0"]["targets"]["rookery"]["variants"][0]["extra"] = true.into();
    fs::write(&idx, serde_json::to_vec(&i).unwrap()).unwrap();

    // Another version and a withdrawal rewrite both files.
    let (next, _) = eggs(&package(f.tmp.path(), "1.5.0", "a"));
    publish_dir(&next, &f.reg, None, t(NOW)).unwrap();
    withdraw_dir(&f.reg, &name(), &Version::new(1, 4, 0), None).unwrap();

    let c = json(&cat);
    assert_eq!(c["motd"], "hello");
    let i = json(&idx);
    assert_eq!(i["owner"]["team"], "x");
    assert_eq!(i["about"]["badge"], "gold");
    assert_eq!(i["versions"]["1.4.0"]["note"], "kept");
    assert_eq!(i["versions"]["1.4.0"]["targets"]["crow"]["tag"], 5);
    assert_eq!(
        i["versions"]["1.4.0"]["targets"]["rookery"]["variants"][0]["extra"],
        true
    );
    assert!(i["versions"]["1.5.0"].is_object());
}

#[test]
fn the_catalog_lists_each_package_once_in_order() {
    let f = fixture();
    let (base, _) = eggs(&package(f.tmp.path(), "1.4.0", "a"));
    publish_dir(&base, &f.reg, None, t(NOW)).unwrap();
    let (next, _) = eggs(&package(f.tmp.path(), "1.5.0", "a"));
    publish_dir(&next, &f.reg, None, t(NOW)).unwrap();
    assert_eq!(
        json(&f.reg.join("index.json"))["packages"],
        serde_json::json!(["example/tool"])
    );
}

#[test]
fn withdrawing_marks_the_version() {
    let f = fixture();
    let (base, _) = eggs(&package(f.tmp.path(), "1.4.0", "a"));
    publish_dir(&base, &f.reg, None, t(NOW)).unwrap();
    withdraw_dir(&f.reg, &name(), &Version::new(1, 4, 0), Some("broken")).unwrap();
    let reg = StaticRegistry::dir(&f.reg, None);
    let o = offer(&reg, "1.4.0");
    assert_eq!(o.withdrawn.unwrap().reason.as_deref(), Some("broken"));
    assert!(matches!(
        withdraw_dir(&f.reg, &name(), &Version::new(9, 9, 9), None),
        Err(Error::NotFound { .. })
    ));
    assert!(matches!(
        withdraw_dir(
            &f.reg,
            &Name::parse("example/none").unwrap(),
            &Version::new(1, 0, 0),
            None
        ),
        Err(Error::NotFound { .. })
    ));
}

#[test]
fn a_changed_egg_file_fails_the_checksum() {
    let f = fixture();
    let (base, variants) = eggs(&package(f.tmp.path(), "1.4.0", "a"));
    publish_dir(&base, &f.reg, None, t(NOW)).unwrap();
    publish_dir(&variants[0], &f.reg, None, t(NOW)).unwrap();
    let reg = StaticRegistry::dir(&f.reg, None);
    let o = offer(&reg, "1.4.0");
    let dir = f.reg.join("example/tool");
    let k = variants[0].variant().unwrap().key();

    fs::write(dir.join("tool-1.4.0.egg"), b"not the egg").unwrap();
    match block_on(reg.fetch(&o)) {
        Err(Error::ChecksumMismatch {
            expected, actual, ..
        }) => {
            assert_eq!(expected, base.sha256());
            assert_ne!(actual, expected);
        }
        other => panic!("{other:?}"),
    }
    fs::write(dir.join(variants[0].file_name()), b"not the egg").unwrap();
    assert!(matches!(
        block_on(reg.fetch_variant(&o, "rookery", &k)),
        Err(Error::ChecksumMismatch { .. })
    ));
}

#[test]
fn a_missing_egg_file_is_not_found() {
    let f = fixture();
    let (base, _) = eggs(&package(f.tmp.path(), "1.4.0", "a"));
    publish_dir(&base, &f.reg, None, t(NOW)).unwrap();
    fs::remove_file(f.reg.join("example/tool/tool-1.4.0.egg")).unwrap();
    let reg = StaticRegistry::dir(&f.reg, None);
    let o = offer(&reg, "1.4.0");
    assert!(matches!(
        block_on(reg.fetch(&o)),
        Err(Error::NotFound { .. })
    ));
}

#[test]
fn an_egg_for_another_package_is_refused() {
    let f = fixture();
    let (base, _) = eggs(&package(f.tmp.path(), "1.4.0", "a"));
    publish_dir(&base, &f.reg, None, t(NOW)).unwrap();
    // Another package's egg, put where tool 1.4.0 is, with the index
    // pointing at its checksum.
    let other_dir = f.tmp.path().join("other");
    common::package(
        &other_dir,
        &MANIFEST
            .replace("VERSION", "1.4.0")
            .replace("id tool", "id other"),
        &[
            ("assets/icon.svg", "<svg/>"),
            ("crow/plugin.json", "x"),
            ("rookery/rookery.json", "{}"),
        ],
    );
    let other = pack(
        &other_dir,
        &PackOptions {
            base: true,
            ..options()
        },
    )
    .unwrap()
    .pop()
    .unwrap();
    let dir = f.reg.join("example/tool");
    fs::write(dir.join("tool-1.4.0.egg"), other.bytes()).unwrap();
    let idx = dir.join("index.json");
    let mut i = json(&idx);
    i["versions"]["1.4.0"]["sha256"] = other.sha256().into();
    fs::write(&idx, serde_json::to_vec(&i).unwrap()).unwrap();

    let reg = StaticRegistry::dir(&f.reg, None);
    let o = offer(&reg, "1.4.0");
    assert!(matches!(
        block_on(reg.fetch(&o)),
        Err(Error::Invalid { .. })
    ));
}

#[test]
fn an_index_for_another_package_is_refused() {
    let f = fixture();
    let (base, _) = eggs(&package(f.tmp.path(), "1.4.0", "a"));
    publish_dir(&base, &f.reg, None, t(NOW)).unwrap();
    let dir = f.reg.join("example/tool");
    let mut i = json(&dir.join("index.json"));
    i["id"] = "other".into();
    fs::write(dir.join("index.json"), serde_json::to_vec(&i).unwrap()).unwrap();
    let reg = StaticRegistry::dir(&f.reg, None);
    assert!(matches!(
        block_on(reg.versions(&name())),
        Err(Error::Invalid { .. })
    ));
    assert!(matches!(
        block_on(reg.index(&name())),
        Err(Error::Invalid { .. })
    ));
}

/// A `Fetch` that serves URLs under a prefix from a folder, and counts what
/// it was asked.
struct FolderFetch {
    prefix: String,
    root: PathBuf,
    token: Option<String>,
    seen: Mutex<Vec<(String, Option<String>)>>,
}

impl Fetch for FolderFetch {
    fn get<'a>(
        &'a self,
        url: &'a Url,
        auth: Option<&'a Token>,
    ) -> BoxFuture<'a, hatchery::Result<Option<Bytes>>> {
        Box::pin(async move {
            let secret = auth.map(|t| t.secret().to_string());
            self.seen
                .lock()
                .unwrap()
                .push((url.to_string(), secret.clone()));
            if self.token.is_some() && secret != self.token {
                return Err(Error::Http {
                    url: url.to_string(),
                    message: "401 unauthorized".into(),
                });
            }
            let Some(rel) = url.as_str().strip_prefix(&self.prefix) else {
                return Ok(None);
            };
            match fs::read(self.root.join(rel)) {
                Ok(b) => Ok(Some(Bytes::from(b))),
                Err(_) => Ok(None),
            }
        })
    }
}

#[test]
fn an_http_registry_reads_through_the_fetch() {
    let f = fixture();
    let (base, variants) = eggs(&package(f.tmp.path(), "1.4.0", "a"));
    publish_dir(&base, &f.reg, Some(&key(7)), t(NOW)).unwrap();
    publish_dir(&variants[0], &f.reg, Some(&key(7)), t(NOW)).unwrap();
    let fetch = Arc::new(FolderFetch {
        prefix: "https://example.org/reg/".into(),
        root: f.reg.clone(),
        token: Some("s3cret".into()),
        seen: Mutex::new(Vec::new()),
    });
    let reg = StaticRegistry::http(
        Url::parse("https://example.org/reg/").unwrap(),
        fetch.clone(),
        Some(Token::new("s3cret")),
        Some(key(7).verifying_key()),
    );
    assert_eq!(reg.label(), "https://example.org/reg");
    assert!(!reg.is_local());
    assert_eq!(block_on(reg.names()).unwrap(), vec![name()]);
    let o = offer(&reg, "1.4.0");
    let egg = block_on(reg.fetch(&o)).unwrap();
    assert_eq!(egg.bytes(), base.bytes());
    let k = variants[0].variant().unwrap().key();
    block_on(reg.fetch_variant(&o, "rookery", &k)).unwrap();

    let seen = fetch.seen.lock().unwrap();
    assert!(seen.iter().all(|(_, t)| t.as_deref() == Some("s3cret")));
    assert!(
        seen.iter()
            .any(|(u, _)| u == "https://example.org/reg/index.json")
    );
    assert!(
        seen.iter()
            .any(|(u, _)| u == "https://example.org/reg/example/tool/tool-1.4.0.egg")
    );
    drop(seen);

    // Without the token the server refuses.
    let anon = StaticRegistry::http(
        Url::parse("https://example.org/reg").unwrap(),
        fetch.clone(),
        None,
        None,
    );
    assert_eq!(anon.label(), "https://example.org/reg");
    assert!(matches!(block_on(anon.names()), Err(Error::Http { .. })));

    // A missing file is None, not an error.
    let missing = StaticRegistry::http(
        Url::parse("https://example.org/elsewhere/").unwrap(),
        fetch,
        Some(Token::new("s3cret")),
        None,
    );
    assert!(block_on(missing.names()).unwrap().is_empty());
    assert!(block_on(missing.versions(&name())).unwrap().is_empty());
}

#[test]
fn registry_paths_cannot_leave_the_registry() {
    let f = fixture();
    let files = hatchery::source::registry::DirFiles::new(&f.reg);
    assert!(files.is_local());
    assert!(block_on(files.read("../x")).is_err());
    assert!(block_on(files.read("/etc/passwd")).is_err());
    assert!(block_on(files.read("a/index.json")).unwrap().is_none());
}

#[cfg(feature = "http")]
#[test]
fn http_fetch_gets_bodies_and_maps_404() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let mut auths = Vec::new();
        for _ in 0..3 {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = s.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let req = String::from_utf8_lossy(&buf).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
            let auth = req
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                .map(|l| l.split_once(':').unwrap().1.trim().to_string());
            auths.push(auth);
            let reply: &[u8] = match path.as_str() {
                "/hello" => b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                "/boom" => b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                _ => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            };
            s.write_all(reply).unwrap();
        }
        auths
    });

    let fetch = hatchery::fetch::HttpFetch::new();
    let base = format!("http://127.0.0.1:{port}");
    let hello = Url::parse(&format!("{base}/hello")).unwrap();
    let body = block_on(fetch.get(&hello, Some(&Token::new("tok")))).unwrap();
    assert_eq!(body.as_deref(), Some(&b"hello"[..]));
    let gone = Url::parse(&format!("{base}/gone")).unwrap();
    assert!(block_on(fetch.get(&gone, None)).unwrap().is_none());
    let boom = Url::parse(&format!("{base}/boom")).unwrap();
    assert!(matches!(
        block_on(fetch.get(&boom, None)),
        Err(Error::Http { .. })
    ));
    let auths = server.join().unwrap();
    assert_eq!(auths[0].as_deref(), Some("Bearer tok"));
    assert_eq!(auths[1], None);
}

#[test]
fn a_folder_source_packs_what_is_there() {
    let f = fixture();
    let dir = package(f.tmp.path(), "1.4.0", "a");
    let src = FolderSource::new(&dir, Vec::new());
    assert!(src.is_local());
    assert_eq!(src.label(), dir.display().to_string());
    assert_eq!(block_on(src.names()).unwrap(), vec![name()]);
    assert!(
        block_on(src.versions(&Name::parse("example/other").unwrap()))
            .unwrap()
            .is_empty()
    );
    let o = offer(&src, "1.4.0");
    assert!(o.built);
    assert!(o.sha256.is_none());
    assert_eq!(o.details.as_ref().unwrap().targets.len(), 2);

    let egg = block_on(src.fetch(&o)).unwrap();
    assert!(egg.variant().is_none());
    assert_eq!(egg.manifest().name(), name());
    assert!(egg.target("crow").is_some());
    let v = block_on(src.fetch_variant(&o, "rookery", "aarch64")).unwrap();
    let h = v.variant().unwrap();
    assert_eq!(
        (h.target.as_str(), h.key().as_str()),
        ("rookery", "aarch64")
    );
    assert!(matches!(
        block_on(src.fetch_variant(&o, "rookery", "riscv64")),
        Err(Error::NotFound { .. })
    ));

    // Edits show on the next call.
    fs::write(dir.join("crow/plugin.json"), "edited").unwrap();
    let again = block_on(src.fetch(&o)).unwrap();
    assert_ne!(again.sha256(), egg.sha256());
}

#[test]
fn a_folder_without_a_manifest_is_an_error() {
    let f = fixture();
    let dir = f.tmp.path().join("empty");
    fs::create_dir_all(&dir).unwrap();
    let src = FolderSource::new(&dir, Vec::new());
    assert!(block_on(src.names()).is_err());
}

#[test]
fn a_file_source_serves_the_egg_it_was_given() {
    let f = fixture();
    let dir = package(f.tmp.path(), "1.4.0", "a");
    let (base, _) = eggs(&dir);
    let path = f.tmp.path().join("tool-1.4.0.egg");
    fs::write(&path, base.bytes()).unwrap();

    let src = FileSource::new(path.display().to_string(), None);
    assert!(src.is_local());
    assert_eq!(block_on(src.names()).unwrap(), vec![name()]);
    let o = offer(&src, "1.4.0");
    assert_eq!(o.sha256.as_deref(), Some(base.sha256()));
    assert_eq!(
        o.details.as_ref().unwrap().about.name.as_deref(),
        Some("Tool")
    );
    let egg = block_on(src.fetch(&o)).unwrap();
    assert_eq!(egg.bytes(), base.bytes());
    assert!(matches!(
        block_on(src.fetch_variant(&o, "rookery", "x86_64")),
        Err(Error::NotFound { .. })
    ));

    // The egg is read once.
    fs::remove_file(&path).unwrap();
    block_on(src.names()).unwrap();

    let mut wrong = o.clone();
    wrong.version = Version::new(2, 0, 0);
    assert!(matches!(
        block_on(src.fetch(&wrong)),
        Err(Error::Invalid { .. })
    ));
}

#[test]
fn a_file_source_takes_variants_from_a_fat_egg() {
    let f = fixture();
    let dir = package(f.tmp.path(), "1.4.0", "a");
    let (base, variants) = eggs(&dir);
    let fat = fat(&dir);
    let path = f.tmp.path().join("fat.egg");
    fs::write(&path, fat.bytes()).unwrap();

    let src = FileSource::new(path.display().to_string(), None);
    let o = offer(&src, "1.4.0");
    // The checksum is that of the base egg inside.
    assert_eq!(o.sha256.as_deref(), Some(base.sha256()));
    let egg = block_on(src.fetch(&o)).unwrap();
    assert!(egg.is_fat());
    let v = block_on(src.fetch_variant(&o, "rookery", "x86_64")).unwrap();
    let expected = variants
        .iter()
        .find(|v| v.variant().unwrap().key() == "x86_64")
        .unwrap();
    assert_eq!(v.bytes(), expected.bytes());
    assert!(matches!(
        block_on(src.fetch_variant(&o, "rookery", "riscv64")),
        Err(Error::NotFound { .. })
    ));
}

#[test]
fn a_file_source_reads_urls_through_the_fetch() {
    let f = fixture();
    let (base, _) = eggs(&package(f.tmp.path(), "1.4.0", "a"));
    let served = f.tmp.path().join("served");
    fs::create_dir_all(&served).unwrap();
    fs::write(served.join("tool-1.4.0.egg"), base.bytes()).unwrap();

    let none = FileSource::new("https://example.org/tool-1.4.0.egg", None);
    assert!(!none.is_local());
    let err = block_on(none.names()).unwrap_err();
    assert!(err.to_string().contains("Fetch"), "{err}");

    let fetch = Arc::new(FolderFetch {
        prefix: "https://example.org/".into(),
        root: served,
        token: None,
        seen: Mutex::new(Vec::new()),
    });
    let src = FileSource::new("https://example.org/tool-1.4.0.egg", Some(fetch.clone()));
    assert_eq!(src.label(), "https://example.org/tool-1.4.0.egg");
    let o = offer(&src, "1.4.0");
    block_on(src.fetch(&o)).unwrap();
    assert_eq!(fetch.seen.lock().unwrap().len(), 1);

    let missing = FileSource::new("https://example.org/gone.egg", Some(fetch));
    assert!(matches!(
        block_on(missing.names()),
        Err(Error::NotFound { .. })
    ));
}

#[test]
fn a_variant_egg_file_is_not_a_package() {
    let f = fixture();
    let (_, variants) = eggs(&package(f.tmp.path(), "1.4.0", "a"));
    let path = f.tmp.path().join("v.egg");
    fs::write(&path, variants[0].bytes()).unwrap();
    let src = FileSource::new(path.display().to_string(), None);
    assert!(matches!(block_on(src.names()), Err(Error::Invalid { .. })));
}

#[test]
fn offers_keep_the_index_order_data() {
    // The details an offer carries come from the index alone.
    let f = fixture();
    let (base, _) = eggs(&package(f.tmp.path(), "1.4.0", "a"));
    publish_dir(&base, &f.reg, None, t(NOW)).unwrap();
    let reg = StaticRegistry::dir(&f.reg, None);
    let o = offer(&reg, "1.4.0");
    let d = o.details.unwrap();
    let crow = d.target("crow").unwrap();
    assert_eq!(crow.dependencies.len(), 1);
    assert_eq!(
        crow.dependencies[0].0,
        Name::parse("example/helper").unwrap()
    );
    assert_eq!(crow.requires[0].fact, "crow");
    let by_key: BTreeMap<_, _> = d
        .target("rookery")
        .unwrap()
        .variants
        .iter()
        .map(|v| (v.key(), v.sha256.clone()))
        .collect();
    assert_eq!(by_key.len(), 2);
}
