//! The client against registries made of folders: add, install from the
//! lock, update, remove, variants, files and listings.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use hatchery::publish::publish_dir;
use hatchery::time::Clock;
use hatchery::{
    Client, Config, Error, Name, PackOptions, Platform, Spec, StaticTarget, Timestamp, Variants,
    block_on, pack, tree_digest,
};
use semver::Version;

const NOW: &str = "2026-11-17T00:00:00Z";

fn manifest(id: &str, version: &str, crow_deps: &[(&str, &str)], rookery: bool) -> String {
    let deps: String = crow_deps
        .iter()
        .map(|(n, r)| format!("      {n} \"{r}\"\n"))
        .collect();
    let rookery = if rookery {
        "  rookery {\n    variants {\n      variant arch=x86_64\n      variant arch=aarch64\n    }\n  }\n"
    } else {
        ""
    };
    format!(
        "format 1\npackage {{\n  group demo\n  id {id}\n  version \"{version}\"\n}}\n\
         targets {{\n  crow {{\n    dependencies {{\n{deps}    }}\n  }}\n{rookery}}}\n"
    )
}

/// Packs a folder and publishes every egg it makes into a registry folder.
fn publish(
    reg: &Path,
    work: &Path,
    id: &str,
    version: &str,
    deps: &[(&str, &str)],
    rookery: bool,
) -> Vec<hatchery::Egg> {
    let dir = work.join(format!("{id}-{version}"));
    let mut files = vec![("crow/plugin.json", "{}")];
    if rookery {
        files.push(("rookery/tasks.yml", "tasks"));
        files.push(("rookery/variants/x86_64/bin", "x86"));
        files.push(("rookery/variants/aarch64/bin", "arm"));
    }
    common::package(&dir, &manifest(id, version, deps, rookery), &files);
    let opts = PackOptions {
        no_build: true,
        ..PackOptions::default()
    };
    let eggs = pack(&dir, &opts).unwrap();
    let now = Timestamp::parse(NOW).unwrap();
    for e in &eggs {
        publish_dir(e, reg, None, now).unwrap();
    }
    eggs
}

fn clock() -> Clock {
    common::clock_at(NOW)
}

fn client_for(reg: &Path, cache: &Path, target: StaticTarget, extra: &str) -> Client {
    let config = Config::parse(&format!(
        "{extra}\nregistries {{\n  dir \"{}\"\n}}\n",
        reg.display()
    ))
    .unwrap();
    Client::builder()
        .git(common::isolated_git())
        .config(config)
        .target(target)
        .clock(clock())
        .cache_dir(cache)
        .build()
        .unwrap()
}

fn crow() -> StaticTarget {
    StaticTarget::new("crow").fact("crow", "1.2.0")
}

struct Env {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Env {
    fn new() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        Env { _tmp: tmp, root }
    }
    fn reg(&self) -> PathBuf {
        self.root.join("registry")
    }
    fn work(&self) -> PathBuf {
        self.root.join("work")
    }
    fn dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
    fn client(&self) -> Client {
        client_for(&self.reg(), &self.root.join("cache"), crow(), "")
    }
}

fn name(s: &str) -> Name {
    Name::parse(s).unwrap()
}

fn spec(s: &str) -> Spec {
    Spec::parse(s).unwrap()
}

#[test]
fn add_installs_dependencies_first_and_writes_the_lock() {
    let env = Env::new();
    publish(&env.reg(), &env.work(), "b", "1.0.0", &[], false);
    publish(&env.reg(), &env.work(), "b", "1.2.0", &[], false);
    publish(&env.reg(), &env.work(), "b", "2.0.0", &[], false);
    publish(&env.reg(), &env.work(), "a", "1.0.0", &[("b", "^1")], false);
    let dir = env.dir("plugins");

    let report = block_on(env.client().add(&spec("demo/a"), &dir)).unwrap();
    let names: Vec<String> = report.packages.iter().map(|c| c.name.to_string()).collect();
    assert_eq!(names, ["demo/b", "demo/a"]);
    assert_eq!(report.packages[0].version, Version::new(1, 2, 0));
    assert!(report.packages[0].previous.is_none());

    assert!(dir.join("demo/a/package.kdl").is_file());
    assert!(dir.join("demo/a/crow/plugin.json").is_file());
    assert!(dir.join("demo/b/crow/plugin.json").is_file());

    let lock = fs::read_to_string(dir.join("eggs.lock")).unwrap();
    assert!(lock.contains("name = \"demo/a\""), "{lock}");
    assert!(lock.contains("version = \"1.2.0\""), "{lock}");
    assert!(lock.contains("targets = [\"crow\"]"), "{lock}");
    assert!(lock.contains("registry ="), "{lock}");
    assert!(lock.find("demo/a").unwrap() < lock.find("demo/b").unwrap());

    let installed = env.client().installed(&dir).unwrap();
    assert_eq!(installed.len(), 2);
    assert_eq!(installed[0].manifest.package.id, "a");
    assert_eq!(installed[0].sha256, report.packages[1].sha256);

    // The id alone works when one group has it.
    publish(&env.reg(), &env.work(), "a", "1.1.0", &[("b", "^1")], false);
    let again = block_on(env.client().add(&spec("a"), &dir)).unwrap();
    assert_eq!(again.packages.len(), 1);
    assert_eq!(again.packages[0].previous, Some(Version::new(1, 0, 0)));
}

#[test]
fn the_installed_version_is_not_fetched_again() {
    let env = Env::new();
    publish(&env.reg(), &env.work(), "a", "1.0.0", &[], false);
    let dir = env.dir("p");
    let client = env.client();
    block_on(client.add(&spec("demo/a@1.0.0"), &dir)).unwrap();
    let lock = fs::read_to_string(dir.join("eggs.lock")).unwrap();
    let installed = fs::read(dir.join("demo/a/package.kdl")).unwrap();

    // The registry now serves other bytes under the same version.
    let egg_path = env.reg().join("demo/a/a-1.0.0.egg");
    let mut bytes = fs::read(&egg_path).unwrap();
    bytes.extend_from_slice(b"other");
    fs::write(&egg_path, bytes).unwrap();

    let report = block_on(client.add(&spec("demo/a@1.0.0"), &dir)).unwrap();
    assert!(report.packages.is_empty());
    assert_eq!(fs::read_to_string(dir.join("eggs.lock")).unwrap(), lock);
    assert_eq!(fs::read(dir.join("demo/a/package.kdl")).unwrap(), installed);
}

#[test]
fn an_installed_version_that_is_withdrawn_stays_and_warns() {
    let env = Env::new();
    publish(&env.reg(), &env.work(), "a", "1.0.0", &[], false);
    publish(&env.reg(), &env.work(), "a", "1.1.0", &[], false);
    let dir = env.dir("p");
    let client = env.client();
    block_on(client.add(&spec("demo/a"), &dir)).unwrap();
    hatchery::publish::withdraw_dir(
        &env.reg(),
        &name("demo/a"),
        &Version::new(1, 1, 0),
        Some("broken"),
    )
    .unwrap();

    let report = block_on(client.update(None, &dir)).unwrap();
    assert!(report.packages.is_empty());
    assert!(
        report.warnings.iter().any(|w| w.contains("withdrawn")),
        "{:?}",
        report.warnings
    );
    assert_eq!(
        client.installed(&dir).unwrap()[0].version,
        Version::new(1, 1, 0)
    );

    publish(&env.reg(), &env.work(), "a", "1.2.0", &[], false);
    let report = block_on(client.update(None, &dir)).unwrap();
    assert_eq!(report.packages[0].version, Version::new(1, 2, 0));
}

#[test]
fn an_index_that_disagrees_with_the_egg_is_refused() {
    let env = Env::new();
    publish(&env.reg(), &env.work(), "b", "1.0.0", &[], false);
    publish(&env.reg(), &env.work(), "a", "1.0.0", &[("b", "^1")], false);
    let path = env.reg().join("demo/a/index.json");
    let mut json: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    json["versions"]["1.0.0"]["targets"]["crow"]["dependencies"] = serde_json::json!({});
    fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    let dir = env.dir("p");
    let err = block_on(env.client().add(&spec("demo/a"), &dir)).unwrap_err();
    assert!(err.to_string().contains("index does not match"), "{err}");
    assert!(!dir.join("demo").exists());
}

#[test]
fn a_folder_can_be_installed_again_from_the_lock() {
    let env = Env::new();
    let src = env.work().join("dev");
    common::package(
        &src,
        &manifest("dev", "0.1.0", &[], false),
        &[("crow/plugin.json", "{}")],
    );
    let client = Client::builder()
        .git(common::isolated_git())
        .target(crow())
        .clock(clock())
        .cache_dir(env.root.join("cache"))
        .build()
        .unwrap();
    let first = env.dir("first");
    block_on(client.add(&Spec::Folder(src.clone()), &first)).unwrap();
    // Change the folder's content: the checksum is not a pin for folders.
    fs::write(src.join("crow/plugin.json"), "{\"changed\":true}").unwrap();
    let second = env.dir("second");
    fs::create_dir_all(&second).unwrap();
    fs::copy(first.join("eggs.lock"), second.join("eggs.lock")).unwrap();
    let report = block_on(client.install(&second)).unwrap();
    assert_eq!(report.packages.len(), 1);
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("checksum changed")),
        "{:?}",
        report.warnings
    );
    let lock = fs::read_to_string(second.join("eggs.lock")).unwrap();
    assert!(lock.contains(report.packages[0].sha256.as_str()));
}

#[test]
fn a_variant_that_differs_from_the_lock_is_refused() {
    let env = Env::new();
    publish(&env.reg(), &env.work(), "v", "1.0.0", &[], true);
    let client = client_for(
        &env.reg(),
        &env.root.join("cache"),
        StaticTarget::new("rookery").variants(Variants::OnDemand),
        "",
    );
    let dir = env.dir("p");
    block_on(client.add(&spec("demo/v"), &dir)).unwrap();
    let pkg = client.installed(&dir).unwrap().remove(0);
    let plat = Platform::new("linux", "x86_64");
    let folder = block_on(client.variant(&pkg, &plat)).unwrap();
    let lock = fs::read_to_string(dir.join("eggs.lock")).unwrap();
    let sha = lock
        .lines()
        .find_map(|l| l.strip_prefix("\"rookery.x86_64\" = \""))
        .unwrap()
        .trim_end_matches('"')
        .to_string();
    fs::write(dir.join("eggs.lock"), lock.replace(&sha, &"0".repeat(64))).unwrap();
    fs::remove_dir_all(&folder).unwrap();
    let err = block_on(client.variant(&pkg, &plat)).unwrap_err();
    assert!(matches!(err, Error::ChecksumMismatch { .. }), "{err}");
}

#[test]
fn a_client_without_targets_cannot_add() {
    let env = Env::new();
    publish(&env.reg(), &env.work(), "a", "1.0.0", &[], false);
    let config = Config::parse(&format!(
        "registries {{\n dir \"{}\"\n}}",
        env.reg().display()
    ))
    .unwrap();
    let client = Client::builder()
        .git(common::isolated_git())
        .config(config)
        .build()
        .unwrap();
    let err = block_on(client.add(&spec("demo/a"), &env.dir("p"))).unwrap_err();
    assert!(matches!(err, Error::Invalid { .. }), "{err}");
}

#[test]
fn install_from_the_lock_gives_the_same_files() {
    let env = Env::new();
    publish(&env.reg(), &env.work(), "b", "1.0.0", &[], false);
    publish(&env.reg(), &env.work(), "a", "1.0.0", &[("b", "^1")], false);
    let first = env.dir("first");
    block_on(env.client().add(&spec("demo/a"), &first)).unwrap();

    let second = env.dir("second");
    fs::create_dir_all(&second).unwrap();
    fs::copy(first.join("eggs.lock"), second.join("eggs.lock")).unwrap();
    let report = block_on(env.client().install(&second)).unwrap();
    assert_eq!(report.packages.len(), 2);
    assert_eq!(
        tree_digest(&first.join("demo/a")).unwrap(),
        tree_digest(&second.join("demo/a")).unwrap()
    );

    // Nothing to do the second time.
    let report = block_on(env.client().install(&second)).unwrap();
    assert!(report.packages.is_empty());
}

#[test]
fn a_tampered_registry_is_refused_on_install() {
    let env = Env::new();
    publish(&env.reg(), &env.work(), "a", "1.0.0", &[], false);
    let first = env.dir("first");
    block_on(env.client().add(&spec("demo/a"), &first)).unwrap();

    // The bytes no longer match the index.
    let egg_path = env.reg().join("demo/a/a-1.0.0.egg");
    let mut bytes = fs::read(&egg_path).unwrap();
    bytes.extend_from_slice(b"tampered");
    fs::write(&egg_path, bytes).unwrap();

    let second = env.dir("second");
    fs::create_dir_all(&second).unwrap();
    fs::copy(first.join("eggs.lock"), second.join("eggs.lock")).unwrap();
    let err = block_on(env.client().install(&second)).unwrap_err();
    assert!(matches!(err, Error::ChecksumMismatch { .. }), "{err}");
    assert!(!second.join("demo/a").exists());
}

#[test]
fn a_lock_checksum_that_differs_is_refused() {
    let env = Env::new();
    publish(&env.reg(), &env.work(), "a", "1.0.0", &[], false);
    let first = env.dir("first");
    block_on(env.client().add(&spec("demo/a"), &first)).unwrap();
    let lock = fs::read_to_string(first.join("eggs.lock")).unwrap();
    let sha = lock
        .lines()
        .find_map(|l| l.strip_prefix("sha256 = \""))
        .unwrap()
        .trim_end_matches('"')
        .to_string();
    let second = env.dir("second");
    fs::create_dir_all(&second).unwrap();
    fs::write(
        second.join("eggs.lock"),
        lock.replace(&sha, &"0".repeat(64)),
    )
    .unwrap();
    let err = block_on(env.client().install(&second)).unwrap_err();
    assert!(matches!(err, Error::ChecksumMismatch { .. }), "{err}");
}

#[test]
fn remove_refuses_while_a_dependent_is_installed() {
    let env = Env::new();
    publish(&env.reg(), &env.work(), "b", "1.0.0", &[], false);
    publish(&env.reg(), &env.work(), "a", "1.0.0", &[("b", "^1")], false);
    let dir = env.dir("p");
    let client = env.client();
    block_on(client.add(&spec("demo/a"), &dir)).unwrap();

    match client.remove(&name("demo/b"), &dir).unwrap_err() {
        Error::RequiredBy { dependents, .. } => assert_eq!(dependents, ["demo/a ^1"]),
        e => panic!("{e}"),
    }
    assert!(dir.join("demo/b").exists());

    client.remove(&name("demo/a"), &dir).unwrap();
    client.remove(&name("demo/b"), &dir).unwrap();
    assert!(!dir.join("demo/a").exists());
    assert!(!dir.join("demo/b").exists());
    assert!(client.installed(&dir).unwrap().is_empty());
    assert!(matches!(
        client.remove(&name("demo/a"), &dir),
        Err(Error::NotFound { .. })
    ));
}

#[test]
fn update_picks_a_newer_version_within_dependents_ranges() {
    let env = Env::new();
    publish(&env.reg(), &env.work(), "b", "1.0.0", &[], false);
    publish(&env.reg(), &env.work(), "a", "1.0.0", &[("b", "^1")], false);
    let dir = env.dir("p");
    let client = env.client();
    block_on(client.add(&spec("demo/a"), &dir)).unwrap();

    publish(&env.reg(), &env.work(), "b", "1.1.0", &[], false);
    publish(&env.reg(), &env.work(), "b", "2.0.0", &[], false);
    publish(&env.reg(), &env.work(), "a", "1.1.0", &[("b", "^1")], false);

    let report = block_on(client.update(Some(&name("demo/b")), &dir)).unwrap();
    assert_eq!(report.packages.len(), 1);
    assert_eq!(report.packages[0].version, Version::new(1, 1, 0));
    assert_eq!(report.packages[0].previous, Some(Version::new(1, 0, 0)));

    let report = block_on(client.update(None, &dir)).unwrap();
    let got: Vec<(String, String)> = report
        .packages
        .iter()
        .map(|c| (c.name.to_string(), c.version.to_string()))
        .collect();
    assert_eq!(got, [("demo/a".to_string(), "1.1.0".to_string())]);

    let report = block_on(client.update(None, &dir)).unwrap();
    assert!(report.packages.is_empty());
    assert!(matches!(
        block_on(client.update(Some(&name("demo/zzz")), &dir)),
        Err(Error::NotFound { .. })
    ));
}

#[test]
fn variants_are_fetched_at_install_or_on_demand() {
    let env = Env::new();
    publish(&env.reg(), &env.work(), "v", "1.0.0", &[], true);
    let rookery = |v: Variants| StaticTarget::new("rookery").variants(v);
    let cache = env.root.join("cache");

    // At install: the variant for this platform.
    let client = client_for(
        &env.reg(),
        &cache,
        rookery(Variants::Matching(Platform::new("linux", "aarch64"))),
        "",
    );
    let dir = env.dir("matching");
    block_on(client.add(&spec("demo/v"), &dir)).unwrap();
    assert_eq!(
        fs::read_to_string(dir.join("demo/v/rookery/variants/aarch64/bin")).unwrap(),
        "arm"
    );
    assert!(!dir.join("demo/v/rookery/variants/x86_64").exists());
    let lock = fs::read_to_string(dir.join("eggs.lock")).unwrap();
    assert!(lock.contains("\"rookery.aarch64\""), "{lock}");

    // No variant for this platform fails before anything is written.
    let client = client_for(
        &env.reg(),
        &cache,
        rookery(Variants::Matching(Platform::new("linux", "riscv64"))),
        "",
    );
    let err = block_on(client.add(&spec("demo/v"), &env.dir("none"))).unwrap_err();
    assert!(matches!(err, Error::NoVariant { .. }), "{err}");
    assert!(!env.dir("none").join("demo").exists());

    // On demand: nothing at install, then the variant when asked, once.
    let client = client_for(&env.reg(), &cache, rookery(Variants::OnDemand), "");
    let dir = env.dir("ondemand");
    block_on(client.add(&spec("demo/v"), &dir)).unwrap();
    assert!(!dir.join("demo/v/rookery/variants").exists());
    let pkg = client.installed(&dir).unwrap().remove(0);
    let folder = block_on(client.variant(&pkg, &Platform::new("linux", "x86_64"))).unwrap();
    assert_eq!(fs::read_to_string(folder.join("bin")).unwrap(), "x86");
    assert!(folder.ends_with("rookery/variants/x86_64"));
    let lock = fs::read_to_string(dir.join("eggs.lock")).unwrap();
    assert!(lock.contains("\"rookery.x86_64\""), "{lock}");
    let again = block_on(client.variant(&pkg, &Platform::new("linux", "x86_64"))).unwrap();
    assert_eq!(again, folder);
    let err = block_on(client.variant(&pkg, &Platform::new("linux", "riscv64"))).unwrap_err();
    assert!(matches!(err, Error::NoVariant { .. }), "{err}");

    // A package whose target has no variants has no variant files.
    publish(&env.reg(), &env.work(), "plain", "1.0.0", &[], false);
    let both = client_for(&env.reg(), &cache, crow(), "");
    let d2 = env.dir("plain");
    block_on(both.add(&spec("demo/plain"), &d2)).unwrap();
    let plain = both.installed(&d2).unwrap().remove(0);
    let err = block_on(both.variant(&plain, &Platform::current())).unwrap_err();
    assert!(matches!(err, Error::NoVariant { .. }), "{err}");
}

#[test]
fn a_fat_egg_file_installs_with_no_registry() {
    let env = Env::new();
    let dir = env.work().join("fat-1.0.0");
    common::package(
        &dir,
        &manifest("fat", "1.0.0", &[], true),
        &[
            ("crow/plugin.json", "{}"),
            ("rookery/variants/x86_64/bin", "x86"),
            ("rookery/variants/aarch64/bin", "arm"),
        ],
    );
    let opts = PackOptions {
        no_build: true,
        fat: true,
        ..PackOptions::default()
    };
    let egg = pack(&dir, &opts).unwrap().remove(0);
    assert!(egg.is_fat());
    let file = env.root.join("fat-1.0.0.egg");
    fs::write(&file, egg.bytes()).unwrap();

    let client = Client::builder()
        .git(common::isolated_git())
        .target(
            StaticTarget::new("rookery")
                .variants(Variants::Matching(Platform::new("linux", "x86_64"))),
        )
        .clock(clock())
        .cache_dir(env.root.join("cache"))
        .build()
        .unwrap();
    let out = env.dir("p");
    let report = block_on(client.add(&Spec::File(file.display().to_string()), &out)).unwrap();
    assert_eq!(report.packages[0].name, name("demo/fat"));
    assert_eq!(
        fs::read_to_string(out.join("demo/fat/rookery/variants/x86_64/bin")).unwrap(),
        "x86"
    );
    let lock = fs::read_to_string(out.join("eggs.lock")).unwrap();
    assert!(lock.contains("file ="), "{lock}");
    // The lock records the base egg's checksum, not the fat egg's.
    assert!(lock.contains(egg.base().unwrap().sha256()), "{lock}");
    assert!(!lock.contains(egg.sha256()), "{lock}");

    // Reinstalling from the lock works with no registry either.
    let again = env.dir("again");
    fs::create_dir_all(&again).unwrap();
    fs::copy(out.join("eggs.lock"), again.join("eggs.lock")).unwrap();
    block_on(client.install(&again)).unwrap();
    assert_eq!(
        tree_digest(&out.join("demo/fat")).unwrap(),
        tree_digest(&again.join("demo/fat")).unwrap()
    );

    // On demand, the other architecture comes from inside the file.
    let od = Client::builder()
        .git(common::isolated_git())
        .target(StaticTarget::new("rookery").variants(Variants::OnDemand))
        .cache_dir(env.root.join("cache"))
        .build()
        .unwrap();
    let dir2 = env.dir("od");
    // The fat egg's variants are unpacked with it, so both are there.
    block_on(od.add(&Spec::File(file.display().to_string()), &dir2)).unwrap();
    let pkg = od.installed(&dir2).unwrap().remove(0);
    let folder = block_on(od.variant(&pkg, &Platform::new("linux", "aarch64"))).unwrap();
    assert_eq!(fs::read_to_string(folder.join("bin")).unwrap(), "arm");
}

#[test]
fn a_folder_installs_and_is_left_by_update() {
    let env = Env::new();
    let dir = env.work().join("dev");
    common::package(
        &dir,
        &manifest("dev", "0.1.0", &[], false),
        &[("crow/plugin.json", "{}")],
    );
    let out = env.dir("p");
    let client = Client::builder()
        .git(common::isolated_git())
        .target(crow())
        .clock(clock())
        .cache_dir(env.root.join("cache"))
        .build()
        .unwrap();
    block_on(client.add(&Spec::Folder(dir.clone()), &out)).unwrap();
    assert!(out.join("demo/dev/crow/plugin.json").is_file());
    let lock = fs::read_to_string(out.join("eggs.lock")).unwrap();
    assert!(lock.contains("folder ="), "{lock}");
    let report = block_on(client.update(None, &out)).unwrap();
    assert!(report.packages.is_empty());
}

#[test]
fn requirements_and_min_age_apply_to_registries() {
    let env = Env::new();
    publish(&env.reg(), &env.work(), "a", "1.0.0", &[], false);
    let cache = env.root.join("cache");

    // Published just now: held for three days.
    let client = client_for(&env.reg(), &cache, crow(), "default min-age=\"3d\"");
    let err = block_on(client.add(&spec("demo/a"), &env.dir("p"))).unwrap_err();
    match err {
        Error::TooYoung { until, .. } => {
            assert_eq!(until, Timestamp::parse("2026-11-20T00:00:00Z").unwrap())
        }
        e => panic!("{e}"),
    }
    let opts = hatchery::Options { allow_young: true };
    block_on(client.add_with(&spec("demo/a"), &env.dir("p"), &opts)).unwrap();

    let listed = block_on(client.offered(&env.dir("q"))).unwrap().packages;
    assert_eq!(
        listed[0].held_until,
        Some(Timestamp::parse("2026-11-20T00:00:00Z").unwrap())
    );
    // The listing recorded what it saw.
    assert!(env.dir("q").join(".hatchery/seen.toml").is_file());
}

#[test]
fn offered_lists_names_versions_and_what_is_installed() {
    let env = Env::new();
    publish(&env.reg(), &env.work(), "a", "1.0.0", &[], false);
    publish(&env.reg(), &env.work(), "a", "1.1.0", &[], false);
    publish(&env.reg(), &env.work(), "v", "1.0.0", &[], true);
    let dir = env.dir("p");
    let client = env.client();
    let listed = block_on(client.offered(&dir)).unwrap().packages;
    assert_eq!(listed.len(), 2);
    let a = &listed[0];
    assert_eq!(a.name, name("demo/a"));
    assert_eq!(a.versions[0].version, Version::new(1, 1, 0));
    assert_eq!(a.versions[1].version, Version::new(1, 0, 0));
    assert!(a.installed.is_none());
    assert!(a.versions[0].problem.is_none());
    assert!(a.versions[0].published.is_some());

    // A package with no target for this client says why it cannot be installed.
    let other = client_for(
        &env.reg(),
        &env.root.join("cache"),
        StaticTarget::new("other"),
        "",
    );
    let listed_other = block_on(other.offered(&env.dir("q"))).unwrap().packages;
    assert!(
        listed_other[0].versions[0]
            .problem
            .as_deref()
            .unwrap()
            .contains("no target")
    );

    block_on(client.add(&spec("demo/a@1.0.0"), &dir)).unwrap();
    let listed = block_on(client.offered(&dir)).unwrap().packages;
    assert_eq!(listed[0].installed, Some(Version::new(1, 0, 0)));
    assert_eq!(listed[0].updatable, Some(Version::new(1, 1, 0)));
}

#[test]
fn fetch_returns_the_base_and_the_variant_eggs() {
    let env = Env::new();
    publish(&env.reg(), &env.work(), "v", "1.0.0", &[], true);
    let client = env.client();
    let opts = hatchery::Options::default();
    let all = block_on(client.fetch(&spec("demo/v"), None, &opts)).unwrap();
    assert_eq!(all.base.manifest().package.id, "v");
    assert_eq!(all.variants.len(), 2);
    let one = block_on(client.fetch(
        &spec("demo/v"),
        Some(&["rookery.x86_64".to_string()]),
        &opts,
    ))
    .unwrap();
    assert_eq!(one.variants.len(), 1);
    assert_eq!(one.variants[0].variant().unwrap().key(), "x86_64");
}

#[test]
fn unknown_and_ambiguous_names() {
    let env = Env::new();
    publish(&env.reg(), &env.work(), "a", "1.0.0", &[], false);
    let client = env.client();
    assert!(matches!(
        block_on(client.add(&spec("demo/missing"), &env.dir("p"))),
        Err(Error::NotFound { .. })
    ));
    assert!(matches!(
        block_on(client.add(&spec("missing"), &env.dir("p"))),
        Err(Error::NotFound { .. })
    ));
}

#[test]
fn a_git_registry_pins_commits_and_refuses_a_moved_tag() {
    let env = Env::new();
    let repo = env.root.join("repo");
    common::git_init(&repo);
    let release = |version: &str| {
        common::write(
            &repo,
            "packages.kdl",
            format!(
                "format 1\ndefaults {{\n  package {{\n    group example\n    version \"{version}\"\n  }}\n}}\n\
                 packages {{\n  find \"agents/*\"\n}}\nrelease {{\n  tag \"v{{version}}\"\n}}\n"
            ),
        );
        common::package(
            &repo.join("agents/alpha"),
            &format!(
                "format 1\npackage {{\n id alpha\n version \"{version}\"\n}}\ntargets {{\n crow {{\n }}\n}}\n"
            ),
            &[("crow/plugin.json", &format!("{{\"v\":\"{version}\"}}"))],
        );
        let c = common::git_commit(&repo, version);
        common::git(&repo, &["tag", "-f", &format!("v{version}")]);
        c
    };
    let first = release("1.0.0");

    let config = Config::parse(&format!(
        "registries {{\n  git \"{}\"\n}}\n",
        repo.display()
    ))
    .unwrap();
    let build = || {
        Client::builder()
            .git(common::isolated_git())
            .config(config.clone())
            .target(crow())
            .clock(clock())
            .cache_dir(env.root.join("cache"))
            .build()
            .unwrap()
    };
    let client = build();
    let dir = env.dir("p");
    block_on(client.add(&spec("example/alpha"), &dir)).unwrap();
    let lock = fs::read_to_string(dir.join("eggs.lock")).unwrap();
    assert!(lock.contains(&format!("commit = \"{first}\"")), "{lock}");
    assert!(lock.contains("ref = \"v1.0.0\""), "{lock}");
    assert!(dir.join("example/alpha/crow/plugin.json").is_file());

    // The tag now points at another commit.
    common::write(&repo, "agents/alpha/crow/plugin.json", "{\"v\":\"evil\"}");
    common::git_commit(&repo, "move");
    common::git(&repo, &["tag", "-f", "v1.0.0"]);

    let fresh = env.dir("fresh");
    fs::create_dir_all(&fresh).unwrap();
    fs::copy(dir.join("eggs.lock"), fresh.join("eggs.lock")).unwrap();
    let err = block_on(build().install(&fresh)).unwrap_err();
    assert!(matches!(err, Error::TagMoved { .. }), "{err}");
    assert!(!fresh.join("example").exists());

    let err = block_on(build().add(&spec("example/alpha"), &dir)).unwrap_err();
    assert!(matches!(err, Error::TagMoved { .. }), "{err}");
}

#[test]
fn credentials_in_git_addresses_stay_out_of_the_lock() {
    let env = Env::new();
    let repo = env.root.join("repo");
    common::git_init(&repo);
    common::write(
        &repo,
        "packages.kdl",
        "format 1\ndefaults {\n  package {\n    group example\n    version \"1.0.0\"\n  }\n}\npackages {\n  find \"agents/*\"\n}\nrelease {\n  tag \"v{version}\"\n}\n",
    );
    common::package(
        &repo.join("agents/alpha"),
        "format 1\npackage {\n id alpha\n version \"1.0.0\"\n}\ntargets {\n crow {\n }\n}\n",
        &[("crow/plugin.json", "{}")],
    );
    common::git_commit(&repo, "1.0.0");
    common::git(&repo, &["tag", "v1.0.0"]);

    let url = format!("file://user:s3cret@localhost{}", repo.display());
    let config = Config::parse(&format!("registries {{\n  git \"{url}\"\n}}\n")).unwrap();
    let build = || {
        Client::builder()
            .git(common::isolated_git())
            .config(config.clone())
            .target(crow())
            .clock(clock())
            .cache_dir(env.root.join("cache"))
            .build()
            .unwrap()
    };
    let dir = env.dir("p");
    block_on(build().add(&spec("example/alpha"), &dir)).unwrap();
    let lock = fs::read_to_string(dir.join("eggs.lock")).unwrap();
    assert!(!lock.contains("s3cret"), "{lock}");
    // The entry still finds its registry.
    let fresh = env.dir("fresh");
    fs::create_dir_all(&fresh).unwrap();
    fs::copy(dir.join("eggs.lock"), fresh.join("eggs.lock")).unwrap();
    block_on(build().install(&fresh)).unwrap();

    // A single git folder at a tag: the tag moving is refused on update.
    let dir2 = env.dir("p2");
    let client = Client::builder()
        .git(common::isolated_git())
        .target(crow())
        .clock(clock())
        .cache_dir(env.root.join("cache"))
        .build()
        .unwrap();
    let spec = Spec::git(
        repo.display().to_string(),
        Some("agents/alpha".to_string()),
        Some("v1.0.0".to_string()),
    );
    block_on(client.add(&spec, &dir2)).unwrap();
    common::write(&repo, "agents/alpha/crow/plugin.json", "{\"moved\":true}");
    common::git_commit(&repo, "move");
    common::git(&repo, &["tag", "-f", "v1.0.0"]);
    let err = block_on(client.update(None, &dir2)).unwrap_err();
    assert!(matches!(err, Error::TagMoved { .. }), "{err}");
    let fresh2 = env.dir("fresh2");
    fs::create_dir_all(&fresh2).unwrap();
    fs::copy(dir2.join("eggs.lock"), fresh2.join("eggs.lock")).unwrap();
    let err = block_on(client.install(&fresh2)).unwrap_err();
    assert!(matches!(err, Error::TagMoved { .. }), "{err}");
}

#[test]
fn a_lock_entry_from_a_registry_that_no_longer_serves_its_group_is_refused() {
    let env = Env::new();
    publish(&env.reg(), &env.work(), "b", "1.0.0", &[], false);
    let first = env.dir("first");
    block_on(env.client().add(&spec("demo/b"), &first)).unwrap();

    let second = env.dir("second");
    fs::create_dir_all(&second).unwrap();
    fs::copy(first.join("eggs.lock"), second.join("eggs.lock")).unwrap();
    let config = Config::parse(&format!(
        "registries {{\n  dir \"{}\" only=\"other\"\n}}\n",
        env.reg().display()
    ))
    .unwrap();
    let client = Client::builder()
        .git(common::isolated_git())
        .config(config)
        .target(crow())
        .clock(clock())
        .cache_dir(env.root.join("cache"))
        .build()
        .unwrap();
    let err = block_on(client.install(&second)).unwrap_err();
    assert!(matches!(err, hatchery::Error::NotFound { .. }), "{err}");
    assert!(!second.join("demo/b").exists());
}

/// A repository with `packages.kdl` and `example/alpha` 1.0.0 at tag v1.0.0.
fn alpha_repo(repo: &Path) {
    common::git_init(repo);
    common::write(
        repo,
        "packages.kdl",
        "format 1\ndefaults {\n  package {\n    group example\n    version \"1.0.0\"\n  }\n}\npackages {\n  find \"agents/*\"\n}\nrelease {\n  tag \"v{version}\"\n}\n",
    );
    common::package(
        &repo.join("agents/alpha"),
        "format 1\npackage {\n id alpha\n version \"1.0.0\"\n}\ntargets {\n crow {\n }\n}\n",
        &[("crow/plugin.json", "{}")],
    );
    common::git_commit(repo, "1.0.0");
    common::git(repo, &["tag", "v1.0.0"]);
}

fn git_client(env: &Env, config: &str) -> Client {
    Client::builder()
        .git(common::isolated_git())
        .config(Config::parse(config).unwrap())
        .target(crow())
        .clock(clock())
        .cache_dir(env.root.join("cache"))
        .build()
        .unwrap()
}

#[test]
fn credentials_in_git_addresses_stay_out_of_listings_errors_and_records() {
    let env = Env::new();
    let repo = env.root.join("repo");
    alpha_repo(&repo);
    let url = format!("file://user:s3cret@localhost{}", repo.display());
    let client = git_client(
        &env,
        &format!("registries {{\n  git \"{url}\" min-age=\"3d\"\n}}\n"),
    );
    let dir = env.dir("p");
    let offered = block_on(client.offered(&dir)).unwrap().packages;
    assert_eq!(offered.len(), 1);
    assert!(!offered[0].registry.contains("s3cret"), "{offered:?}");
    let err = block_on(client.add(&spec("example/alpha"), &dir)).unwrap_err();
    assert!(matches!(err, Error::TooYoung { .. }), "{err}");
    assert!(!format!("{err} {err:?}").contains("s3cret"), "{err:?}");
    let err = block_on(client.add(&spec("example/nope"), &dir)).unwrap_err();
    assert!(!format!("{err} {err:?}").contains("s3cret"), "{err:?}");
    let seen = fs::read_to_string(dir.join(".hatchery/seen.toml")).unwrap();
    assert!(seen.contains("file://localhost"), "{seen}");
    assert!(!seen.contains("s3cret"), "{seen}");
}

/// A repository whose packages are `example/<id>` in `agents/<id>`, with
/// tags `v<version>` shared by all of them.
struct Repo {
    dir: PathBuf,
}

impl Repo {
    fn new(env: &Env) -> Repo {
        let dir = env.root.join("repo");
        common::git_init(&dir);
        common::write(
            &dir,
            "packages.kdl",
            "format 1\ndefaults {\n  package {\n    group example\n  }\n}\npackages {\n  find \"agents/*\"\n}\nrelease {\n  tag \"v{version}\"\n  eggs branch=\"eggs\"\n}\n",
        );
        Repo { dir }
    }

    /// Writes package `id` at `version` in `folder` (under the repository).
    fn put(&self, folder: &str, id: &str, version: &str) {
        common::package(
            &self.dir.join(folder),
            &format!(
                "format 1\npackage {{\n id {id}\n version \"{version}\"\n}}\ntargets {{\n crow {{\n }}\n}}\n"
            ),
            &[("crow/plugin.json", &format!("{{\"v\":\"{version}\"}}"))],
        );
    }

    fn commit_tag(&self, tag: &str) -> String {
        let c = common::git_commit(&self.dir, tag);
        common::git(&self.dir, &["tag", tag]);
        c
    }

    fn url(&self) -> String {
        self.dir.display().to_string()
    }
}

fn plugin(dir: &Path, name: &str) -> String {
    fs::read_to_string(dir.join(name).join("crow/plugin.json")).unwrap()
}

#[test]
fn a_keyed_git_registry_does_not_install_a_version_that_has_only_a_tag() {
    let env = Env::new();
    let repo = Repo::new(&env);
    repo.put("agents/alpha", "alpha", "1.0.0");
    repo.commit_tag("v1.0.0");
    // 1.0.0 is published signed; 1.1.0 is only a tag anyone could push.
    let key = hatchery::SigningKey::from_bytes(&[9; 32]);
    let work = env.dir("built");
    common::package(
        &work,
        "format 1\npackage {\n group example\n id alpha\n version \"1.0.0\"\n}\ntargets {\n crow {\n }\n}\n",
        &[("crow/plugin.json", "{\"v\":\"1.0.0\"}")],
    );
    let opts = PackOptions {
        no_build: true,
        ..PackOptions::default()
    };
    let egg = pack(&work, &opts).unwrap().pop().unwrap();
    hatchery::source::git::publish_branch(
        &egg,
        &repo.url(),
        "eggs",
        None,
        Some(&key),
        Timestamp::parse(NOW).unwrap(),
        &common::isolated_git(),
    )
    .unwrap();
    repo.put("agents/alpha", "alpha", "1.1.0");
    repo.commit_tag("v1.1.0");

    let public = hatchery::sign::format_verifying_key(&key.verifying_key());
    let line = |extra: &str| {
        format!(
            "registries {{\n  git \"{}\" key=\"{public}\" min-age=\"0d\"{extra}\n}}\n",
            repo.url()
        )
    };
    let client = git_client(&env, &line(""));
    let dir = env.dir("p");
    let listing = block_on(client.offered(&dir)).unwrap();
    let versions: Vec<String> = listing.packages[0]
        .versions
        .iter()
        .map(|v| v.version.to_string())
        .collect();
    assert_eq!(versions, ["1.0.0"]);
    let report = block_on(client.add(&spec("example/alpha"), &dir)).unwrap();
    assert_eq!(report.packages[0].version, Version::new(1, 0, 0));
    assert_eq!(plugin(&dir, "example/alpha"), "{\"v\":\"1.0.0\"}");
    let err = block_on(client.add(&spec("example/alpha@1.1.0"), &dir)).unwrap_err();
    assert!(matches!(err, Error::SignatureInvalid { .. }), "{err}");
    assert!(err.to_string().contains("only a tag"), "{err}");

    // A registry line can allow tags again.
    let client = git_client(&env, &line(" unsigned-tags=#true"));
    let report = block_on(client.add(&spec("example/alpha@1.1.0"), &dir)).unwrap();
    assert_eq!(report.packages[0].version, Version::new(1, 1, 0));
}

#[test]
fn a_broken_package_does_not_hide_the_others_of_a_git_registry() {
    let env = Env::new();
    let repo = Repo::new(&env);
    repo.put("agents/alpha", "alpha", "1.0.0");
    common::write(
        &repo.dir,
        "agents/broken/package.kdl",
        "format 1\npackage {\n id\n}\n",
    );
    repo.commit_tag("v1.0.0");
    let client = git_client(
        &env,
        &format!("registries {{\n  git \"{}\"\n}}\n", repo.url()),
    );
    let dir = env.dir("p");
    let listing = block_on(client.offered(&dir)).unwrap();
    assert_eq!(listing.packages.len(), 1);
    assert!(
        listing.warnings.iter().any(|w| w.contains("agents/broken")),
        "{:?}",
        listing.warnings
    );
    let report = block_on(client.add(&spec("example/alpha"), &dir)).unwrap();
    assert!(
        report.warnings.iter().any(|w| w.contains("agents/broken")),
        "{:?}",
        report.warnings
    );
    let err = block_on(client.add(&spec("example/broken"), &dir)).unwrap_err();
    assert!(matches!(err, Error::Invalid { .. }), "{err}");
    assert!(err.to_string().contains("agents/broken"), "{err}");
}

#[test]
fn a_package_moved_or_removed_on_the_default_branch_still_installs_from_its_tag() {
    let env = Env::new();
    let repo = Repo::new(&env);
    repo.put("agents/alpha", "alpha", "1.0.0");
    repo.commit_tag("v1.0.0");
    let client = || {
        git_client(
            &env,
            &format!("registries {{\n  git \"{}\"\n}}\n", repo.url()),
        )
    };
    let dir = env.dir("p");
    block_on(client().add(&spec("example/alpha"), &dir)).unwrap();

    let install_fresh = |name: &str| {
        let fresh = env.dir(name);
        fs::create_dir_all(&fresh).unwrap();
        fs::copy(dir.join("eggs.lock"), fresh.join("eggs.lock")).unwrap();
        block_on(client().install(&fresh)).unwrap();
        plugin(&fresh, "example/alpha")
    };

    // Moved to another folder on the default branch.
    common::git(&repo.dir, &["mv", "agents/alpha", "agents/moved"]);
    common::git_commit(&repo.dir, "move");
    assert_eq!(install_fresh("moved"), "{\"v\":\"1.0.0\"}");
    // A version resolved now is read where the package was at its tag.
    let other = env.dir("other");
    let report = block_on(client().add(&spec("example/alpha@1.0.0"), &other)).unwrap();
    assert_eq!(report.packages[0].version, Version::new(1, 0, 0));

    // Removed from the default branch.
    common::git(&repo.dir, &["rm", "-rq", "agents/moved"]);
    common::git_commit(&repo.dir, "remove");
    assert_eq!(install_fresh("removed"), "{\"v\":\"1.0.0\"}");
}

#[test]
fn a_shared_tag_counts_only_for_packages_at_that_version() {
    let env = Env::new();
    let repo = Repo::new(&env);
    repo.put("agents/alpha", "alpha", "1.0.0");
    repo.commit_tag("v1.0.0");
    repo.put("agents/beta", "beta", "2.0.0");
    repo.commit_tag("v2.0.0");
    let client = git_client(
        &env,
        &format!("registries {{\n  git \"{}\"\n}}\n", repo.url()),
    );
    let listing = block_on(client.offered(&env.dir("p"))).unwrap();
    let newest: Vec<(String, String)> = listing
        .packages
        .iter()
        .map(|o| (o.name.to_string(), o.versions[0].version.to_string()))
        .collect();
    assert_eq!(
        newest,
        [
            ("example/alpha".to_string(), "1.0.0".to_string()),
            ("example/beta".to_string(), "2.0.0".to_string())
        ]
    );
    for o in &listing.packages {
        assert_eq!(o.versions.len(), 1, "{o:?}");
    }
}

#[test]
fn install_installs_a_lock_that_changed() {
    let env = Env::new();
    publish(&env.reg(), &env.work(), "b", "1.0.0", &[], false);
    publish(&env.reg(), &env.work(), "b", "1.1.0", &[], false);
    let (old, new) = (env.dir("old"), env.dir("new"));
    block_on(env.client().add(&spec("demo/b@1.0.0"), &old)).unwrap();
    block_on(env.client().add(&spec("demo/b@1.1.0"), &new)).unwrap();

    // The lock of a newer version, say from version control.
    fs::copy(new.join("eggs.lock"), old.join("eggs.lock")).unwrap();
    let listed = env.client().installed(&old).unwrap();
    assert_eq!(listed[0].version, Version::new(1, 0, 0));
    let report = block_on(env.client().install(&old)).unwrap();
    assert_eq!(report.packages.len(), 1, "{report:?}");
    assert_eq!(report.packages[0].previous, Some(Version::new(1, 0, 0)));
    assert_eq!(report.packages[0].version, Version::new(1, 1, 0));
    assert_eq!(
        env.client().installed(&old).unwrap()[0].version,
        Version::new(1, 1, 0)
    );
    assert_eq!(
        tree_digest(&old.join("demo/b")).unwrap(),
        tree_digest(&new.join("demo/b")).unwrap()
    );
    // Now it is what the lock says, and nothing changes.
    let again = block_on(env.client().install(&old)).unwrap();
    assert!(again.packages.is_empty(), "{again:?}");

    // A folder that holds another egg of the same version is unpacked again.
    fs::write(old.join("demo/b/crow/plugin.json"), "changed").unwrap();
    let records = old.join(".hatchery/unpacked.toml");
    let text = fs::read_to_string(&records).unwrap();
    fs::write(
        &records,
        text.replace(&report.packages[0].sha256, &"0".repeat(64)),
    )
    .unwrap();
    let report = block_on(env.client().install(&old)).unwrap();
    assert_eq!(report.packages.len(), 1);
    assert_eq!(
        fs::read_to_string(old.join("demo/b/crow/plugin.json")).unwrap(),
        "{}"
    );
}
