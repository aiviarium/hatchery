//! Tests of the git wrapper and its cache that reach past the public API.
//! They use the repository of `tests/common/repo.rs`.

use std::fs;
use std::path::Path;

use crate::block_on;
use crate::error::Error;
use crate::source::git::publish_branch;
use crate::source::{GitFolder, GitRegistry, Source};
use crate::test_common::repo::{Fixture, alpha, built_alpha, commit_attributes, now, versions};
use crate::test_common::{git, isolated_git};

#[test]
fn the_git_wrapper_reports_failures_and_lists_remotes() {
    let f = Fixture::new();
    let g = isolated_git();
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
    let g = isolated_git();

    assert!(g.ls_remote(&evil).is_err());
    assert!(g.cache(&f.cache, &evil).is_err());
    assert!(g.ls_remote("ssh://host/\nrepo").is_err());
    let reg = GitRegistry::new(&evil, &f.cache, isolated_git(), None);
    assert!(block_on(reg.names()).is_err());
    let folder = GitFolder::new(&evil, None, None, &f.cache, isolated_git());
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
    let s = GitFolder::new(&f.url, None, Some("--foo".into()), &f.cache, isolated_git());
    assert!(block_on(s.names()).is_err());
}

#[test]
fn remote_helpers_and_plain_http_are_refused() {
    let f = Fixture::new();
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("PWNED");
    let ext = format!("ext::sh -c touch% {}", marker.display());
    let g = isolated_git();

    assert!(matches!(
        g.ls_remote(&ext).unwrap_err(),
        Error::Invalid { .. }
    ));
    assert!(g.cache(&f.cache, &ext).is_err());
    let reg = GitRegistry::new(&ext, &f.cache, isolated_git(), None);
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
    let err = isolated_git().ls_remote(secret).unwrap_err();
    assert!(!err.to_string().contains("ghp_SECRET"), "{err}");
    assert!(matches!(err, Error::Git { .. }));
    let reg = GitRegistry::new(secret, &f.cache, isolated_git(), None);
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
        isolated_git(),
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
fn a_lock_file_left_behind_does_not_block_and_stays() {
    let f = Fixture::new();
    let g = isolated_git();
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

#[test]
fn an_export_is_the_committed_bytes_whatever_the_configuration() {
    let f = Fixture::new();
    let commit = commit_attributes(&f);
    let cache = isolated_git().cache(&f.cache, &f.url).unwrap();
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
    let cache = isolated_git().cache(&f.cache, &f.url).unwrap();

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
