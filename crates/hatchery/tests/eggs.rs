//! Packing, opening, subsetting and unpacking eggs.

mod common;

use std::fs;
use std::path::Path;

use common::{git, git_commit, git_init, write};
use hatchery::egg::{Egg, Entry};
use hatchery::manifest::{Manifest, Origin};
use hatchery::pack::{PackOptions, VariantPick, pack};
use hatchery::{Error, Problem, StaticTarget, Target, TargetFiles, Timestamp};

const MANIFEST: &str = r#"
package {
  group aiviarium
  id demo
  version "1.2.0"
}
about {
  name "Demo"
  icon "icon.svg"
  readme "README.md"
  repository "https://example.org/demo.git"
}
targets {
  crow {
    exclude "**/*.map"
  }
  rookery {
    variants {
      variant os=linux arch=x86_64
      variant os=macos
      variant
    }
  }
}
"#;

fn origin() -> Origin {
    Origin {
        repository: Some("https://example.org/demo.git".into()),
        commit: None,
        built: Some(Timestamp::parse("2026-11-01T00:00:00Z").unwrap()),
        hatchery: Some("0.0.0".into()),
        extra: Vec::new(),
    }
}

fn opts() -> PackOptions<'static> {
    PackOptions {
        origin: Some(origin()),
        no_build: true,
        ..Default::default()
    }
}

/// A package folder with two targets and three variants of one of them.
fn demo(dir: &Path) {
    write(dir, "package.kdl", MANIFEST);
    write(dir, "icon.svg", "<svg/>");
    write(dir, "README.md", "# Demo\n");
    write(dir, "crow/plugin.json", "{}");
    write(dir, "crow/host.js", "export {}");
    write(dir, "crow/host.js.map", "{}");
    write(dir, "rookery/server.json", "{}");
    write(dir, "rookery/variants/linux-x86_64/bin", "linux-x64");
    write(dir, "rookery/variants/macos/bin", "mac");
    write(dir, "rookery/variants/default/bin", "any");
}

fn paths(egg: &Egg) -> Vec<&str> {
    egg.entries().iter().map(|e| e.path.as_str()).collect()
}

#[test]
fn packing_twice_gives_the_same_bytes() {
    let d = tempfile::tempdir().unwrap();
    demo(d.path());
    let a = pack(d.path(), &opts()).unwrap();
    let b = pack(d.path(), &opts()).unwrap();
    assert_eq!(a.len(), 4);
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x.sha256(), y.sha256());
        assert_eq!(x.bytes(), y.bytes());
    }
    // The time of the build comes from `now` when nothing else gives one.
    let o = PackOptions {
        origin: None,
        now: Some(Timestamp::parse("2026-11-02T00:00:00Z").unwrap()),
        ..opts()
    };
    let e1 = pack(d.path(), &o).unwrap();
    let e2 = pack(d.path(), &o).unwrap();
    assert_eq!(e1.last().unwrap().sha256(), e2.last().unwrap().sha256());
}

#[test]
fn base_egg_holds_the_manifest_about_files_and_shared_files() {
    let d = tempfile::tempdir().unwrap();
    demo(d.path());
    let eggs = pack(
        d.path(),
        &PackOptions {
            base: true,
            ..opts()
        },
    )
    .unwrap();
    assert_eq!(eggs.len(), 1);
    let egg = &eggs[0];
    assert!(!egg.is_fat());
    assert_eq!(egg.file_name(), "demo-1.2.0.egg");
    assert_eq!(
        paths(egg),
        [
            "README.md",
            "crow/host.js",
            "crow/plugin.json",
            "icon.svg",
            "package.kdl",
            "rookery/server.json",
        ]
    );
    let crow = egg.target("crow").unwrap();
    assert_eq!(crow.get("plugin.json"), Some(&b"{}"[..]));
    assert!(crow.get("host.js.map").is_none());
    assert!(
        egg.target("rookery")
            .unwrap()
            .get("variants/macos/bin")
            .is_none()
    );
    assert!(egg.target("nope").is_none());
    assert_eq!(
        egg.manifest().origin.as_ref().unwrap().hatchery.as_deref(),
        Some("0.0.0")
    );

    let reopened = Egg::open(egg.bytes().clone()).unwrap();
    assert_eq!(reopened.sha256(), egg.sha256());
}

#[test]
fn defaults_from_packages_kdl_are_written_out_and_paths_removed() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "packages.kdl",
        "defaults {\n  package {\n    group aiviarium\n    version \"2.0.0\"\n  }\n  about {\n    license MIT\n  }\n}\n",
    );
    let pkg = d.path().join("pkgs/thing");
    write(
        &pkg,
        "package.kdl",
        "package {\n  id thing\n}\ntargets {\n  crow path=\"src/crow\" {\n    variants {\n      variant os=linux path=\"out/linux\"\n    }\n  }\n}\n",
    );
    write(&pkg, "src/crow/a.txt", "a");
    write(&pkg, "out/linux/b.txt", "b");
    let eggs = pack(&pkg, &opts()).unwrap();
    assert_eq!(eggs.len(), 2);
    let base = &eggs[1];
    let m = base.manifest();
    assert_eq!(m.package.group, "aiviarium");
    assert_eq!(m.package.version.to_string(), "2.0.0");
    assert_eq!(m.about.licenses, ["MIT"]);
    assert!(m.targets[0].path.is_none());
    assert!(m.targets[0].variants[0].path.is_none());
    assert_eq!(paths(base), ["crow/a.txt", "package.kdl"]);
    let text = std::str::from_utf8(&base.entries()[1].data).unwrap();
    assert!(!text.contains("path="), "{text}");
    assert!(text.contains("origin"), "{text}");
    assert_eq!(
        paths(&eggs[0]),
        ["crow/variants/linux/b.txt", "variant.kdl"]
    );
}

#[test]
fn origin_comes_from_git() {
    let d = tempfile::tempdir().unwrap();
    git_init(d.path());
    git(
        d.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://user:secret@example.org/owner/demo.git",
        ],
    );
    write(
        d.path(),
        "package.kdl",
        "package {\n  group g\n  id demo\n  version \"1.0.0\"\n}\ntargets {\n  crow\n}\n",
    );
    write(d.path(), "crow/a.txt", "a");
    let commit = git_commit(d.path(), "first");
    let o = PackOptions {
        origin: None,
        base: true,
        no_build: true,
        ..Default::default()
    };
    let egg = pack(d.path(), &o).unwrap().pop().unwrap();
    let origin = egg.manifest().origin.clone().unwrap();
    assert_eq!(origin.commit.as_deref(), Some(commit.as_str()));
    assert_eq!(
        origin.repository.as_deref(),
        Some("https://example.org/owner/demo.git")
    );
    assert_eq!(origin.hatchery.as_deref(), Some(hatchery::VERSION));
    if std::env::var_os("SOURCE_DATE_EPOCH").is_none() {
        assert_eq!(
            origin.built,
            Some(Timestamp::parse("2026-01-01T00:00:00Z").unwrap())
        );
    }
    // The same commit packs to the same egg.
    let again = pack(d.path(), &o).unwrap().pop().unwrap();
    assert_eq!(egg.sha256(), again.sha256());
}

#[test]
fn include_and_exclude() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "package.kdl",
        "package {\n  group g\n  id demo\n  version \"1.0.0\"\n}\ntargets {\n  crow {\n    include \"dist\" \"plugin.json\" \"**/*.js\"\n    exclude \"dist/secret\" \"**/*.map\"\n  }\n}\n",
    );
    write(d.path(), "crow/plugin.json", "{}");
    write(d.path(), "crow/dist/a.css", "a");
    write(d.path(), "crow/dist/a.js.map", "m");
    write(d.path(), "crow/dist/secret/key", "k");
    write(d.path(), "crow/src/b.js", "b");
    write(d.path(), "crow/notes.txt", "n");
    let egg = pack(d.path(), &opts()).unwrap().pop().unwrap();
    let files = egg.target("crow").unwrap();
    assert_eq!(
        files.paths().collect::<Vec<_>>(),
        ["dist/a.css", "plugin.json", "src/b.js"]
    );
}

#[test]
fn variants_are_separate_eggs_and_never_in_the_shared_files() {
    let d = tempfile::tempdir().unwrap();
    demo(d.path());
    let eggs = pack(d.path(), &opts()).unwrap();
    let names: Vec<_> = eggs.iter().map(|e| e.file_name()).collect();
    assert_eq!(
        names,
        [
            "demo-1.2.0.rookery.linux-x86_64.egg",
            "demo-1.2.0.rookery.macos.egg",
            "demo-1.2.0.rookery.default.egg",
            "demo-1.2.0.egg",
        ]
    );
    let linux = &eggs[0];
    let h = linux.variant().unwrap();
    assert_eq!(h.target, "rookery");
    assert_eq!(h.os.as_deref(), Some("linux"));
    assert_eq!(h.arch.as_deref(), Some("x86_64"));
    assert_eq!(h.name.to_string(), "aiviarium/demo");
    assert_eq!(h.version.to_string(), "1.2.0");
    assert_eq!(
        paths(linux),
        ["rookery/variants/linux-x86_64/bin", "variant.kdl"]
    );
    let kdl = std::str::from_utf8(&linux.entries()[1].data).unwrap();
    assert!(
        kdl.contains("arch x86_64") && kdl.contains("os linux"),
        "{kdl}"
    );
    assert_eq!(eggs[2].variant().unwrap().key(), "default");
    assert!(
        eggs[3]
            .entries()
            .iter()
            .all(|e| !e.path.contains("variants"))
    );
    // The variant egg's manifest is the package, the target and the variant.
    let m = linux.manifest();
    assert_eq!(m.targets.len(), 1);
    assert_eq!(m.targets[0].variants[0].key(), "linux-x86_64");

    // A subset of the picks writes just those.
    let one = pack(
        d.path(),
        &PackOptions {
            variants: vec![VariantPick {
                target: "rookery".into(),
                os: Some("macos".into()),
                arch: None,
            }],
            ..opts()
        },
    )
    .unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].file_name(), "demo-1.2.0.rookery.macos.egg");

    let err = pack(
        d.path(),
        &PackOptions {
            variants: vec![VariantPick {
                target: "rookery".into(),
                os: Some("windows".into()),
                arch: None,
            }],
            ..opts()
        },
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("rookery") && msg.contains("macos"), "{msg}");
}

#[test]
fn missing_variant_folder_is_an_error() {
    let d = tempfile::tempdir().unwrap();
    demo(d.path());
    fs::remove_dir_all(d.path().join("rookery/variants/macos")).unwrap();
    let err = pack(d.path(), &opts()).unwrap_err();
    assert!(err.to_string().contains("must exist"), "{err}");
}

#[test]
fn fat_egg_holds_variants_and_gives_the_base_back() {
    let d = tempfile::tempdir().unwrap();
    demo(d.path());
    let base = pack(
        d.path(),
        &PackOptions {
            base: true,
            ..opts()
        },
    )
    .unwrap()
    .pop()
    .unwrap();
    let fat = pack(
        d.path(),
        &PackOptions {
            fat: true,
            ..opts()
        },
    )
    .unwrap();
    assert_eq!(fat.len(), 1);
    let fat = &fat[0];
    assert!(fat.is_fat());
    assert_eq!(fat.file_name(), "demo-1.2.0.egg");
    assert_eq!(
        fat.embedded_variants(),
        [
            ("rookery".to_string(), "default".to_string()),
            ("rookery".to_string(), "linux-x86_64".to_string()),
            ("rookery".to_string(), "macos".to_string()),
        ]
    );
    assert_eq!(fat.base().unwrap().bytes(), base.bytes());
    assert_eq!(base.base().unwrap().sha256(), base.sha256());

    let mac = fat.embedded_variant("rookery", "macos").unwrap().unwrap();
    assert_eq!(mac.file_name(), "demo-1.2.0.rookery.macos.egg");
    assert!(
        fat.embedded_variant("rookery", "freebsd")
            .unwrap()
            .is_none()
    );
    let direct = pack(
        d.path(),
        &PackOptions {
            variants: vec![VariantPick {
                target: "rookery".into(),
                os: Some("macos".into()),
                arch: None,
            }],
            ..opts()
        },
    )
    .unwrap();
    assert_eq!(mac.bytes(), direct[0].bytes());

    // fatten rebuilds the same fat egg from the pieces.
    let all = pack(d.path(), &opts()).unwrap();
    let rebuilt = base.fatten(&all[..3]).unwrap();
    assert_eq!(rebuilt.bytes(), fat.bytes());

    // Only some variants.
    let some = pack(
        d.path(),
        &PackOptions {
            fat: true,
            variants: vec![VariantPick {
                target: "rookery".into(),
                os: None,
                arch: None,
            }],
            ..opts()
        },
    )
    .unwrap();
    assert_eq!(some[0].embedded_variants().len(), 1);

    // Reopening checks the variant eggs inside.
    let reopened = Egg::open(fat.bytes().clone()).unwrap();
    assert!(reopened.is_fat());
}

#[test]
fn fatten_refuses_eggs_of_other_packages() {
    let d = tempfile::tempdir().unwrap();
    demo(d.path());
    let base = pack(
        d.path(),
        &PackOptions {
            base: true,
            ..opts()
        },
    )
    .unwrap()
    .pop()
    .unwrap();
    let other = tempfile::tempdir().unwrap();
    write(
        other.path(),
        "package.kdl",
        "package {\n  group aiviarium\n  id other\n  version \"1.2.0\"\n}\ntargets {\n  rookery {\n    variants {\n      variant\n    }\n  }\n}\n",
    );
    write(other.path(), "rookery/variants/default/x", "x");
    let foreign = pack(other.path(), &opts()).unwrap();
    assert!(base.fatten(&foreign[..1]).is_err());
    assert!(base.fatten(std::slice::from_ref(&base)).is_err());
}

#[test]
fn unpacking_a_fat_egg_puts_variants_in_place() {
    let d = tempfile::tempdir().unwrap();
    demo(d.path());
    let fat = pack(
        d.path(),
        &PackOptions {
            fat: true,
            ..opts()
        },
    )
    .unwrap()
    .pop()
    .unwrap();
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("aiviarium/demo");
    write(&dest, "stale.txt", "old");
    fat.unpack(&dest).unwrap();
    assert!(!dest.join("stale.txt").exists());
    assert!(dest.join("package.kdl").is_file());
    assert!(dest.join("icon.svg").is_file());
    assert!(dest.join("crow/host.js").is_file());
    assert_eq!(
        fs::read_to_string(dest.join("rookery/variants/macos/bin")).unwrap(),
        "mac"
    );
    assert!(dest.join("rookery/variants/linux-x86_64/bin").is_file());
    assert!(!dest.join("variant.kdl").exists());
    assert!(!dest.join("variants").exists());
    // No staging folders are left behind.
    let left: Vec<_> = fs::read_dir(dest.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left.len(), 1, "{left:?}");
}

#[test]
fn unpacking_a_variant_egg_replaces_only_its_folder() {
    let d = tempfile::tempdir().unwrap();
    demo(d.path());
    let eggs = pack(d.path(), &opts()).unwrap();
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("demo");
    eggs[3].unpack(&dest).unwrap();
    write(&dest, "rookery/variants/macos/old", "old");
    eggs[1].unpack(&dest).unwrap();
    assert!(!dest.join("rookery/variants/macos/old").exists());
    assert_eq!(
        fs::read_to_string(dest.join("rookery/variants/macos/bin")).unwrap(),
        "mac"
    );
    assert!(dest.join("rookery/server.json").is_file());
    assert!(!dest.join("variant.kdl").exists());
    assert!(!dest.join("rookery/variants/linux-x86_64").exists());
}

#[cfg(unix)]
#[test]
fn executable_bit_is_kept() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    demo(d.path());
    let script = d.path().join("crow/run.sh");
    write(d.path(), "crow/run.sh", "#!/bin/sh\n");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let egg = pack(
        d.path(),
        &PackOptions {
            base: true,
            ..opts()
        },
    )
    .unwrap()
    .pop()
    .unwrap();
    let e = egg
        .entries()
        .iter()
        .find(|e| e.path == "crow/run.sh")
        .unwrap();
    assert!(e.executable);
    let out = tempfile::tempdir().unwrap();
    egg.unpack(&out.path().join("p")).unwrap();
    let mode = fs::metadata(out.path().join("p/crow/run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
    let plain = fs::metadata(out.path().join("p/crow/host.js"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(plain & 0o777, 0o644);
}

#[cfg(unix)]
#[test]
fn symlinks_are_refused_when_packing() {
    let d = tempfile::tempdir().unwrap();
    demo(d.path());
    std::os::unix::fs::symlink("host.js", d.path().join("crow/link")).unwrap();
    let err = pack(d.path(), &opts()).unwrap_err();
    assert!(err.to_string().contains("symlink"), "{err}");
}

#[test]
fn subset_keeps_only_the_named_targets() {
    let d = tempfile::tempdir().unwrap();
    demo(d.path());
    let fat = pack(
        d.path(),
        &PackOptions {
            fat: true,
            ..opts()
        },
    )
    .unwrap()
    .pop()
    .unwrap();
    let sub = fat.subset(&["rookery"]).unwrap().unwrap();
    assert_eq!(sub.manifest().name(), fat.manifest().name());
    assert_eq!(sub.manifest().version(), fat.manifest().version());
    assert!(sub.manifest().target("crow").is_none());
    assert!(sub.manifest().target("rookery").is_some());
    assert!(sub.target("crow").is_none());
    assert_eq!(sub.embedded_variants().len(), 3);
    assert!(sub.entries().iter().any(|e| e.path == "icon.svg"));
    assert!(sub.entries().iter().all(|e| !e.path.starts_with("crow/")));
    let again = Egg::open(sub.bytes().clone()).unwrap();
    assert_eq!(again.sha256(), sub.sha256());

    let crow = fat.subset(&["crow", "nothing"]).unwrap().unwrap();
    assert!(!crow.is_fat());
    assert!(crow.embedded_variants().is_empty());
    assert_eq!(crow.target("crow").unwrap().len(), 2);

    assert!(fat.subset(&["nothing"]).unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn builds_run_with_the_environment() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "package.kdl",
        r#"package {
  group g
  id demo
  version "3.1.4"
}
targets {
  crow {
    build "mkdir -p crow && echo \"$EGG_ID-$EGG_TARGET-$EGG_VERSION\" > crow/built.txt"
  }
  rookery {
    variants {
      variant os=linux arch=x86_64 build="mkdir -p rookery/variants/linux-x86_64 && echo \"$EGG_TARGET $EGG_OS $EGG_ARCH $EGG_VERSION\" > rookery/variants/linux-x86_64/info"
      variant build="mkdir -p rookery/variants/default && echo \"[$EGG_OS][$EGG_ARCH]\" > rookery/variants/default/info"
    }
  }
}
"#,
    );
    write(d.path(), "rookery/shared.txt", "s");
    let eggs = pack(
        d.path(),
        &PackOptions {
            no_build: false,
            ..opts()
        },
    )
    .unwrap();
    assert_eq!(eggs.len(), 3);
    let linux = eggs[0]
        .entries()
        .iter()
        .find(|e| e.path.ends_with("/info"))
        .unwrap();
    assert_eq!(linux.data.as_ref(), b"rookery linux x86_64 3.1.4\n");
    let any = eggs[1]
        .entries()
        .iter()
        .find(|e| e.path.ends_with("/info"))
        .unwrap();
    assert_eq!(any.data.as_ref(), b"[][]\n");
    let built = eggs[2].target("crow").unwrap();
    assert_eq!(built.get("built.txt"), Some(&b"demo-crow-3.1.4\n"[..]));
    // The variant folders the build made are not shared files.
    assert_eq!(
        eggs[2]
            .target("rookery")
            .unwrap()
            .paths()
            .collect::<Vec<_>>(),
        ["shared.txt"]
    );
}

#[cfg(unix)]
#[test]
fn no_build_skips_commands_and_failures_report_stderr() {
    let d = tempfile::tempdir().unwrap();
    let manifest = |cmd: &str| {
        format!(
            "package {{\n  group g\n  id demo\n  version \"1.0.0\"\n}}\ntargets {{\n  crow {{\n    build \"{cmd}\"\n  }}\n}}\n"
        )
    };
    write(
        d.path(),
        "package.kdl",
        manifest("echo made > crow/made.txt"),
    );
    write(d.path(), "crow/a.txt", "a");
    let skipped = pack(d.path(), &opts()).unwrap().pop().unwrap();
    assert!(skipped.target("crow").unwrap().get("made.txt").is_none());
    let built = pack(
        d.path(),
        &PackOptions {
            no_build: false,
            ..opts()
        },
    )
    .unwrap()
    .pop()
    .unwrap();
    assert!(built.target("crow").unwrap().get("made.txt").is_some());

    write(
        d.path(),
        "package.kdl",
        manifest("echo no luck >&2; exit 3"),
    );
    let err = pack(
        d.path(),
        &PackOptions {
            no_build: false,
            ..opts()
        },
    )
    .unwrap_err();
    match err {
        Error::Build { command, message } => {
            assert!(command.contains("exit 3"));
            assert!(message.contains("no luck"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    // With no_build the broken command never runs.
    pack(d.path(), &opts()).unwrap();
}

struct Strict;

impl Target for Strict {
    fn name(&self) -> &str {
        "crow"
    }
    fn fact(&self, _: &str) -> Option<String> {
        None
    }
    fn check(&self, files: &TargetFiles) -> Result<(), Vec<Problem>> {
        match files.get("plugin.json") {
            Some(b) if b == b"{}" => Ok(()),
            Some(_) => Err(vec![Problem::in_file("plugin.json", "must be empty")]),
            None => Err(vec![Problem::new("has no plugin.json")]),
        }
    }
}

#[test]
fn target_checks_fail_the_pack() {
    let d = tempfile::tempdir().unwrap();
    demo(d.path());
    let strict = Strict;
    let checked = PackOptions {
        checks: vec![&strict],
        ..opts()
    };
    pack(d.path(), &checked).unwrap();
    // A tool for a target the package lacks is not asked.
    let other = StaticTarget::new("elsewhere");
    pack(
        d.path(),
        &PackOptions {
            checks: vec![&other],
            ..opts()
        },
    )
    .unwrap();

    write(d.path(), "crow/plugin.json", "{\"id\": 1}");
    let err = pack(d.path(), &checked).unwrap_err();
    match err {
        Error::Check { target, problems } => {
            assert_eq!(target, "crow");
            assert_eq!(problems[0].path.as_deref(), Some("plugin.json"));
        }
        other => panic!("{other:?}"),
    }
    let err = Egg::pack(d.path(), &[&strict]).unwrap_err();
    assert!(matches!(err, Error::Check { .. }));
}

struct Deriver;

impl Target for Deriver {
    fn name(&self) -> &str {
        "crow"
    }
    fn fact(&self, _: &str) -> Option<String> {
        None
    }
    fn derive(&self, dir: &Path) -> hatchery::Result<Option<Manifest>> {
        let text = fs::read_to_string(dir.join("crow/crow.json")).unwrap();
        let name = text.trim();
        Manifest::parse(&format!(
            "package {{\n  group g\n  id {name}\n  version \"0.1.0\"\n}}\ntargets {{\n  crow\n}}\n"
        ))
        .map(Some)
    }
}

#[test]
fn a_target_derives_the_manifest_of_a_folder_without_one() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "crow/crow.json", "plain");
    let none = pack(d.path(), &opts());
    assert!(matches!(none, Err(Error::Invalid { .. })));
    let other = StaticTarget::new("crow");
    assert!(
        pack(
            d.path(),
            &PackOptions {
                checks: vec![&other],
                ..opts()
            }
        )
        .is_err()
    );
    let derive = Deriver;
    let egg = Egg::pack(d.path(), &[&derive]).unwrap();
    assert_eq!(egg.manifest().package.id, "plain");
    assert_eq!(
        egg.target("crow").unwrap().paths().collect::<Vec<_>>(),
        ["crow.json"]
    );
}

#[test]
fn about_files_must_exist_and_stay_out_of_reserved_places() {
    let d = tempfile::tempdir().unwrap();
    demo(d.path());
    fs::remove_file(d.path().join("README.md")).unwrap();
    assert!(pack(d.path(), &opts()).is_err());
    write(d.path(), "README.md", "x");
    let reserved = MANIFEST.replace("README.md", "package.kdl");
    write(d.path(), "package.kdl", &reserved);
    assert!(pack(d.path(), &opts()).is_err());
    let reserved = MANIFEST.replace("README.md", "variants/readme");
    write(d.path(), "package.kdl", &reserved);
    assert!(pack(d.path(), &opts()).is_err());
}

#[test]
fn an_about_file_inside_a_target_is_kept_once() {
    let d = tempfile::tempdir().unwrap();
    demo(d.path());
    let m = MANIFEST.replace("\"README.md\"", "\"crow/plugin.json\"");
    write(d.path(), "package.kdl", &m);
    let egg = pack(
        d.path(),
        &PackOptions {
            base: true,
            ..opts()
        },
    )
    .unwrap()
    .pop()
    .unwrap();
    assert_eq!(
        paths(&egg)
            .iter()
            .filter(|p| **p == "crow/plugin.json")
            .count(),
        1
    );
}

#[test]
fn a_missing_target_folder_is_an_error() {
    let d = tempfile::tempdir().unwrap();
    demo(d.path());
    fs::remove_dir_all(d.path().join("crow")).unwrap();
    let err = pack(d.path(), &opts()).unwrap_err();
    assert!(err.to_string().contains("crow"), "{err}");
}

// Opening hostile or broken eggs.

fn entry(path: &str, data: &str) -> Entry {
    Entry {
        path: path.into(),
        data: data.to_string().into(),
        executable: false,
    }
}

#[test]
fn opening_refuses_broken_eggs() {
    assert!(matches!(
        Egg::open(&b"not an archive"[..]),
        Err(Error::UnsafeArchive(_))
    ));
    // No manifest.
    assert!(Egg::from_entries(vec![entry("crow/a", "a")]).is_err());
    // Both kinds of header.
    let both = Egg::from_entries(vec![
        entry("package.kdl", MANIFEST),
        entry("variant.kdl", "variant {\n}\n"),
    ]);
    assert!(both.is_err());
    // Something that is not a variant egg under variants/.
    let junk = Egg::from_entries(vec![
        entry("package.kdl", MANIFEST),
        entry("variants/x.egg", "junk"),
    ]);
    assert!(junk.is_err());
}

#[test]
fn opening_checks_embedded_variants() {
    let d = tempfile::tempdir().unwrap();
    demo(d.path());
    let eggs = pack(d.path(), &opts()).unwrap();
    let base = &eggs[3];
    // Wrong file name for what the header says.
    let mut entries = base.entries().to_vec();
    entries.push(Entry {
        path: "variants/demo-1.2.0.rookery.other.egg".into(),
        data: eggs[0].bytes().clone(),
        executable: false,
    });
    assert!(Egg::from_entries(entries).is_err());
    // A variant of another version.
    let newer = MANIFEST.replace("1.2.0", "1.3.0");
    let other = tempfile::tempdir().unwrap();
    demo(other.path());
    write(other.path(), "package.kdl", &newer);
    let foreign = pack(other.path(), &opts()).unwrap();
    let mut entries = base.entries().to_vec();
    entries.push(Entry {
        path: format!("variants/{}", foreign[0].file_name()),
        data: foreign[0].bytes().clone(),
        executable: false,
    });
    assert!(Egg::from_entries(entries).is_err());
}

#[test]
fn variant_eggs_must_keep_to_their_folder() {
    let header = "format 1\nvariant {\n  group g\n  id demo\n  version \"1.0.0\"\n  target crow\n  os linux\n}\n";
    let ok = Egg::from_entries(vec![
        entry("variant.kdl", header),
        entry("crow/variants/linux/a", "a"),
    ])
    .unwrap();
    assert_eq!(ok.variant().unwrap().key(), "linux");
    let bad = Egg::from_entries(vec![
        entry("variant.kdl", header),
        entry("crow/other/a", "a"),
    ]);
    assert!(bad.is_err());
}

#[test]
fn egg_read_reads_a_file() {
    let d = tempfile::tempdir().unwrap();
    demo(d.path());
    let egg = pack(
        d.path(),
        &PackOptions {
            base: true,
            ..opts()
        },
    )
    .unwrap()
    .pop()
    .unwrap();
    let f = d.path().join("out.egg");
    fs::write(&f, egg.bytes()).unwrap();
    assert_eq!(Egg::read(&f).unwrap().sha256(), egg.sha256());
    assert!(Egg::read(&d.path().join("missing.egg")).is_err());
}

#[test]
fn tree_digest_is_stable_and_sensitive() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    for d in [a.path(), b.path()] {
        write(d, "x/one.txt", "1");
        write(d, "two.txt", "2");
    }
    let same = hatchery::tree_digest(a.path()).unwrap();
    assert_eq!(same, hatchery::tree_digest(b.path()).unwrap());
    write(b.path(), "two.txt", "3");
    assert_ne!(same, hatchery::tree_digest(b.path()).unwrap());
    write(b.path(), "two.txt", "2");
    fs::rename(b.path().join("two.txt"), b.path().join("x/two.txt")).unwrap();
    assert_ne!(same, hatchery::tree_digest(b.path()).unwrap());
}
