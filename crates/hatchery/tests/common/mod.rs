//! Helpers shared by the integration tests: package folders, local git
//! repositories and a fixed clock. Each test file uses what it needs.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use hatchery::Timestamp;
use hatchery::time::Clock;

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

/// Runs git with a fixed identity, dates and no user or system config, and
/// returns its trimmed standard output.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.org")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.org")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
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
