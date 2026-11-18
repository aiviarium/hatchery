//! Runs the `hatchery` binary in temporary folders, with its own
//! configuration and cache.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

const MANIFEST: &str = r#"format 1

package {
  group example
  id demo
  version "1.0.0"
}

about {
  name "Demo"
  summary "A made-up package."
  license MIT
}

targets {
  crow {
    requires {
      crow "^1"
    }
  }
  rookery manifest="plugin.json" {
    variants {
      variant arch=x86_64
    }
  }
}
"#;

const SCHEMA: &str = r#"{
  "type": "object",
  "required": ["name"],
  "properties": { "name": { "type": "string" } }
}"#;

struct Env {
    tmp: TempDir,
}

struct Ran {
    out: Output,
}

impl Ran {
    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.out.stdout).into_owned()
    }

    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.out.stderr).into_owned()
    }

    fn ok(self) -> Ran {
        assert!(
            self.out.status.success(),
            "failed: {}\n{}",
            self.stdout(),
            self.stderr()
        );
        self
    }

    fn fails(self) -> Ran {
        assert!(!self.out.status.success(), "succeeded: {}", self.stdout());
        assert_eq!(self.out.status.code(), Some(1));
        self
    }
}

impl Env {
    fn new() -> Env {
        Env {
            tmp: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.path(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    fn package(&self) -> PathBuf {
        self.write("pkg/package.kdl", MANIFEST);
        self.write("pkg/crow/crow.json", r#"{"name": "demo"}"#);
        self.write("pkg/rookery/plugin.json", r#"{"name": "demo"}"#);
        self.write("pkg/rookery/variants/x86_64/tool", "binary\n");
        self.path("pkg")
    }

    fn run(&self, args: &[&str]) -> Ran {
        self.run_env(args, &[])
    }

    fn run_env(&self, args: &[&str], env: &[(&str, &str)]) -> Ran {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_hatchery"));
        cmd.args(args)
            .current_dir(self.tmp.path())
            .env("HATCHERY_CONFIG", self.path("config/hatchery.kdl"))
            .env("HATCHERY_CACHE", self.path("cache"))
            .env("XDG_CONFIG_HOME", self.path("xdg-config"))
            .env("HOME", self.path("home"))
            .env("SOURCE_DATE_EPOCH", "1767225600")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.org")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.org");
        for (k, v) in env {
            cmd.env(k, v);
        }
        Ran {
            out: cmd.output().unwrap(),
        }
    }
}

fn lines(s: &str) -> Vec<String> {
    s.lines().map(str::to_string).collect()
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn version() {
    let env = Env::new();
    let out = env.run(&["--version"]).ok().stdout();
    assert_eq!(
        out.trim(),
        format!("hatchery {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn pack_is_reproducible() {
    let env = Env::new();
    let pkg = env.package();
    let a = env
        .run(&["pack", s(&pkg), "--out", s(&env.path("one"))])
        .ok();
    let b = env
        .run(&["pack", s(&pkg), "--out", s(&env.path("two"))])
        .ok();
    assert_eq!(lines(&a.stdout()), lines(&b.stdout()));
    let names = a.stdout();
    assert!(names.contains("demo-1.0.0.egg"), "{names}");
    assert!(names.contains("demo-1.0.0.rookery.x86_64.egg"), "{names}");
    assert!(env.path("one/demo-1.0.0.egg").is_file());
    assert_eq!(
        fs::read(env.path("one/demo-1.0.0.egg")).unwrap(),
        fs::read(env.path("two/demo-1.0.0.egg")).unwrap()
    );
}

#[test]
fn pack_one_variant() {
    let env = Env::new();
    let pkg = env.package();
    let out = env
        .run(&[
            "pack",
            s(&pkg),
            "--variant",
            "rookery",
            "arch=x86_64",
            "--out",
            s(&env.path("v")),
        ])
        .ok()
        .stdout();
    assert!(out.contains("demo-1.0.0.rookery.x86_64.egg"), "{out}");
    assert!(!env.path("v/demo-1.0.0.egg").exists());
}

#[test]
fn check_passes_and_fails() {
    let env = Env::new();
    let pkg = env.package();
    env.write("schema.json", SCHEMA);
    let schema = env.path("schema.json");
    let check = format!("rookery={}", s(&schema));

    env.run(&[
        "pack",
        s(&pkg),
        "--base",
        "--check",
        &check,
        "--out",
        s(&env.path("good")),
    ])
    .ok();
    assert!(env.path("good/demo-1.0.0.egg").is_file());

    env.write("pkg/rookery/plugin.json", r#"{"name": 5}"#);
    let bad = env
        .run(&[
            "pack",
            s(&pkg),
            "--base",
            "--check",
            &check,
            "--out",
            s(&env.path("bad")),
        ])
        .fails();
    let err = bad.stderr();
    assert!(err.contains("plugin.json"), "{err}");
    assert!(err.contains("/name"), "{err}");
    assert!(!env.path("bad").exists());
}

#[test]
fn inspect_base_and_json() {
    let env = Env::new();
    let pkg = env.package();
    env.run(&["pack", s(&pkg), "--out", s(&env.path("dist"))])
        .ok();
    let egg = env.path("dist/demo-1.0.0.egg");

    let text = env.run(&["inspect", s(&egg)]).ok().stdout();
    assert!(text.contains("example/demo 1.0.0 (base egg)"), "{text}");
    assert!(text.contains("crow ^1"), "{text}");
    assert!(text.contains("x86_64"), "{text}");

    let json = env.run(&["inspect", s(&egg), "--json"]).ok().stdout();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["kind"], "base");
    assert_eq!(v["name"], "example/demo");
    assert_eq!(v["targets"][0]["name"], "crow");
}

#[test]
fn keygen_prints_a_pair() {
    let env = Env::new();
    let ran = env.run(&["keygen"]).ok();
    let out = ran.stdout();
    let keys = lines(&out);
    assert_eq!(keys.len(), 2, "{out}");
    assert!(
        keys.iter()
            .all(|k| k.starts_with("ed25519:") && k.len() == 8 + 64)
    );
    assert_ne!(keys[0], keys[1]);
    assert!(!ran.stderr().contains(&keys[0]));
    let again = lines(&env.run(&["keygen"]).ok().stdout());
    assert_ne!(again[0], keys[0]);
}

#[test]
fn publish_verify_and_register() {
    let env = Env::new();
    let pkg = env.package();
    env.run(&["pack", s(&pkg), "--out", s(&env.path("dist"))])
        .ok();
    let base = env.path("dist/demo-1.0.0.egg");
    let variant = env.path("dist/demo-1.0.0.rookery.x86_64.egg");

    let keys = lines(&env.run(&["keygen"]).ok().stdout());
    let (secret, public) = (&keys[0], &keys[1]);
    let reg = env.path("registry");

    // The base egg is listed first; the variant is still published before it.
    let out = env
        .run_env(
            &[
                "publish",
                s(&base),
                s(&variant),
                "--to",
                s(&reg),
                "--sign-key-env",
                "SIGN",
            ],
            &[("SIGN", secret)],
        )
        .ok()
        .stdout();
    let at = |name: &str| out.find(name).unwrap_or_else(|| panic!("{name} in {out}"));
    assert!(
        at("demo-1.0.0.rookery.x86_64.egg") < at("demo-1.0.0.egg  "),
        "{out}"
    );
    assert!(out.contains("signed"), "{out}");
    assert!(reg.join("example/demo/index.json").is_file());

    // The same bytes again are accepted.
    env.run_env(
        &[
            "publish",
            s(&base),
            "--to",
            s(&reg),
            "--sign-key-env",
            "SIGN",
        ],
        &[("SIGN", secret)],
    )
    .ok();

    let ok = env
        .run(&["verify", s(&base), "--registry", s(&reg), "--key", public])
        .ok()
        .stdout();
    assert!(ok.contains("signature is valid"), "{ok}");
    env.run(&[
        "verify",
        s(&variant),
        "--registry",
        s(&reg),
        "--key",
        public,
    ])
    .ok();

    let other = lines(&env.run(&["keygen"]).ok().stdout());
    env.run(&[
        "verify",
        s(&base),
        "--registry",
        s(&reg),
        "--key",
        &other[1],
    ])
    .fails();
    env.run(&["verify", s(&base)]).ok();

    // A registry line in the configuration.
    env.run(&[
        "registry",
        "add",
        "dir",
        s(&reg),
        "--key",
        public,
        "--only",
        "example",
    ])
    .ok();
    let list = env.run(&["registry", "list"]).ok().stdout();
    assert!(list.contains("dir") && list.contains(s(&reg)), "{list}");
    let kdl = fs::read_to_string(env.path("config/hatchery.kdl")).unwrap();
    assert!(
        kdl.contains("only=") && kdl.contains(public.as_str()),
        "{kdl}"
    );
}

#[test]
fn install_list_remove_fetch_withdraw() {
    let env = Env::new();
    let pkg = env.package();
    env.run(&["pack", s(&pkg), "--out", s(&env.path("dist"))])
        .ok();
    let reg = env.path("registry");
    for egg in ["demo-1.0.0.rookery.x86_64.egg", "demo-1.0.0.egg"] {
        let p = env.path(&format!("dist/{egg}"));
        env.run(&["publish", s(&p), "--to", s(&reg)]).ok();
    }
    fs::create_dir_all(env.path("config")).unwrap();
    env.run(&["registry", "add", "dir", s(&reg)]).ok();

    let dir = env.path("installed");
    let added = env
        .run(&[
            "add",
            "example/demo",
            "--dir",
            s(&dir),
            "--target",
            "crow",
            "--fact",
            "crow=1.2.0",
        ])
        .ok()
        .stdout();
    assert!(added.contains("installed example/demo 1.0.0"), "{added}");
    assert!(dir.join("eggs.lock").is_file());
    assert!(dir.join("example/demo/package.kdl").is_file());

    let list = env.run(&["list", "--dir", s(&dir)]).ok().stdout();
    assert!(
        list.contains("example/demo") && list.contains("1.0.0"),
        "{list}"
    );
    let json = env.run(&["list", "--dir", s(&dir), "--json"]).ok().stdout();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v[0]["name"], "example/demo");
    assert_eq!(v[0]["targets"][0], "crow");

    // No tool named, no install.
    env.run(&["add", "example/demo", "--dir", s(&dir)]).fails();

    env.run(&["remove", "example/demo", "--dir", s(&dir)]).ok();
    let after = env.run(&["list", "--dir", s(&dir)]).ok().stdout();
    assert!(!after.contains("example/demo"), "{after}");

    // One fat egg from the registry.
    let fat = env.path("fat");
    env.run(&["fetch", "example/demo@1.0.0", "--fat", "--out", s(&fat)])
        .ok();
    let egg = fat.join("demo-1.0.0.egg");
    assert!(egg.is_file());
    let text = env.run(&["inspect", s(&egg)]).ok().stdout();
    assert!(text.contains("(fat egg)"), "{text}");
    assert!(text.contains("x86_64 (inside)"), "{text}");

    env.run(&[
        "withdraw",
        "example/demo@1.0.0",
        "--to",
        s(&reg),
        "--reason",
        "broken",
    ])
    .ok();
    let index = fs::read_to_string(reg.join("example/demo/index.json")).unwrap();
    assert!(index.contains("broken"), "{index}");
}

#[test]
fn errors_print_and_exit_one() {
    let env = Env::new();
    let ran = env.run(&["inspect", "nothing.egg"]).fails();
    assert!(ran.stderr().starts_with("error: "), "{}", ran.stderr());
    env.run(&["publish", "nothing.egg", "--to", "x"]).fails();
    // Not inside a repository with a packages.kdl.
    let pkg = env.package();
    env.run(&["pack", s(&pkg), "--out", s(&env.path("dist"))])
        .ok();
    let ran = env
        .run(&["publish", s(&env.path("dist/demo-1.0.0.egg"))])
        .fails();
    assert!(ran.stderr().contains("--to"), "{}", ran.stderr());
}
