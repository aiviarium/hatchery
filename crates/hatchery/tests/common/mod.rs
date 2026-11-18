//! Helpers shared by the integration tests: package folders, local git
//! repositories and a fixed clock. Each test file uses what it needs.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use hatchery::Git;
use hatchery::Timestamp;
use hatchery::time::Clock;

pub mod repo;

/// Writes a file, creating its folders.
pub fn write(dir: &Path, rel: &str, content: impl AsRef<[u8]>) -> PathBuf {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, content).unwrap();
    p
}

/// A package folder with a `package.kdl` and the given files.
pub fn package(dir: &Path, manifest: &str, files: &[(&str, &str)]) -> PathBuf {
    write(dir, "package.kdl", manifest);
    for (rel, content) in files {
        write(dir, rel, content);
    }
    dir.to_path_buf()
}

/// A clock that always says the same time.
pub fn clock_at(s: &str) -> Clock {
    let t = Timestamp::parse(s).unwrap();
    Arc::new(move || t)
}

/// The variables that keep git from reading the user's configuration and
/// give it a fixed identity and dates.
pub const GIT_ENV: [(&str, &str); 8] = [
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_CONFIG_GLOBAL", NO_CONFIG),
    ("GIT_AUTHOR_NAME", "Test"),
    ("GIT_AUTHOR_EMAIL", "test@example.org"),
    ("GIT_COMMITTER_NAME", "Test"),
    ("GIT_COMMITTER_EMAIL", "test@example.org"),
    ("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z"),
    ("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z"),
];

/// An empty configuration file.
#[cfg(windows)]
const NO_CONFIG: &str = "NUL";
/// An empty configuration file.
#[cfg(not(windows))]
const NO_CONFIG: &str = "/dev/null";

/// The library's [`Git`] for tests: it runs with [`GIT_ENV`], so nothing in
/// the developer's git configuration changes a result. Give it to every
/// client and registry a test builds.
pub fn isolated_git() -> Git {
    GIT_ENV.iter().fold(Git::new(), |g, (k, v)| g.env(*k, *v))
}

/// Runs git with a fixed identity, dates and no user or system config, and
/// returns its trimmed standard output.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .envs(GIT_ENV)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A new repository on branch `main`.
pub fn git_init(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
}

/// Stages everything and commits; returns the commit.
pub fn git_commit(dir: &Path, message: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}
