//! The `git` command, and the per-repository cache of shallow, partial
//! clones.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::digest::sha256_hex;
use crate::egg::Egg;
use crate::error::{Error, IoContext, Result};
use crate::manifest::{self, Defaults, Manifest, Origin};
use crate::pack::{self, PackOptions, VariantPick};
use crate::time::Timestamp;

/// How long to wait for another user of a cache folder.
const LOCK_WAIT: Duration = Duration::from_secs(60);

/// How to run git.
#[derive(Debug, Clone)]
pub struct Git {
    /// The program to run, a path or a name found on `PATH`. `git` by default.
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
    /// The same as [`Git::default`]. It runs `git` from `PATH`.
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

    /// A git command ready to run. `config` goes before the subcommand as
    /// `-c` options.
    fn command(
        &self,
        dir: Option<&Path>,
        config: &[&str],
        args: &[&str],
        env: &[(&str, &str)],
    ) -> Command {
        let mut cmd = Command::new(&self.program);
        // Local repositories are remotes in tests and in some set-ups.
        cmd.args(["-c", "protocol.file.allow=always"]);
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
        cmd
    }

    pub(crate) fn exec(
        &self,
        dir: Option<&Path>,
        config: &[&str],
        args: &[&str],
        env: &[(&str, &str)],
    ) -> Result<Vec<u8>> {
        self.finish(
            self.command(dir, config, args, env).stdin(Stdio::null()),
            args,
        )
    }

    fn finish(&self, cmd: &mut Command, args: &[&str]) -> Result<Vec<u8>> {
        let shown = redact_text(&args.join(" "));
        let out = cmd.output().map_err(|e| Error::Git {
            args: shown.clone(),
            message: format!("cannot run {}: {e}", self.program.display()),
        })?;
        if out.status.success() {
            return Ok(out.stdout);
        }
        Err(Error::Git {
            args: shown,
            message: redact_text(&stderr_tail(&out.stderr, &out.status.to_string())),
        })
    }

    /// One `git ls-remote`: the default branch, the branches and the tags.
    pub fn ls_remote(&self, url: &str) -> Result<Remote> {
        check_url(url)?;
        let out = self.run_text(
            None,
            &[
                "ls-remote",
                "--symref",
                "--",
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
        check_url(url)?;
        fs::create_dir_all(root).at_path(root)?;
        let name: String = sha256_hex(url.as_bytes()).chars().take(16).collect();
        let lock = lock_path(&root.join(format!("{name}.lock")))?;
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

/// Refuses an address git would take for an option, or that holds control
/// characters.
pub(crate) fn check_url(url: &str) -> Result<()> {
    if url.is_empty() || url.starts_with('-') || url.chars().any(char::is_control) {
        return Err(Error::invalid(
            crate::fsx::redact_url(url),
            "is not a valid git address",
        ));
    }
    Ok(())
}

/// Refuses a tag or branch name that git would take for an option or that
/// `git check-ref-format --allow-onelevel` would refuse.
pub(crate) fn check_ref(name: &str) -> Result<()> {
    let bad = name.is_empty()
        || name.starts_with(['-', '/', '.'])
        || name.ends_with(['/', '.'])
        || name.ends_with(".lock")
        || name.contains("..")
        || name.contains("//")
        || name.contains("@{")
        || name == "@"
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
        || name
            .split('/')
            .any(|c| c.starts_with('.') || c.ends_with(".lock"));
    if bad {
        return Err(Error::invalid(name, "is not a valid tag or branch name"));
    }
    Ok(())
}

/// A commit id: 40 hex digits (64 for SHA-256 repositories).
pub(crate) fn check_commit(commit: &str) -> Result<()> {
    if matches!(commit.len(), 40 | 64) && commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(Error::invalid(commit, "is not a commit id"))
    }
}

/// Replaces the credentials of every address in a text.
pub(crate) fn redact_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("://") {
        let start = rest[..i]
            .rfind(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | '`' | '<' | '('))
            .map_or(0, |p| p + 1);
        let tail = &rest[i..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | '`' | '>' | ')'))
            .map_or(rest.len(), |e| i + e);
        out.push_str(&rest[..start]);
        out.push_str(&crate::fsx::redact_url(&rest[start..end]));
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
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

/// A lock on a file, released when dropped or when the process dies. The
/// operating system holds it, so a run that was killed leaves nothing
/// behind to wait for. The file itself stays: removing it would let two
/// processes lock two different files at one path.
#[derive(Debug)]
pub(crate) struct FileLock {
    _file: fs::File,
}

/// Takes the lock on the file at `path`, creating it when needed and waiting
/// up to a minute for its holder.
pub(crate) fn lock_path(path: &Path) -> Result<FileLock> {
    lock_path_within(path, LOCK_WAIT)
}

fn lock_path_within(path: &Path, wait: Duration) -> Result<FileLock> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .at_path(path)?;
    let start = std::time::Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(FileLock { _file: file }),
            Err(fs::TryLockError::WouldBlock) => {
                if start.elapsed() >= wait {
                    return Err(Error::other(format!(
                        "{}: still locked by another process after {}s",
                        path.display(),
                        wait.as_secs()
                    )));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(fs::TryLockError::Error(e)) => return Err(e).at_path(path),
        }
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

/// The cache folder of one repository: a shallow, partial clone that
/// is reused for every package of the repository. Holds the folder's lock.
#[derive(Debug)]
pub struct RepoCache {
    dir: PathBuf,
    git: Git,
    url: String,
    _lock: FileLock,
}

impl RepoCache {
    /// The cache folder of the repository's checkout.
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
                self.run(&["remote", "set-url", "origin", "--", &self.url])?;
            }
            return Ok(());
        }
        fs::create_dir_all(&self.dir).at_path(&self.dir)?;
        self.run(&["init", "--quiet"])?;
        self.run(&["remote", "add", "origin", "--", &self.url])?;
        Ok(())
    }

    /// Fetches a tag shallowly and returns its commit.
    pub fn fetch_tag(&self, tag: &str) -> Result<String> {
        check_ref(tag)?;
        let spec = format!("+refs/tags/{tag}:refs/tags/{tag}");
        self.fetch(&spec)?;
        self.rev_parse(&format!("refs/tags/{tag}^{{commit}}"))
    }

    /// Fetches a branch shallowly and returns its commit.
    pub fn fetch_branch(&self, branch: &str) -> Result<String> {
        check_ref(branch)?;
        let spec = format!("+refs/heads/{branch}:refs/remotes/origin/{branch}");
        self.fetch(&spec)?;
        self.rev_parse(&format!("refs/remotes/origin/{branch}^{{commit}}"))
    }

    /// Fetches a commit by its id. This needs a server that allows it;
    /// protocol version 2 does.
    pub fn fetch_commit(&self, commit: &str) -> Result<()> {
        check_commit(commit)?;
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
        // `rev-list --missing=print` never fetches. `cat-file` would fetch a
        // commit the cache lacks, with its whole history, just to answer, and
        // only git 2.44 and later can be told not to.
        check_commit(commit).is_ok()
            && self
                .git
                .run(
                    Some(&self.dir),
                    &[
                        "rev-list",
                        "-n1",
                        "--missing=print",
                        &format!("{commit}^{{commit}}"),
                    ],
                )
                .is_ok()
    }

    /// The bytes of a file at a commit. None when the commit has no such
    /// file.
    pub fn show(&self, commit: &str, path: &str) -> Result<Option<Vec<u8>>> {
        check_commit(commit)?;
        manifest::validate_rel_path("path", path)?;
        let out = self.run(&["ls-tree", "-z", commit, "--", path])?;
        let is_blob = parse_ls_tree(&out)
            .iter()
            .any(|e| e.path == path && e.kind == "blob");
        if !is_blob {
            return Ok(None);
        }
        self.run(&["cat-file", "blob", &format!("{commit}:{path}")])
            .map(Some)
    }

    /// Every folder that holds a `package.kdl` at a commit, `/`-separated,
    /// not counting the root.
    pub fn manifest_dirs(&self, commit: &str) -> Result<Vec<String>> {
        check_commit(commit)?;
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
        check_commit(commit)?;
        let t = self
            .git
            .run_text(Some(&self.dir), &["log", "-1", "--format=%ct", commit])?;
        t.parse::<i64>()
            .map(Timestamp::from_secs)
            .map_err(|_| Error::other(format!("git gave {t:?} as the time of {commit}")))
    }

    /// Writes one folder of a commit (the whole repository when `path` is
    /// None) to `dest`, which must not exist yet. The files are the blobs of
    /// the commit as they are, with no attributes, line-ending conversion or
    /// filter applied, so the same commit gives the same bytes on every
    /// machine. Symlinks and submodules are refused.
    pub fn export(&self, commit: &str, path: Option<&str>, dest: &Path) -> Result<()> {
        check_commit(commit)?;
        let prefix = match path {
            Some(p) => {
                manifest::validate_rel_path("path", p)?;
                Some(p.strip_prefix("./").unwrap_or(p).trim_end_matches('/'))
            }
            None => None,
        };
        let mut args = vec!["ls-tree", "-r", "-z", commit];
        if let Some(p) = prefix {
            args.extend(["--", p]);
        }
        let listing = self.run(&args)?;
        let mut files = Vec::new();
        for e in parse_ls_tree(&listing) {
            let rel = match prefix {
                Some(p) => match e.path.strip_prefix(p).and_then(|r| r.strip_prefix('/')) {
                    Some(r) => r,
                    // The path is a file, not a folder.
                    None => continue,
                },
                None => e.path.as_str(),
            };
            manifest::validate_rel_path("path", rel)?;
            if rel.split('/').any(|c| c.eq_ignore_ascii_case(".git")) {
                return Err(Error::invalid(&e.path, "is inside a .git folder"));
            }
            match e.mode.as_str() {
                "100644" | "100755" | "100664" => files.push((rel.to_string(), e)),
                "120000" => {
                    return Err(Error::invalid(
                        &e.path,
                        "is a symlink, which a package cannot hold",
                    ));
                }
                "160000" => {
                    return Err(Error::invalid(
                        &e.path,
                        "is a submodule, which a package cannot hold",
                    ));
                }
                mode => {
                    return Err(Error::invalid(
                        &e.path,
                        format!("has the unsupported mode {mode}"),
                    ));
                }
            }
        }
        if files.is_empty() {
            return Err(Error::invalid(
                path.unwrap_or("."),
                format!("is not a folder at commit {commit}"),
            ));
        }
        let oids: Vec<&str> = files.iter().map(|(_, e)| e.oid.as_str()).collect();
        self.fetch_blobs(commit, &oids)?;
        fs::create_dir_all(dest).at_path(dest)?;
        let mut next = files.iter();
        self.read_blobs(&oids, |oid, content| {
            let (rel, entry) = next
                .next()
                .filter(|(_, e)| e.oid == oid)
                .ok_or_else(|| Error::other("git cat-file answered out of order"))?;
            let to = dest.join(rel);
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent).at_path(parent)?;
            }
            fs::write(&to, content).at_path(&to)?;
            set_executable(&to, entry.mode == "100755")
        })
    }

    /// Fetches the blobs of a commit that the cache lacks, in a few
    /// requests. Git would fetch each one when it is first read, with one
    /// connection for every file.
    fn fetch_blobs(&self, commit: &str, oids: &[&str]) -> Result<()> {
        // `--missing=print` lists what is not here instead of fetching it.
        let out = self.run(&["rev-list", "--objects", "--missing=print", commit])?;
        let text = String::from_utf8_lossy(&out);
        let absent: std::collections::BTreeSet<&str> =
            text.lines().filter_map(|l| l.strip_prefix('?')).collect();
        let missing: std::collections::BTreeSet<&str> = oids
            .iter()
            .copied()
            .filter(|o| absent.contains(o))
            .collect();
        let missing: Vec<&str> = missing.into_iter().collect();
        for chunk in missing.chunks(100) {
            let mut args = vec![
                "fetch",
                "--quiet",
                "--no-tags",
                "--recurse-submodules=no",
                "--filter=blob:none",
                "origin",
            ];
            args.extend(chunk);
            self.git.exec(
                Some(&self.dir),
                // Without this, a fetch in a shallow repository checks that
                // the blobs it asked for are commits, which fails. It is what
                // git itself uses to fetch a missing object.
                &["protocol.version=2", "fetch.negotiationAlgorithm=noop"],
                &args,
                &[],
            )?;
        }
        Ok(())
    }

    /// Runs one `git cat-file --batch` for the blobs and hands each, in
    /// order, to `each` with its id.
    fn read_blobs(
        &self,
        oids: &[&str],
        mut each: impl FnMut(&str, &[u8]) -> Result<()>,
    ) -> Result<()> {
        use std::io::{BufRead, BufReader, Read, Write};
        let mut child = self
            .git
            .command(Some(&self.dir), &[], &["cat-file", "--batch"], &[])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::Git {
                args: "cat-file --batch".into(),
                message: format!("cannot run {}: {e}", self.git.program.display()),
            })?;
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let mut stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
        let mut stderr = child.stderr.take().expect("stderr is piped");
        let wanted: Vec<String> = oids.iter().map(|o| o.to_string()).collect();
        // Written from another thread: git answers while it reads, and a full
        // pipe in both directions would stop both processes.
        let writer = std::thread::spawn(move || {
            for oid in wanted {
                if writeln!(stdin, "{oid}").is_err() {
                    break;
                }
            }
        });
        let errors = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf);
            buf
        });
        let result = (|| -> Result<()> {
            let mut content = Vec::new();
            for oid in oids {
                let mut header = String::new();
                stdout
                    .read_line(&mut header)
                    .map_err(|e| Error::other(format!("git cat-file: {e}")))?;
                let parts: Vec<&str> = header.split_whitespace().collect();
                let size = match parts.as_slice() {
                    [id, "blob", size] if id == oid => size.parse::<usize>().ok(),
                    _ => None,
                };
                let Some(size) = size else {
                    return Err(Error::other(format!(
                        "git cat-file did not give blob {oid}: {}",
                        header.trim_end()
                    )));
                };
                content.resize(size + 1, 0);
                stdout
                    .read_exact(&mut content)
                    .map_err(|e| Error::other(format!("git cat-file: {e}")))?;
                each(oid, &content[..size])?;
            }
            Ok(())
        })();
        drop(stdout);
        if result.is_err() {
            let _ = child.kill();
        }
        let _ = writer.join();
        let status = child
            .wait()
            .map_err(|e| Error::other(format!("git: {e}")))?;
        let stderr = errors.join().unwrap_or_default();
        result?;
        if !status.success() {
            return Err(Error::Git {
                args: "cat-file --batch".into(),
                message: redact_text(&stderr_tail(&stderr, &status.to_string())),
            });
        }
        Ok(())
    }
}

/// One line of `git ls-tree -z`.
#[derive(Debug, PartialEq, Eq)]
struct TreeEntry {
    mode: String,
    kind: String,
    oid: String,
    path: String,
}

/// Parses the default output of `git ls-tree -z`:
/// `<mode> SP <type> SP <oid> TAB <path>`, each ended by a NUL.
fn parse_ls_tree(out: &[u8]) -> Vec<TreeEntry> {
    out.split(|b| *b == 0)
        .filter_map(|rec| {
            let text = std::str::from_utf8(rec).ok()?;
            let (meta, path) = text.split_once('\t')?;
            let mut it = meta.split(' ');
            Some(TreeEntry {
                mode: it.next()?.to_string(),
                kind: it.next()?.to_string(),
                oid: it.next()?.to_string(),
                path: path.to_string(),
            })
        })
        .collect()
}

#[cfg(unix)]
fn set_executable(path: &Path, exec: bool) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if exec { 0o755 } else { 0o644 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).at_path(path)
}

#[cfg(not(unix))]
fn set_executable(_path: &Path, _exec: bool) -> Result<()> {
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
            repository: Some(crate::fsx::redact_url(url)),
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

    const HOLDER_ENV: &str = "HATCHERY_TEST_LOCK_HOLDER";

    /// Not a test of its own: the next test runs it in a child process, which
    /// takes the lock named by the environment and then waits to be killed.
    #[test]
    fn lock_holder() {
        let Some(path) = std::env::var_os(HOLDER_ENV) else {
            return;
        };
        let _lock = lock_path(Path::new(&path)).unwrap();
        use std::io::Write;
        let mut out = std::io::stdout();
        writeln!(out, "locked").unwrap();
        out.flush().unwrap();
        std::thread::sleep(Duration::from_secs(120));
    }

    #[test]
    fn a_killed_process_releases_its_lock() {
        use std::io::{BufRead, BufReader};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.lock");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "git::tests::lock_holder", "--nocapture"])
            .env(HOLDER_ENV, &path)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        while lines.next().unwrap().unwrap() != "locked" {}
        assert!(lock_path_within(&path, Duration::from_millis(200)).is_err());
        child.kill().unwrap();
        child.wait().unwrap();
        let start = std::time::Instant::now();
        lock_path_within(&path, Duration::from_secs(5)).unwrap();
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_second_lock_waits_for_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.lock");
        let first = lock_path(&path).unwrap();
        let err = lock_path_within(&path, Duration::from_millis(150)).unwrap_err();
        assert!(err.to_string().contains("still locked"), "{err}");
        drop(first);
        lock_path_within(&path, Duration::from_millis(150)).unwrap();
        assert!(path.exists(), "the lock file stays");
    }

    #[test]
    fn reads_ls_tree_output() {
        let out = b"100644 blob aaa\ta b.txt\x00100755 blob bbb\tdir/run\x00\
                    160000 commit ccc\tsub\x00120000 blob ddd\tweird\nname\x00";
        let e = parse_ls_tree(out);
        assert_eq!(e.len(), 4);
        assert_eq!(e[0].path, "a b.txt");
        assert_eq!(e[0].mode, "100644");
        assert_eq!(e[1].mode, "100755");
        assert_eq!(e[2].kind, "commit");
        assert_eq!(e[3].path, "weird\nname");
        assert!(parse_ls_tree(b"").is_empty());
    }

    #[test]
    fn an_empty_remote_has_nothing() {
        assert_eq!(parse_ls_remote(""), Remote::default());
    }
}
