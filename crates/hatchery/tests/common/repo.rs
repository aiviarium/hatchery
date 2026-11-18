//! A git repository with several packages and releases, for the tests of
//! git registries and of the git cache.

use std::fs;
use std::path::{Path, PathBuf};

use hatchery::pack::{PackOptions, pack};
use hatchery::publish::Published;
use hatchery::sign::{SigningKey, VerifyingKey};
use hatchery::source::git::publish_branch;
use hatchery::source::{GitRegistry, Offer, Source};
use hatchery::{Egg, Name, Timestamp, block_on};

use super::{git, git_commit, git_init, isolated_git, package, write};

pub const PACKAGES_KDL: &str = r#"
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

pub const BASE_KDL: &str = r#"
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

pub struct Fixture {
    pub tmp: tempfile::TempDir,
    /// The bare repository everything is pushed to.
    pub remote: PathBuf,
    pub work: PathBuf,
    pub cache: PathBuf,
    pub url: String,
}

pub fn alpha() -> Name {
    Name::parse("example/alpha").unwrap()
}

pub fn beta() -> Name {
    Name::parse("example/beta").unwrap()
}

pub fn base() -> Name {
    Name::parse("example/base").unwrap()
}

impl Fixture {
    /// A repository with two packages in `agents/` and one in `shared/`,
    /// released as v1.0.0, v1.1.0 and v1.2.0, and the base package as
    /// base-v0.3.0.
    pub fn new() -> Fixture {
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
            tmp,
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
    pub fn release(&self, version: &str) -> String {
        self.commit_version(version);
        git(&self.work, &["tag", &format!("v{version}")]);
        git(
            &self.work,
            &["push", "-q", "origin", "main", &format!("v{version}")],
        );
        git(&self.work, &["rev-parse", "HEAD"])
    }

    pub fn commit_version(&self, version: &str) -> String {
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

    pub fn registry(&self) -> GitRegistry {
        GitRegistry::new(&self.url, &self.cache, isolated_git(), None)
    }

    pub fn keyed(&self, key: &VerifyingKey) -> GitRegistry {
        GitRegistry::new(&self.url, &self.cache, isolated_git(), Some(*key))
    }

    pub fn tag_commit(&self, tag: &str) -> String {
        git(&self.work, &["rev-parse", &format!("{tag}^{{commit}}")])
    }

    pub fn cache_folders(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = fs::read_dir(&self.cache)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.is_dir())
            .collect();
        v.sort();
        v
    }
}

pub fn offer<'a>(offers: &'a [Offer], version: &str) -> &'a Offer {
    offers
        .iter()
        .find(|o| o.version.to_string() == version)
        .unwrap_or_else(|| panic!("no offer {version} in {offers:?}"))
}

pub fn versions(s: &impl Source, name: &Name) -> Vec<Offer> {
    let mut v = block_on(s.versions(name)).unwrap();
    v.sort_by(|a, b| a.version.cmp(&b.version));
    v
}

pub fn now() -> Timestamp {
    Timestamp::parse("2026-11-10T09:12:00Z").unwrap()
}

pub fn signing_key() -> SigningKey {
    SigningKey::from_bytes(&[7; 32])
}

/// Packs a package folder into its base egg, and variant eggs when it has
/// variants.
pub fn pack_all(dir: &Path) -> Vec<Egg> {
    pack(dir, &PackOptions::new().no_build(true)).unwrap()
}

/// A package folder of a built alpha, with a file the source does not have.
pub fn built_alpha(root: &Path, version: &str) -> Egg {
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

pub fn publish(f: &Fixture, egg: &Egg, key: Option<&SigningKey>) -> Published {
    publish_branch(egg, &f.url, "eggs", None, key, now(), &isolated_git()).unwrap()
}

/// Commits a text file with LF endings, an executable and attributes that
/// would convert line endings in a checkout.
pub fn commit_attributes(f: &Fixture) -> String {
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
