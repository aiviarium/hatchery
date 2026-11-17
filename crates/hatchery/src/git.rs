//! The `git` command, and the per-repository cache of shallow, sparse
//! checkouts.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use crate::digest::sha256_hex;
use crate::egg::Egg;
use crate::error::{Error, IoContext, Result};
use crate::manifest::{self, Defaults, Manifest, Origin};
use crate::pack::{self, PackOptions, VariantPick};
use crate::time::Timestamp;

/// How long to wait for another user of a cache folder.
const LOCK_WAIT: Duration = Duration::from_secs(60);
/// A lock file older than this is left over from a process that died.
const LOCK_STALE: Duration = Duration::from_secs(10 * 60);

/// How to run git.
#[derive(Debug, Clone)]
pub struct Git {
    pub program: PathBuf,
}

impl Default for Git {
    fn default() -> Self {
        Git {
            program: PathBuf::from("git"),
        }
    }
}

/// What `git ls-remote` says about a repository.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Remote {
    /// The branch `HEAD` points at.
    pub default_branch: Option<String>,
    /// Branch name to commit.
    pub branches: BTreeMap<String, String>,
    /// Tag name to commit; annotated tags are peeled.
    pub tags: BTreeMap<String, String>,
}

impl Remote {
    /// The commit of the default branch.
    pub fn default_commit(&self) -> Option<&str> {
        self.default_branch
            .as_ref()
            .and_then(|b| self.branches.get(b))
            .map(String::as_str)
    }
}

impl Git {
    pub fn new() -> Git {
        Git::default()
    }

    /// Runs git in a folder (or where the process is when None) and returns
    /// its standard output.
    pub fn run(&self, dir: Option<&Path>, args: &[&str]) -> Result<Vec<u8>> {
        self.exec(dir, &[], args, &[])
    }

    /// Runs git and returns its standard output as text, without the final
    /// newline.
    pub fn run_text(&self, dir: Option<&Path>, args: &[&str]) -> Result<String> {
        let out = self.run(dir, args)?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }

    /// `config` goes before the subcommand as `-c` options.
    pub(crate) fn exec(
        &self,
        dir: Option<&Path>,
        config: &[&str],
        args: &[&str],
        env: &[(&str, &str)],
    ) -> Result<Vec<u8>> {
        let mut cmd = Command::new(&self.program);
        // Local repositories are remotes in tests and in some set-ups.
        cmd.args(["-c", "protocol.file.allow=always"]);
        // Files must come out as they went in, for the egg to be the same.
        cmd.args(["-c", "core.autocrlf=false"]);
        cmd.args(["-c", "advice.detachedHead=false"]);
        for c in config {
            cmd.args(["-c", c]);
        }
        cmd.args(args);
        if let Some(d) = dir {
            cmd.current_dir(d);
        }
        cmd.env("GIT_TERMINAL_PROMPT", "0");
        // A password prompt of ssh would hang a server.
        if std::env::var_os("GIT_SSH_COMMAND").is_none() && std::env::var_os("GIT_SSH").is_none() {
            cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::null());
        let shown = args.join(" ");
        let out = cmd.output().map_err(|e| Error::Git {
            args: shown.clone(),
            message: format!("cannot run {}: {e}", self.program.display()),
        })?;
        if out.status.success() {
            return Ok(out.stdout);
        }
        Err(Error::Git {
            args: shown,
            message: stderr_tail(&out.stderr, &out.status.to_string()),
        })
    }

    /// One `git ls-remote`: the default branch, the branches and the tags.
    pub fn ls_remote(&self, url: &str) -> Result<Remote> {
        let out = self.run_text(
            None,
            &[
                "ls-remote",
                "--symref",
                url,
                "HEAD",
                "refs/heads/*",
                "refs/tags/*",
            ],
        )?;
        Ok(parse_ls_remote(&out))
    }

    /// Opens the cache folder of a repository under `root`, creating it on
    /// first use, and holds its lock until the result is dropped.
    pub fn cache(&self, root: &Path, url: &str) -> Result<RepoCache> {
        fs::create_dir_all(root).at_path(root)?;
        let name: String = sha256_hex(url.as_bytes()).chars().take(16).collect();
        let lock = Lock::acquire(root.join(format!("{name}.lock")))?;
        let dir = root.join(&name);
        let cache = RepoCache {
            dir,
            git: self.clone(),
            url: url.to_string(),
            _lock: lock,
        };
        cache.init()?;
        Ok(cache)
    }
}

fn stderr_tail(stderr: &[u8], fallback: &str) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let from = lines.len().saturating_sub(8);
    let tail = lines[from..].join("\n");
    if tail.is_empty() {
        fallback.to_string()
    } else {
        tail
    }
}

fn parse_ls_remote(text: &str) -> Remote {
    let mut r = Remote::default();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("ref: ") {
            if let Some((target, name)) = rest.split_once('\t')
                && name == "HEAD"
            {
                r.default_branch = target.strip_prefix("refs/heads/").map(str::to_string);
            }
            continue;
        }
        let Some((sha, name)) = line.split_once('\t') else {
            continue;
        };
        if let Some(b) = name.strip_prefix("refs/heads/") {
            r.branches.insert(b.to_string(), sha.to_string());
        } else if let Some(t) = name.strip_prefix("refs/tags/") {
            // The peeled line follows the tag object's own line.
            let t = t.strip_suffix("^{}").unwrap_or(t);
            r.tags.insert(t.to_string(), sha.to_string());
        }
    }
    r
}

/// A lock file, removed when dropped.
#[derive(Debug)]
struct Lock {
    path: PathBuf,
}

impl Lock {
    fn acquire(path: PathBuf) -> Result<Lock> {
        let start = std::time::Instant::now();
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    use std::io::Write;
                    let _ = writeln!(f, "{}", std::process::id());
                    return Ok(Lock { path });
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| SystemTime::now().duration_since(t).ok())
                        .is_some_and(|age| age > LOCK_STALE);
                    if stale {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    if start.elapsed() > LOCK_WAIT {
                        return Err(Error::other(format!(
                            "{}: still locked by another process after {}s",
                            path.display(),
                            LOCK_WAIT.as_secs()
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(e).at_path(&path),
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// A folder removed when dropped, for checkouts and clones.
#[derive(Debug)]
pub(crate) struct Scratch(PathBuf);

impl Scratch {
    pub(crate) fn new() -> Result<Scratch> {
        let p = std::env::temp_dir().join(format!("hatchery-{}", crate::fsx::nonce()));
        fs::create_dir_all(&p).at_path(&p)?;
        Ok(Scratch(p))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The cache folder of one repository: a shallow, sparse, partial clone that
/// is reused for every package of the repository. Holds the folder's lock.
#[derive(Debug)]
pub struct RepoCache {
    dir: PathBuf,
    git: Git,
    url: String,
    _lock: Lock,
}

impl RepoCache {
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn run(&self, args: &[&str]) -> Result<Vec<u8>> {
        self.git.run(Some(&self.dir), args)
    }

    fn init(&self) -> Result<()> {
        if self.dir.join(".git").exists() {
            // The folder is named by the address, so this only repairs a
            // hand-edited configuration.
            let current = self
                .git
                .run_text(Some(&self.dir), &["remote", "get-url", "origin"]);
            if current.ok().as_deref() != Some(self.url.as_str()) {
                self.run(&["remote", "set-url", "origin", &self.url])?;
            }
            return Ok(());
        }
        fs::create_dir_all(&self.dir).at_path(&self.dir)?;
        self.run(&["init", "--quiet"])?;
        self.run(&["remote", "add", "origin", &self.url])?;
        Ok(())
    }

    /// Fetches a tag shallowly and returns its commit.
    pub fn fetch_tag(&self, tag: &str) -> Result<String> {
        let spec = format!("+refs/tags/{tag}:refs/tags/{tag}");
        self.fetch(&spec)?;
        self.rev_parse(&format!("refs/tags/{tag}^{{commit}}"))
    }

    /// Fetches a branch shallowly and returns its commit.
    pub fn fetch_branch(&self, branch: &str) -> Result<String> {
        let spec = format!("+refs/heads/{branch}:refs/remotes/origin/{branch}");
        self.fetch(&spec)?;
        self.rev_parse(&format!("refs/remotes/origin/{branch}^{{commit}}"))
    }

    /// Fetches a commit by its id. This needs a server that allows it;
    /// protocol version 2 does.
    pub fn fetch_commit(&self, commit: &str) -> Result<()> {
        let spec = format!("+{commit}:refs/hatchery/{commit}");
        self.fetch(&spec)?;
        let got = self.rev_parse(&format!("{commit}^{{commit}}"))?;
        if got != commit {
            return Err(Error::other(format!(
                "{}: asked for commit {commit}, got {got}",
                self.url
            )));
        }
        Ok(())
    }

    fn fetch(&self, refspec: &str) -> Result<()> {
        self.git.exec(
            Some(&self.dir),
            &["protocol.version=2"],
            &[
                "fetch",
                "--quiet",
                "--depth",
                "1",
                "--filter=blob:none",
                "--no-tags",
                "origin",
                refspec,
            ],
            &[],
        )?;
        Ok(())
    }

    fn rev_parse(&self, what: &str) -> Result<String> {
        self.git
            .run_text(Some(&self.dir), &["rev-parse", "--verify", what])
    }

    /// Whether the commit is in the cache already.
    pub fn has_commit(&self, commit: &str) -> bool {
        self.git
            .exec(
                Some(&self.dir),
                &[],
                &["cat-file", "-e", &format!("{commit}^{{commit}}")],
                &[("GIT_NO_LAZY_FETCH", "1")],
            )
            .is_ok()
    }

    /// The bytes of a file at a commit. None when the commit has no such
    /// file.
    pub fn show(&self, commit: &str, path: &str) -> Result<Option<Vec<u8>>> {
        let listed = self.git.run_text(
            Some(&self.dir),
            &["ls-tree", "--format=%(objecttype)", commit, "--", path],
        )?;
        if listed != "blob" {
            return Ok(None);
        }
        self.run(&["show", &format!("{commit}:{path}")]).map(Some)
    }

    /// Every folder that holds a `package.kdl` at a commit, `/`-separated,
    /// not counting the root.
    pub fn manifest_dirs(&self, commit: &str) -> Result<Vec<String>> {
        let out = self.run(&["ls-tree", "-r", "--name-only", "-z", commit])?;
        let mut dirs: Vec<String> = out
            .split(|b| *b == 0)
            .filter_map(|p| std::str::from_utf8(p).ok())
            .filter_map(|p| p.strip_suffix(manifest::FILE))
            .filter_map(|d| d.strip_suffix('/'))
            .map(str::to_string)
            .collect();
        dirs.sort();
        Ok(dirs)
    }

    /// The commit's time.
    pub fn commit_time(&self, commit: &str) -> Result<Timestamp> {
        let t = self
            .git
            .run_text(Some(&self.dir), &["log", "-1", "--format=%ct", commit])?;
        t.parse::<i64>()
            .map(Timestamp::from_secs)
            .map_err(|_| Error::other(format!("git gave {t:?} as the time of {commit}")))
    }

    /// Checks out one folder of a commit (the whole repository when `path`
    /// is None) and copies it to `dest`, which must not exist yet. Nothing
    /// else of the repository is checked out. Symlinks are refused.
    pub fn export(&self, commit: &str, path: Option<&str>, dest: &Path) -> Result<()> {
        match path {
            Some(p) => {
                manifest::validate_rel_path("path", p)?;
                self.run(&["sparse-checkout", "set", "--cone", "--", p])?;
            }
            None => {
                self.run(&["sparse-checkout", "disable"])?;
            }
        }
        self.run(&["checkout", "--quiet", "--force", "--detach", commit])?;
        let src = match path {
            Some(p) => self.dir.join(p),
            None => self.dir.clone(),
        };
        if !src.is_dir() {
            return Err(Error::invalid(
                path.unwrap_or("."),
                format!("is not a folder at commit {commit}"),
            ));
        }
        copy_tree(&src, dest, true)
    }
}

/// Copies a folder, refusing symlinks. `.git` at the top is not copied.
fn copy_tree(src: &Path, dest: &Path, top: bool) -> Result<()> {
    fs::create_dir_all(dest).at_path(dest)?;
    let mut entries = fs::read_dir(src)
        .at_path(src)?
        .collect::<std::io::Result<Vec<_>>>()
        .at_path(src)?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name();
        if top && name == ".git" {
            continue;
        }
        let from = e.path();
        let to = dest.join(&name);
        let meta = fs::symlink_metadata(&from).at_path(&from)?;
        if meta.file_type().is_symlink() {
            return Err(Error::invalid(
                from.display(),
                "is a symlink, which a package cannot hold",
            ));
        }
        if meta.is_dir() {
            copy_tree(&from, &to, false)?;
        } else {
            fs::copy(&from, &to).at_path(&from)?;
        }
    }
    Ok(())
}

/// `dir/file`, or `file` for the root.
pub(crate) fn join_path(dir: Option<&str>, file: &str) -> String {
    match dir {
        Some(d) if !d.is_empty() => format!("{}/{file}", d.trim_end_matches('/')),
        _ => file.to_string(),
    }
}

/// The `defaults` of the `packages.kdl` at a commit, when it has one.
pub(crate) fn defaults_at(cache: &RepoCache, commit: &str) -> Result<Option<Defaults>> {
    match cache.show(commit, crate::repo::FILE)? {
        Some(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let m = crate::repo::RepoManifest::parse(&text).map_err(|e| match e {
                Error::Invalid { message, .. } => {
                    Error::invalid(format!("{} at {commit}", crate::repo::FILE), message)
                }
                e => e,
            })?;
            Ok(Some(m.defaults))
        }
        None => Ok(None),
    }
}

/// The manifest of the package in a folder at a commit, with the
/// repository's defaults, and those defaults.
pub(crate) fn manifest_at(
    cache: &RepoCache,
    commit: &str,
    path: Option<&str>,
) -> Result<(Manifest, Option<Defaults>)> {
    let file = join_path(path, manifest::FILE);
    let bytes = cache
        .show(commit, &file)?
        .ok_or_else(|| Error::invalid(&file, format!("there is no {file} at commit {commit}")))?;
    let defaults = defaults_at(cache, commit)?;
    let text = String::from_utf8_lossy(&bytes);
    let m = Manifest::parse_with(&text, defaults.as_ref()).map_err(|e| match e {
        Error::Invalid { message, .. } => Error::invalid(format!("{file} at {commit}"), message),
        e => e,
    })?;
    Ok((m, defaults))
}

/// A package folder checked out of a commit, in a temporary folder.
pub(crate) struct Checkout {
    scratch: Scratch,
    pub manifest: Manifest,
    pub defaults: Option<Defaults>,
    pub time: Timestamp,
}

impl Checkout {
    fn dir(&self) -> PathBuf {
        self.scratch.path().join("package")
    }
}

/// Checks out a package folder at a commit that the cache already has.
pub(crate) fn checkout_package(
    cache: &RepoCache,
    commit: &str,
    path: Option<&str>,
) -> Result<Checkout> {
    let (manifest, defaults) = manifest_at(cache, commit, path)?;
    let time = cache.commit_time(commit)?;
    let scratch = Scratch::new()?;
    cache.export(commit, path, &scratch.path().join("package"))?;
    Ok(Checkout {
        scratch,
        manifest,
        defaults,
        time,
    })
}

/// Packs a checkout: the base egg, or the one variant egg `(target, key)`.
/// The egg is unsigned and does not depend on when it is packed.
pub(crate) fn pack_checkout(
    co: &Checkout,
    url: &str,
    commit: &str,
    variant: Option<(&str, &str)>,
) -> Result<Egg> {
    let mut opts = PackOptions {
        no_build: true,
        defaults: co.defaults.clone(),
        origin: Some(Origin {
            repository: Some(url.to_string()),
            commit: Some(commit.to_string()),
            built: Some(co.time),
            hatchery: Some(crate::VERSION.into()),
            extra: vec![],
        }),
        ..Default::default()
    };
    match variant {
        None => opts.base = true,
        Some((target, key)) => {
            let spec = co
                .manifest
                .target(target)
                .and_then(|t| t.variant(key))
                .ok_or_else(|| {
                    Error::invalid(
                        manifest::FILE,
                        format!("target {target} has no variant {key}"),
                    )
                })?;
            opts.variants.push(VariantPick {
                target: target.to_string(),
                os: spec.os.clone(),
                arch: spec.arch.clone(),
            });
        }
    }
    let eggs = pack::pack(&co.dir(), &opts)?;
    let want_variant = variant.is_some();
    eggs.into_iter()
        .rev()
        .find(|e| e.variant().is_some() == want_variant)
        .ok_or_else(|| Error::other("packing gave no egg"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ls_remote_output() {
        let text = "ref: refs/heads/trunk\tHEAD\n\
                    aaa\tHEAD\n\
                    aaa\trefs/heads/trunk\n\
                    bbb\trefs/heads/dev\n\
                    ccc\trefs/tags/v1.0.0\n\
                    ddd\trefs/tags/v2.0.0\n\
                    eee\trefs/tags/v2.0.0^{}";
        let r = parse_ls_remote(text);
        assert_eq!(r.default_branch.as_deref(), Some("trunk"));
        assert_eq!(r.default_commit(), Some("aaa"));
        assert_eq!(r.branches.len(), 2);
        assert_eq!(r.tags["v1.0.0"], "ccc");
        assert_eq!(r.tags["v2.0.0"], "eee");
    }

    #[test]
    fn an_empty_remote_has_nothing() {
        assert_eq!(parse_ls_remote(""), Remote::default());
    }
}
