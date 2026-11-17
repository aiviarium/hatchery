//! Git repositories as sources, against local repositories in temp folders.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{git, git_commit, git_init, package, write};
use hatchery::git::Git;
use hatchery::pack::{PackOptions, pack};
use hatchery::publish::Published;
use hatchery::sign::{SigningKey, VerifyingKey};
use hatchery::source::git::publish_branch;
use hatchery::source::{GitFolder, GitRegistry, Offer, Source};
use hatchery::{Egg, Error, Name, Timestamp, block_on};

const PACKAGES_KDL: &str = r#"
format 1

defaults {
    package {
        group example
        version "@V@"
    }
    about {
        license MIT
        repository "https://example.org/parrot.git"
    }
}

packages {
    find "agents/*"
    package "shared/base" tag="base-v{version}"
}

release {
    tag "v{version}"
    eggs branch="eggs"
    withdrawn alpha "1.1.0" reason="broke sign-in"
}
"#;

const BASE_KDL: &str = r#"
format 1
package {
    id base
    version "0.3.0"
}
about {
    name "Base"
}
targets {
    rookery {
        variants {
            variant arch=x86_64
            variant arch=aarch64
        }
    }
}
"#;

struct Fixture {
    _tmp: tempfile::TempDir,
    /// The bare repository everything is pushed to.
    remote: PathBuf,
    work: PathBuf,
    cache: PathBuf,
    url: String,
}

fn alpha() -> Name {
    Name::parse("example/alpha").unwrap()
}

fn beta() -> Name {
    Name::parse("example/beta").unwrap()
}

fn base() -> Name {
    Name::parse("example/base").unwrap()
}

impl Fixture {
    /// A repository with two packages in `agents/` and one in `shared/`,
    /// released as v1.0.0, v1.1.0 and v1.2.0, and the base package as
    /// base-v0.3.0.
    fn new() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let remote = tmp.path().join("remote.git");
        fs::create_dir_all(&remote).unwrap();
        git(&remote, &["init", "-q", "--bare", "-b", "main"]);
        // Like GitHub: filters are honoured, so blobs come on demand.
        git(&remote, &["config", "uploadpack.allowFilter", "true"]);
        let work = tmp.path().join("work");
        git_init(&work);
        git(
            &work,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        let f = Fixture {
            url: remote.to_str().unwrap().to_string(),
            cache: tmp.path().join("cache"),
            remote,
            work,
            _tmp: tmp,
        };
        package(&f.work.join("shared/base"), BASE_KDL, &[]);
        write(&f.work, "shared/base/rookery/rookery.json", "{}");
        write(&f.work, "shared/base/rookery/variants/x86_64/bin", "x86");
        write(&f.work, "shared/base/rookery/variants/aarch64/bin", "arm");
        f.release("1.0.0");
        git(&f.work, &["tag", "base-v0.3.0"]);
        git(&f.work, &["push", "-q", "origin", "base-v0.3.0"]);
        f.release("1.1.0");
        f.release("1.2.0");
        git(&f.work, &["tag", "nightly"]);
        git(&f.work, &["tag", "vx.y"]);
        git(&f.work, &["push", "-q", "origin", "nightly", "vx.y"]);
        f
    }

    /// Writes the package files for a version, commits, tags `v<version>`
    /// and pushes.
    fn release(&self, version: &str) -> String {
        self.commit_version(version);
        git(&self.work, &["tag", &format!("v{version}")]);
        git(
            &self.work,
            &["push", "-q", "origin", "main", &format!("v{version}")],
        );
        git(&self.work, &["rev-parse", "HEAD"])
    }

    fn commit_version(&self, version: &str) -> String {
        write(
            &self.work,
            "packages.kdl",
            PACKAGES_KDL.replace("@V@", version),
        );
        package(
            &self.work.join("agents/alpha"),
            &format!(
                "format 1\npackage {{\n id alpha\n version \"{version}\"\n}}\n\
                 about {{\n name \"Alpha\"\n summary \"The alpha package\"\n}}\n\
                 targets {{\n crow {{\n requires {{\n crow \"^1\"\n }}\n }}\n}}\n"
            ),
            &[("crow/plugin.json", &format!("{{\"v\":\"{version}\"}}"))],
        );
        package(
            &self.work.join("agents/beta"),
            "format 1\npackage {\n id beta\n}\n\
             targets {\n crow {\n build \"echo building\"\n }\n}\n",
            &[("crow/host.js", &format!("// {version}"))],
        );
        git_commit(&self.work, &format!("release {version}"))
    }

    fn registry(&self) -> GitRegistry {
        GitRegistry::new(&self.url, &self.cache, Git::new(), None)
    }

    fn keyed(&self, key: &VerifyingKey) -> GitRegistry {
        GitRegistry::new(&self.url, &self.cache, Git::new(), Some(*key))
    }

    fn tag_commit(&self, tag: &str) -> String {
        git(&self.work, &["rev-parse", &format!("{tag}^{{commit}}")])
    }

    fn cache_folders(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = fs::read_dir(&self.cache)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.is_dir())
            .collect();
        v.sort();
        v
    }
}

fn offer<'a>(offers: &'a [Offer], version: &str) -> &'a Offer {
    offers
        .iter()
        .find(|o| o.version.to_string() == version)
        .unwrap_or_else(|| panic!("no offer {version} in {offers:?}"))
}

fn versions(s: &impl Source, name: &Name) -> Vec<Offer> {
    let mut v = block_on(s.versions(name)).unwrap();
    v.sort_by(|a, b| a.version.cmp(&b.version));
    v
}

fn now() -> Timestamp {
    Timestamp::parse("2026-11-10T09:12:00Z").unwrap()
}

fn signing_key() -> SigningKey {
    SigningKey::from_bytes(&[7; 32])
}

/// Packs a package folder into its base egg, and variant eggs when it has
/// variants.
fn pack_all(dir: &Path) -> Vec<Egg> {
    pack(
        dir,
        &PackOptions {
            no_build: true,
            ..Default::default()
        },
    )
    .unwrap()
}

/// A package folder of a built alpha, with a file the source does not have.
fn built_alpha(root: &Path, version: &str) -> Egg {
    let dir = root.join(format!("built-alpha-{version}"));
    package(
        &dir,
        &format!(
            "format 1\npackage {{\n group example\n id alpha\n version \"{version}\"\n}}\n\
             targets {{\n crow {{\n requires {{\n crow \"^1\"\n }}\n }}\n}}\n"
        ),
        &[
            ("crow/plugin.json", "{\"built\":true}"),
            ("crow/dist/built.js", "// bundled"),
        ],
    );
    pack_all(&dir).pop().unwrap()
}

fn publish(f: &Fixture, egg: &Egg, key: Option<&SigningKey>) -> Published {
    publish_branch(egg, &f.url, "eggs", None, key, now(), &Git::new()).unwrap()
}

#[test]
fn lists_the_packages_of_the_root_manifest() {
    let f = Fixture::new();
    let mut names = block_on(f.registry().names()).unwrap();
    names.sort();
    assert_eq!(names, vec![alpha(), base(), beta()]);
    assert_eq!(f.registry().label(), f.url);
    assert!(f.registry().is_local());
}

#[test]
fn a_repository_without_a_root_manifest_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("plain");
    git_init(&work);
    write(&work, "README.md", "hi");
    git_commit(&work, "first");
    let reg = GitRegistry::new(
        work.to_str().unwrap(),
        tmp.path().join("cache"),
        Git::new(),
        None,
    );
    let err = block_on(reg.names()).unwrap_err();
    assert!(matches!(err, Error::Invalid { .. }), "{err}");
    assert!(err.to_string().contains("packages.kdl"), "{err}");
}

#[test]
fn versions_come_from_tags_that_match_the_pattern() {
    let f = Fixture::new();
    let reg = f.registry();

    let a = versions(&reg, &alpha());
    let shown: Vec<String> = a.iter().map(|o| o.version.to_string()).collect();
    assert_eq!(shown, ["1.0.0", "1.1.0", "1.2.0"]);
    for o in &a {
        assert!(!o.built);
        assert!(o.details.is_none());
        assert_eq!(o.registry, f.url);
        assert_eq!(o.path.as_deref(), Some("agents/alpha"));
        assert_eq!(
            o.git_ref.as_deref(),
            Some(format!("v{}", o.version).as_str())
        );
        assert_eq!(
            o.commit.as_deref(),
            Some(f.tag_commit(o.git_ref.as_ref().unwrap()).as_str())
        );
    }

    // The package with its own pattern sees only its own tags.
    let b = versions(&reg, &base());
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].version.to_string(), "0.3.0");
    assert_eq!(b[0].git_ref.as_deref(), Some("base-v0.3.0"));
    assert_eq!(b[0].path.as_deref(), Some("shared/base"));

    assert!(versions(&reg, &Name::parse("example/none").unwrap()).is_empty());
}

#[test]
fn withdrawn_versions_come_from_the_root_manifest() {
    let f = Fixture::new();
    let a = versions(&f.registry(), &alpha());
    assert_eq!(
        offer(&a, "1.1.0")
            .withdrawn
            .as_ref()
            .unwrap()
            .reason
            .as_deref(),
        Some("broke sign-in")
    );
    assert!(offer(&a, "1.0.0").withdrawn.is_none());
    // The line names alpha only.
    let b = versions(&f.registry(), &beta());
    assert!(b.iter().all(|o| o.withdrawn.is_none()));
}

#[test]
fn a_new_tag_shows_up_in_the_next_call() {
    let f = Fixture::new();
    let reg = f.registry();
    assert_eq!(versions(&reg, &alpha()).len(), 3);
    f.release("1.3.0");
    let a = versions(&reg, &alpha());
    assert_eq!(a.len(), 4);
    assert_eq!(
        offer(&a, "1.3.0").commit.as_deref(),
        Some(f.tag_commit("v1.3.0").as_str())
    );
}

#[test]
fn details_read_the_manifest_at_the_tag() {
    let f = Fixture::new();
    let reg = f.registry();
    let a = versions(&reg, &alpha());
    let d = block_on(reg.details(offer(&a, "1.0.0"))).unwrap();
    assert_eq!(d.about.name.as_deref(), Some("Alpha"));
    assert_eq!(d.targets.len(), 1);
    assert!(!d.target("crow").unwrap().build);
    assert_eq!(d.target("crow").unwrap().requires.len(), 1);

    // The version and the group come from packages.kdl at that tag.
    let b = versions(&reg, &beta());
    let d = block_on(reg.details(offer(&b, "1.1.0"))).unwrap();
    assert!(d.target("crow").unwrap().build);
    assert_eq!(d.about.licenses, ["MIT"]);
}

#[test]
fn a_tag_whose_manifest_has_another_version_is_an_error() {
    let f = Fixture::new();
    // The manifest says 1.2.0, the tag says 9.9.9.
    git(&f.work, &["tag", "v9.9.9"]);
    git(&f.work, &["push", "-q", "origin", "v9.9.9"]);
    let reg = f.registry();
    let a = versions(&reg, &alpha());
    let err = block_on(reg.details(offer(&a, "9.9.9"))).unwrap_err();
    assert!(matches!(err, Error::Invalid { .. }), "{err}");
    let text = err.to_string();
    assert!(text.contains("9.9.9") && text.contains("1.2.0"), "{text}");
    // Fetching refuses it too.
    assert!(block_on(reg.fetch(offer(&a, "9.9.9"))).is_err());
}

#[test]
fn fetches_a_package_from_source_with_a_shallow_clone() {
    let f = Fixture::new();
    let reg = f.registry();
    let a = versions(&reg, &alpha());
    let o = offer(&a, "1.1.0");

    let egg = block_on(reg.fetch(o)).unwrap();
    let m = egg.manifest();
    assert_eq!(m.name(), alpha());
    assert_eq!(m.package.version.to_string(), "1.1.0");
    let origin = m.origin.as_ref().unwrap();
    assert_eq!(origin.commit, o.commit);
    assert_eq!(origin.repository.as_deref(), Some(f.url.as_str()));
    assert_eq!(origin.hatchery.as_deref(), Some(hatchery::VERSION));
    assert!(origin.built.is_some());
    // The license comes from the defaults of that commit.
    assert_eq!(m.about.licenses, ["MIT"]);
    assert!(
        egg.entries()
            .iter()
            .any(|e| e.path == "crow/plugin.json" && e.data.as_ref() == br#"{"v":"1.1.0"}"#)
    );

    // Same tag, same bytes.
    let again = block_on(reg.fetch(o)).unwrap();
    assert_eq!(again.sha256(), egg.sha256());

    // One cache folder, shallow, with this package's folder only.
    let folders = f.cache_folders();
    assert_eq!(folders.len(), 1, "{folders:?}");
    let cache = &folders[0];
    assert_eq!(cache.file_name().unwrap().len(), 16);
    assert_eq!(
        git(cache, &["rev-parse", "--is-shallow-repository"]),
        "true"
    );
    // Nothing is checked out: files are read from the objects.
    assert!(!cache.join("agents").exists());
    assert!(!cache.join("packages.kdl").exists());

    // The next package of the repository reuses the folder.
    let b = versions(&reg, &beta());
    let beta_egg = block_on(reg.fetch(offer(&b, "1.2.0"))).unwrap();
    assert_eq!(beta_egg.manifest().name(), beta());
    assert_eq!(beta_egg.manifest().package.version.to_string(), "1.2.0");
    assert_eq!(f.cache_folders(), folders);
}

#[test]
fn fetches_one_variant_from_source() {
    let f = Fixture::new();
    let reg = f.registry();
    let b = versions(&reg, &base());
    let o = &b[0];
    let d = block_on(reg.details(o)).unwrap();
    assert_eq!(d.target("rookery").unwrap().variants.len(), 2);

    let v = block_on(reg.fetch_variant(o, "rookery", "aarch64")).unwrap();
    let h = v.variant().unwrap();
    assert_eq!(h.target, "rookery");
    assert_eq!(h.arch.as_deref(), Some("aarch64"));
    assert!(
        v.entries()
            .iter()
            .any(|e| e.path == "rookery/variants/aarch64/bin" && e.data.as_ref() == b"arm")
    );
    assert!(!v.entries().iter().any(|e| e.path.contains("x86_64")));
    let again = block_on(reg.fetch_variant(o, "rookery", "aarch64")).unwrap();
    assert_eq!(again.sha256(), v.sha256());

    // The base egg has the shared files only.
    let base_egg = block_on(reg.fetch(o)).unwrap();
    assert!(base_egg.variant().is_none());
    assert!(
        !base_egg
            .entries()
            .iter()
            .any(|e| e.path.contains("variants"))
    );

    assert!(block_on(reg.fetch_variant(o, "rookery", "riscv")).is_err());
}

#[test]
fn a_moved_tag_is_refused() {
    let f = Fixture::new();
    let reg = f.registry();
    let a = versions(&reg, &alpha());
    let o = offer(&a, "1.0.0").clone();
    let old = o.commit.clone().unwrap();

    // Move the tag to a commit that changes the package.
    write(&f.work, "agents/alpha/crow/plugin.json", "{\"evil\":true}");
    let new = git_commit(&f.work, "move");
    git(&f.work, &["tag", "-f", "v1.0.0"]);
    git(&f.work, &["push", "-q", "-f", "origin", "v1.0.0"]);

    let err = block_on(reg.fetch(&o)).unwrap_err();
    match err {
        Error::TagMoved {
            tag, locked, now, ..
        } => {
            assert_eq!(tag, "v1.0.0");
            assert_eq!(locked, old);
            assert_eq!(now, new);
        }
        e => panic!("{e}"),
    }
    let err = block_on(reg.fetch_variant(&o, "crow", "default")).unwrap_err();
    assert!(matches!(err, Error::TagMoved { .. }), "{err}");

    // A deleted tag is the same.
    git(&f.work, &["push", "-q", "origin", ":refs/tags/v1.1.0"]);
    let gone = offer(&a, "1.1.0");
    assert!(matches!(
        block_on(reg.fetch(gone)).unwrap_err(),
        Error::TagMoved { .. }
    ));
}

#[test]
fn built_eggs_are_looked_up_first() {
    let f = Fixture::new();
    let tmp = tempfile::tempdir().unwrap();
    let key = signing_key();
    let reg = f.keyed(&key.verifying_key());

    // Before anything is published, everything is source.
    assert!(versions(&reg, &alpha()).iter().all(|o| !o.built));

    let egg = built_alpha(tmp.path(), "1.0.0");
    let p = publish(&f, &egg, Some(&key));
    assert!(p.signed);
    assert_eq!(p.sha256, egg.sha256());

    let a = versions(&reg, &alpha());
    assert_eq!(a.len(), 3);
    let built = offer(&a, "1.0.0");
    assert!(built.built);
    assert_eq!(built.sha256.as_deref(), Some(egg.sha256()));
    assert!(built.signature.is_some());
    assert_eq!(built.published, Some(now()));
    // The tag still gives the commit, the ref and the folder.
    assert_eq!(
        built.commit.as_deref(),
        Some(f.tag_commit("v1.0.0").as_str())
    );
    assert_eq!(built.git_ref.as_deref(), Some("v1.0.0"));
    assert_eq!(built.path.as_deref(), Some("agents/alpha"));
    assert_eq!(built.registry, f.url);
    let d = built.details.as_ref().unwrap();
    assert!(d.target("crow").is_some());
    // The other tags have no egg.
    let src = offer(&a, "1.1.0");
    assert!(!src.built);
    assert!(src.details.is_none());
    assert!(src.withdrawn.is_some());

    // The details of a built offer are the offer's own.
    assert_eq!(block_on(reg.details(built)).unwrap(), *d);

    // Fetching gives the published egg, not the source of the tag.
    let got = block_on(reg.fetch(built)).unwrap();
    assert_eq!(got.sha256(), egg.sha256());
    assert!(got.entries().iter().any(|e| e.path == "crow/dist/built.js"));

    // The source of another version still comes from its tag.
    let from_tag = block_on(reg.fetch(src)).unwrap();
    assert!(
        !from_tag
            .entries()
            .iter()
            .any(|e| e.path == "crow/dist/built.js")
    );
    assert_eq!(from_tag.manifest().package.version.to_string(), "1.1.0");
}

#[test]
fn a_built_egg_with_the_wrong_key_is_refused() {
    let f = Fixture::new();
    let tmp = tempfile::tempdir().unwrap();
    let key = signing_key();
    publish(&f, &built_alpha(tmp.path(), "1.0.0"), Some(&key));

    let wrong = SigningKey::from_bytes(&[9; 32]).verifying_key();
    let reg = f.keyed(&wrong);
    let result = block_on(async {
        let offers = reg.versions(&alpha()).await?;
        reg.fetch(offer(&offers, "1.0.0")).await
    });
    let err = result.unwrap_err();
    assert!(matches!(err, Error::SignatureInvalid { .. }), "{err}");

    // An unsigned egg is refused by a registry with a key as well.
    let g = Fixture::new();
    publish(&g, &built_alpha(tmp.path(), "1.0.0"), None);
    let reg = g.keyed(&key.verifying_key());
    let result = block_on(async {
        let offers = reg.versions(&alpha()).await?;
        reg.fetch(offer(&offers, "1.0.0")).await
    });
    assert!(result.is_err());
}

#[test]
fn built_versions_without_a_tag_are_offered_too() {
    let f = Fixture::new();
    let tmp = tempfile::tempdir().unwrap();
    let egg = built_alpha(tmp.path(), "7.0.0");
    publish(&f, &egg, None);
    let reg = f.registry();
    let a = versions(&reg, &alpha());
    assert_eq!(a.len(), 4);
    let o = offer(&a, "7.0.0");
    assert!(o.built);
    assert!(o.commit.is_none());
    assert!(o.git_ref.is_none());
    assert_eq!(block_on(reg.fetch(o)).unwrap().sha256(), egg.sha256());
}

#[test]
fn a_version_is_visible_when_its_base_egg_is_published() {
    let f = Fixture::new();
    let tmp = tempfile::tempdir().unwrap();
    let key = signing_key();
    let reg = f.keyed(&key.verifying_key());

    let dir = tmp.path().join("base");
    // The same package with its group, for packing outside the repository.
    write(
        &dir,
        "package.kdl",
        BASE_KDL.replace("id base", "group example\n    id base"),
    );
    write(&dir, "rookery/rookery.json", "{}");
    write(&dir, "rookery/variants/x86_64/bin", "x86");
    write(&dir, "rookery/variants/aarch64/bin", "arm");
    let eggs = pack_all(&dir);
    assert_eq!(eggs.len(), 3);
    let (variants, base_egg): (Vec<&Egg>, Vec<&Egg>) =
        eggs.iter().partition(|e| e.variant().is_some());
    let base_egg = base_egg[0];

    // The variants first: the version is not offered as built yet.
    for v in &variants {
        publish(&f, v, Some(&key));
    }
    let b = versions(&reg, &base());
    assert_eq!(b.len(), 1);
    assert!(!b[0].built);

    // The base egg makes it visible, with the variants' checksums.
    publish(&f, base_egg, Some(&key));
    let b = versions(&reg, &base());
    assert_eq!(b.len(), 1);
    let o = &b[0];
    assert!(o.built);
    assert_eq!(o.sha256.as_deref(), Some(base_egg.sha256()));
    assert_eq!(
        o.commit.as_deref(),
        Some(f.tag_commit("base-v0.3.0").as_str())
    );
    let info = &o
        .details
        .as_ref()
        .unwrap()
        .target("rookery")
        .unwrap()
        .variants;
    assert_eq!(info.len(), 2);
    assert!(
        info.iter()
            .all(|v| v.sha256.is_some() && v.signature.is_some())
    );

    let arm = variants
        .iter()
        .find(|v| v.variant().unwrap().arch.as_deref() == Some("aarch64"))
        .unwrap();
    let got = block_on(reg.fetch_variant(o, "rookery", "aarch64")).unwrap();
    assert_eq!(got.sha256(), arm.sha256());
    assert_eq!(block_on(reg.fetch(o)).unwrap().sha256(), base_egg.sha256());

    // The commit messages say what was published.
    let log = git(&f.remote, &["log", "--format=%s", "eggs"]);
    assert!(
        log.contains("Publish example/base 0.3.0 rookery.aarch64"),
        "{log}"
    );
    assert!(
        log.lines().any(|l| l == "Publish example/base 0.3.0"),
        "{log}"
    );
}

#[test]
fn publishing_from_parallel_jobs_retries_a_rejected_push() {
    let f = Fixture::new();
    let tmp = tempfile::tempdir().unwrap();
    let eggs: Vec<Egg> = ["1.0.0", "1.1.0", "1.2.0", "1.3.0", "1.4.0"]
        .iter()
        .map(|v| built_alpha(tmp.path(), v))
        .collect();
    let url = f.url.clone();
    let handles: Vec<_> = eggs
        .iter()
        .cloned()
        .map(|egg| {
            let url = url.clone();
            std::thread::spawn(move || {
                publish_branch(&egg, &url, "eggs", None, None, now(), &Git::new()).unwrap()
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let reg = f.registry();
    let a = versions(&reg, &alpha());
    for e in &eggs {
        let v = e.manifest().package.version.to_string();
        assert_eq!(offer(&a, &v).sha256.as_deref(), Some(e.sha256()), "{v}");
    }
    // Five commits on a branch with one history.
    assert_eq!(git(&f.remote, &["rev-list", "--count", "eggs"]), "5");
}

#[test]
fn publishing_the_same_egg_again_is_a_no_op() {
    let f = Fixture::new();
    let tmp = tempfile::tempdir().unwrap();
    let egg = built_alpha(tmp.path(), "1.0.0");
    let first = publish(&f, &egg, None);
    let second = publish(&f, &egg, None);
    assert_eq!(first.sha256, second.sha256);
    assert_eq!(git(&f.remote, &["rev-list", "--count", "eggs"]), "1");
}

#[test]
fn publishes_into_a_folder_of_the_branch() {
    let f = Fixture::new();
    let tmp = tempfile::tempdir().unwrap();
    let egg = built_alpha(tmp.path(), "1.0.0");
    publish_branch(
        &egg,
        &f.url,
        "eggs",
        Some("registry"),
        None,
        now(),
        &Git::new(),
    )
    .unwrap();
    let listing = git(&f.remote, &["ls-tree", "-r", "--name-only", "eggs"]);
    assert!(
        listing.contains("registry/example/alpha/alpha-1.0.0.egg"),
        "{listing}"
    );
}

// GitFolder

struct Folder {
    f: Fixture,
}

impl Folder {
    fn source(&self, git_ref: Option<&str>) -> GitFolder {
        GitFolder::new(
            &self.f.url,
            Some("agents/alpha".into()),
            git_ref.map(str::to_string),
            &self.f.cache,
            Git::new(),
        )
    }
}

#[test]
fn a_folder_at_a_tag() {
    let f = Fixture::new();
    let s = GitFolder::new(
        &f.url,
        Some("agents/alpha".into()),
        Some("v1.1.0".into()),
        &f.cache,
        Git::new(),
    );
    assert_eq!(s.label(), f.url);
    assert_eq!(block_on(s.names()).unwrap(), vec![alpha()]);
    let o = versions(&s, &alpha());
    assert_eq!(o.len(), 1);
    let o = &o[0];
    assert_eq!(o.version.to_string(), "1.1.0");
    assert_eq!(o.commit.as_deref(), Some(f.tag_commit("v1.1.0").as_str()));
    assert_eq!(o.git_ref.as_deref(), Some("v1.1.0"));
    assert_eq!(o.path.as_deref(), Some("agents/alpha"));
    assert!(!o.built);
    assert_eq!(
        o.details.as_ref().unwrap().about.name.as_deref(),
        Some("Alpha")
    );
    assert!(versions(&s, &beta()).is_empty());

    let egg = block_on(s.fetch(o)).unwrap();
    assert_eq!(egg.manifest().package.version.to_string(), "1.1.0");
    assert_eq!(egg.manifest().origin.as_ref().unwrap().commit, o.commit);
    assert_eq!(block_on(s.fetch(o)).unwrap().sha256(), egg.sha256());

    // The same folder from the registry's source path is the same egg.
    let reg = f.registry();
    let a = versions(&reg, &alpha());
    assert_eq!(
        block_on(reg.fetch(offer(&a, "1.1.0"))).unwrap().sha256(),
        egg.sha256()
    );

    // A moved tag is refused.
    write(&f.work, "agents/alpha/crow/plugin.json", "{}");
    git_commit(&f.work, "move");
    git(&f.work, &["tag", "-f", "v1.1.0"]);
    git(&f.work, &["push", "-q", "-f", "origin", "v1.1.0"]);
    assert!(matches!(
        block_on(s.fetch(o)).unwrap_err(),
        Error::TagMoved { .. }
    ));
}

#[test]
fn a_folder_without_a_ref_follows_the_default_branch() {
    let f = Fixture::new();
    let folder = Folder { f };
    let s = folder.source(None);
    let o = versions(&s, &alpha()).pop().unwrap();
    assert_eq!(o.git_ref.as_deref(), Some("main"));
    assert_eq!(o.version.to_string(), "1.2.0");
    assert_eq!(
        o.commit.as_deref(),
        Some(git(&folder.f.work, &["rev-parse", "main"]).as_str())
    );
}

#[test]
fn a_branch_resolves_again_and_a_pinned_commit_still_fetches() {
    let f = Fixture::new();
    let folder = Folder { f };
    let f = &folder.f;
    let s = folder.source(Some("main"));

    let first = versions(&s, &alpha()).pop().unwrap();
    assert_eq!(first.version.to_string(), "1.2.0");
    let old_egg = block_on(s.fetch(&first)).unwrap();

    // The branch moves on: a new version of the package.
    let new_commit = f.commit_version("1.3.0");
    git(&f.work, &["push", "-q", "origin", "main"]);

    let second = versions(&s, &alpha()).pop().unwrap();
    assert_eq!(second.version.to_string(), "1.3.0");
    assert_eq!(second.commit.as_deref(), Some(new_commit.as_str()));
    assert_eq!(second.git_ref.as_deref(), Some("main"));
    let new_egg = block_on(s.fetch(&second)).unwrap();
    assert_eq!(new_egg.manifest().package.version.to_string(), "1.3.0");

    // The old offer is the lock file's pin: it gives the old content, and
    // a moved branch is not an error.
    let again = block_on(s.fetch(&first)).unwrap();
    assert_eq!(again.sha256(), old_egg.sha256());
    assert_eq!(again.manifest().package.version.to_string(), "1.2.0");
    assert_ne!(old_egg.sha256(), new_egg.sha256());
}

#[test]
fn a_folder_at_a_commit_id() {
    let f = Fixture::new();
    // An older commit that is not the tip of any branch.
    let commit = f.tag_commit("v1.0.0");
    let s = GitFolder::new(
        &f.url,
        Some("agents/alpha".into()),
        Some(commit.clone()),
        &f.cache,
        Git::new(),
    );
    let o = versions(&s, &alpha()).pop().unwrap();
    assert_eq!(o.version.to_string(), "1.0.0");
    assert_eq!(o.commit.as_deref(), Some(commit.as_str()));
    assert_eq!(o.git_ref.as_deref(), Some(commit.as_str()));
    let egg = block_on(s.fetch(&o)).unwrap();
    assert_eq!(egg.manifest().package.version.to_string(), "1.0.0");

    // Pinned fetch from a fresh cache: the commit is fetched by its id.
    let fresh = tempfile::tempdir().unwrap();
    let s2 = GitFolder::new(
        &f.url,
        Some("agents/alpha".into()),
        Some("main".into()),
        fresh.path(),
        Git::new(),
    );
    let mut pinned = o.clone();
    pinned.git_ref = Some("main".into());
    let again = block_on(s2.fetch(&pinned)).unwrap();
    assert_eq!(again.sha256(), egg.sha256());
}

#[test]
fn an_unknown_ref_is_an_error_and_a_folder_may_be_the_root() {
    let f = Fixture::new();
    let s = GitFolder::new(
        &f.url,
        Some("agents/alpha".into()),
        Some("nope".into()),
        &f.cache,
        Git::new(),
    );
    assert!(block_on(s.names()).is_err());

    // A repository that is one package, with no packages.kdl.
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("one");
    git_init(&work);
    package(
        &work,
        "format 1\npackage {\n group example\n id solo\n version \"0.1.0\"\n}\n\
         targets {\n crow {\n }\n}\n",
        &[("crow/plugin.json", "{}")],
    );
    git_commit(&work, "first");
    let s = GitFolder::new(
        work.to_str().unwrap(),
        None,
        None,
        tmp.path().join("cache"),
        Git::new(),
    );
    let o = versions(&s, &Name::parse("example/solo").unwrap())
        .pop()
        .unwrap();
    assert_eq!(o.path, None);
    let egg = block_on(s.fetch(&o)).unwrap();
    assert!(egg.entries().iter().any(|e| e.path == "crow/plugin.json"));
    assert!(!egg.entries().iter().any(|e| e.path.starts_with(".git")));
}

#[test]
fn symlinks_in_a_package_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("linked");
    git_init(&work);
    package(
        &work,
        "format 1\npackage {\n group example\n id linked\n version \"0.1.0\"\n}\n\
         targets {\n crow {\n }\n}\n",
        &[("crow/plugin.json", "{}")],
    );
    std::os::unix::fs::symlink("/etc/passwd", work.join("crow/passwd")).unwrap();
    git_commit(&work, "first");
    let s = GitFolder::new(
        work.to_str().unwrap(),
        None,
        None,
        tmp.path().join("cache"),
        Git::new(),
    );
    let o = versions(&s, &Name::parse("example/linked").unwrap())
        .pop()
        .unwrap();
    let err = block_on(s.fetch(&o)).unwrap_err();
    assert!(err.to_string().contains("symlink"), "{err}");
}

#[test]
fn the_git_wrapper_reports_failures_and_lists_remotes() {
    let f = Fixture::new();
    let g = Git::new();
    let r = g.ls_remote(&f.url).unwrap();
    assert_eq!(r.default_branch.as_deref(), Some("main"));
    assert!(r.tags.contains_key("v1.0.0"));
    assert!(r.tags.contains_key("nightly"));
    assert_eq!(r.tags["v1.2.0"], f.tag_commit("v1.2.0"));

    // An annotated tag is peeled to its commit.
    git(&f.work, &["tag", "-a", "-m", "note", "v2.0.0"]);
    git(&f.work, &["push", "-q", "origin", "v2.0.0"]);
    let r = g.ls_remote(&f.url).unwrap();
    assert_eq!(r.tags["v2.0.0"], git(&f.work, &["rev-parse", "HEAD"]));

    let err = g.ls_remote("/no/such/repository").unwrap_err();
    match err {
        Error::Git { args, message } => {
            assert!(args.starts_with("ls-remote"));
            assert!(!message.is_empty());
        }
        e => panic!("{e}"),
    }
}

#[test]
fn addresses_and_refs_that_look_like_options_are_refused() {
    let f = Fixture::new();
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("PWNED");
    let evil = format!("--upload-pack=touch {}", marker.display());
    let g = Git::new();

    assert!(g.ls_remote(&evil).is_err());
    assert!(g.cache(&f.cache, &evil).is_err());
    assert!(g.ls_remote("ssh://host/\nrepo").is_err());
    let reg = GitRegistry::new(&evil, &f.cache, Git::new(), None);
    assert!(block_on(reg.names()).is_err());
    let folder = GitFolder::new(&evil, None, None, &f.cache, Git::new());
    assert!(block_on(folder.names()).is_err());
    let egg = built_alpha(tmp.path(), "1.0.0");
    assert!(publish_branch(&egg, &evil, "eggs", None, None, now(), &g).is_err());
    assert!(publish_branch(&egg, &f.url, "--foo", None, None, now(), &g).is_err());
    assert!(publish_branch(&egg, &f.url, "eggs", Some("../x"), None, now(), &g).is_err());
    assert!(!marker.exists());

    // Refs, commits and paths that reach git are checked.
    let cache = g.cache(&f.cache, &f.url).unwrap();
    for bad in ["--foo", "a..b", "x y", "-", "", "a:b", "x.lock", "/x"] {
        assert!(cache.fetch_tag(bad).is_err(), "{bad:?}");
        assert!(cache.fetch_branch(bad).is_err(), "{bad:?}");
    }
    let head = f.tag_commit("v1.0.0");
    assert!(cache.fetch_commit("HEAD; touch x").is_err());
    assert!(cache.fetch_commit("--all").is_err());
    assert!(cache.show("HEAD; x", "packages.kdl").is_err());
    assert!(!cache.has_commit("HEAD"));
    cache.fetch_tag("v1.0.0").unwrap();
    assert!(cache.show(&head, "../x").is_err());
    assert!(cache.show(&head, "/etc/passwd").is_err());
    assert!(
        cache
            .export(&head, Some("../x"), &tmp.path().join("out"))
            .is_err()
    );
    drop(cache);

    // A ref that is not a tag, branch or commit is an error, not an option.
    let s = GitFolder::new(&f.url, None, Some("--foo".into()), &f.cache, Git::new());
    assert!(block_on(s.names()).is_err());
}

#[test]
fn remote_helpers_and_plain_http_are_refused() {
    let f = Fixture::new();
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("PWNED");
    let ext = format!("ext::sh -c touch% {}", marker.display());
    let g = Git::new();

    assert!(matches!(
        g.ls_remote(&ext).unwrap_err(),
        Error::Invalid { .. }
    ));
    assert!(g.cache(&f.cache, &ext).is_err());
    let reg = GitRegistry::new(&ext, &f.cache, Git::new(), None);
    assert!(block_on(reg.names()).is_err());
    let egg = built_alpha(tmp.path(), "1.0.0");
    assert!(publish_branch(&egg, &ext, "eggs", None, None, now(), &g).is_err());
    for url in ["http://127.0.0.1:1/x.git", "fd::17", "ftp://example.org/x"] {
        assert!(g.ls_remote(url).is_err(), "{url}");
    }

    // Git itself refuses the helper too, were an address to get past the check.
    let err = g.run(None, &["ls-remote", "--", &ext]).unwrap_err();
    assert!(err.to_string().contains("not allowed"), "{err}");
    assert!(!marker.exists());
}

#[test]
fn credentials_in_addresses_do_not_reach_eggs_or_messages() {
    let f = Fixture::new();
    let secret = "https://ci:ghp_SECRET@127.0.0.1:1/x.git";
    let err = Git::new().ls_remote(secret).unwrap_err();
    assert!(!err.to_string().contains("ghp_SECRET"), "{err}");
    assert!(matches!(err, Error::Git { .. }));
    let reg = GitRegistry::new(secret, &f.cache, Git::new(), None);
    let err = block_on(reg.names()).unwrap_err();
    assert!(!format!("{err} {err:?}").contains("ghp_SECRET"), "{err}");

    // The origin of an egg packed from a repository with a user in its
    // address names the address without it.
    let with_user = format!("file://ci:ghp_SECRET@{}", f.url);
    let s = GitFolder::new(
        &with_user,
        Some("agents/alpha".into()),
        Some("v1.0.0".into()),
        &f.cache,
        Git::new(),
    );
    match block_on(s.names()) {
        Ok(_) => {
            let o = versions(&s, &alpha()).pop().unwrap();
            let egg = block_on(s.fetch(&o)).unwrap();
            let repo = egg
                .manifest()
                .origin
                .as_ref()
                .unwrap()
                .repository
                .clone()
                .unwrap();
            assert!(!repo.contains("ghp_SECRET"), "{repo}");
            assert!(!egg.manifest().to_kdl().contains("ghp_SECRET"));
        }
        Err(e) => assert!(!e.to_string().contains("ghp_SECRET"), "{e}"),
    }
}

#[test]
fn a_moved_tag_is_refused_for_built_eggs_too() {
    let f = Fixture::new();
    let tmp = tempfile::tempdir().unwrap();
    publish(&f, &built_alpha(tmp.path(), "1.0.0"), None);
    let reg = f.registry();
    let a = versions(&reg, &alpha());
    let o = offer(&a, "1.0.0").clone();
    assert!(o.built);
    block_on(reg.fetch(&o)).unwrap();

    write(&f.work, "agents/alpha/crow/plugin.json", "{}");
    git_commit(&f.work, "move");
    git(&f.work, &["tag", "-f", "v1.0.0"]);
    git(&f.work, &["push", "-q", "-f", "origin", "v1.0.0"]);
    assert!(matches!(
        block_on(reg.fetch(&o)).unwrap_err(),
        Error::TagMoved { .. }
    ));
}

#[test]
fn a_folder_says_whether_its_ref_is_a_tag() {
    let f = Fixture::new();
    let is_tag = |r: Option<&str>| {
        let s = GitFolder::new(
            &f.url,
            Some("agents/alpha".into()),
            r.map(str::to_string),
            &f.cache,
            Git::new(),
        );
        block_on(s.ref_is_tag()).unwrap()
    };
    assert!(is_tag(Some("v1.0.0")));
    assert!(!is_tag(Some("main")));
    assert!(!is_tag(None));
    assert!(!is_tag(Some(&f.tag_commit("v1.0.0"))));
}

#[test]
fn a_lock_file_left_behind_does_not_block_and_stays() {
    let f = Fixture::new();
    let g = Git::new();
    let held = g.cache(&f.cache, &f.url).unwrap();
    let lock = fs::read_dir(&f.cache)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "lock"))
        .unwrap();
    drop(held);
    // The file is not removed, and a file that nobody holds is no lock.
    assert!(lock.exists());
    drop(g.cache(&f.cache, &f.url).unwrap());
    assert!(lock.exists());
}

/// Commits a text file with LF endings, an executable and attributes that
/// would convert line endings in a checkout.
fn commit_attributes(f: &Fixture) -> String {
    write(&f.work, ".gitattributes", "* text=auto eol=crlf\n");
    write(&f.work, "agents/alpha/crow/text.txt", "one\ntwo\n");
    let run = write(&f.work, "agents/alpha/crow/run.sh", "#!/bin/sh\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&run, fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(not(unix))]
    git(
        &f.work,
        &[
            "update-index",
            "--add",
            "--chmod=+x",
            "agents/alpha/crow/run.sh",
        ],
    );
    let _ = run;
    let commit = git_commit(&f.work, "attributes");
    git(&f.work, &["push", "-q", "origin", "main"]);
    commit
}

#[test]
fn an_export_is_the_committed_bytes_whatever_the_configuration() {
    let f = Fixture::new();
    let commit = commit_attributes(&f);
    let cache = Git::new().cache(&f.cache, &f.url).unwrap();
    // A partial clone, as with a large repository: git sends the blobs of
    // a shallow fetch anyway, so this fetch is not shallow.
    git(
        cache.dir(),
        &[
            "-c",
            "protocol.file.allow=always",
            "-c",
            "protocol.version=2",
            "fetch",
            "-q",
            "--filter=blob:none",
            "--no-tags",
            "origin",
            "+refs/heads/main:refs/remotes/origin/main",
        ],
    );
    // Nothing of the file is there yet.
    let blob = git(&f.work, &["rev-parse", "HEAD:agents/alpha/crow/text.txt"]);
    let listed = git(
        cache.dir(),
        &["rev-list", "--objects", "--missing=print", &commit],
    );
    assert!(listed.contains(&format!("?{blob}")), "{listed}");

    let tmp = tempfile::tempdir().unwrap();
    let plain = tmp.path().join("plain");
    cache.export(&commit, Some("agents/alpha"), &plain).unwrap();
    assert_eq!(
        fs::read(plain.join("crow/text.txt")).unwrap(),
        b"one\ntwo\n"
    );

    git(cache.dir(), &["config", "core.eol", "crlf"]);
    git(cache.dir(), &["config", "core.autocrlf", "true"]);
    let converting = tmp.path().join("converting");
    cache
        .export(&commit, Some("agents/alpha"), &converting)
        .unwrap();
    assert_eq!(
        fs::read(converting.join("crow/text.txt")).unwrap(),
        b"one\ntwo\n"
    );
    assert_eq!(
        fs::read(converting.join("crow/run.sh")).unwrap(),
        b"#!/bin/sh\n"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o111;
        assert_ne!(mode(&converting.join("crow/run.sh")), 0);
        assert_eq!(mode(&converting.join("crow/text.txt")), 0);
    }

    // The whole repository too, with no `.git` in it.
    let whole = tmp.path().join("whole");
    cache.export(&commit, None, &whole).unwrap();
    assert!(whole.join(".gitattributes").is_file());
    assert!(!whole.join(".git").exists());
    assert!(whole.join("agents/beta/crow/host.js").is_file());
}

#[test]
fn symlinks_and_submodules_are_refused_by_an_export() {
    let f = Fixture::new();
    let tmp = tempfile::tempdir().unwrap();
    let cache = Git::new().cache(&f.cache, &f.url).unwrap();

    git(
        &f.work,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{},agents/alpha/sub", "1".repeat(40)),
        ],
    );
    // Not `git_commit`: its `add -A` would drop the entry, as the folder is
    // not there.
    git(&f.work, &["commit", "-q", "-m", "submodule"]);
    let commit = git(&f.work, &["rev-parse", "HEAD"]);
    git(&f.work, &["push", "-q", "origin", "main"]);
    cache.fetch_branch("main").unwrap();
    let err = cache
        .export(&commit, Some("agents/alpha"), &tmp.path().join("a"))
        .unwrap_err();
    assert!(err.to_string().contains("submodule"), "{err}");
    // A folder next to it is fine.
    cache
        .export(&commit, Some("agents/beta"), &tmp.path().join("b"))
        .unwrap();
    // A file is not a folder, and a missing folder is not one either.
    for p in ["agents/alpha/package.kdl", "agents/none"] {
        let err = cache
            .export(&commit, Some(p), &tmp.path().join("c"))
            .unwrap_err();
        assert!(err.to_string().contains("not a folder"), "{err}");
    }
}
