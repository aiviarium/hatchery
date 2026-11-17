//! `hatchery pack`: from a package folder to eggs.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use bytes::Bytes;

use crate::egg::{Egg, Entry, VARIANT_FILE, VARIANTS_DIR, VariantHeader};
use crate::error::{Error, IoContext, Result};
use crate::glob::Glob;
use crate::manifest::{self, Defaults, Manifest, Origin, TargetSpec, VariantSpec};
use crate::target::Target;
use crate::time::Timestamp;

/// One variant to pack: a target and the os/arch values that pick it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantPick {
    pub target: String,
    pub os: Option<String>,
    pub arch: Option<String>,
}

/// What `pack` writes. With nothing chosen, it writes the base egg and
/// every variant egg.
#[derive(Default)]
pub struct PackOptions<'a> {
    /// The base egg.
    pub base: bool,
    /// These variant eggs.
    pub variants: Vec<VariantPick>,
    /// One fat egg with every variant (or those in `variants`) inside,
    /// instead of separate eggs.
    pub fat: bool,
    /// Skip the build commands and pack what is there.
    pub no_build: bool,
    /// Tools whose `check` runs on their target's files.
    pub checks: Vec<&'a dyn Target>,
    /// Defaults to apply instead of looking for a `packages.kdl` above the
    /// folder.
    pub defaults: Option<Defaults>,
    /// The origin section to write instead of reading it from git.
    pub origin: Option<Origin>,
    /// The build time when neither `origin` nor `SOURCE_DATE_EPOCH` nor the
    /// git commit gives one.
    pub now: Option<Timestamp>,
}

/// Packs a package folder. Runs build commands first unless `no_build`.
/// Returns the eggs in the order: variant eggs, then the base (or fat) egg.
///
/// Targets' own checks run on the shared files whenever a base or fat egg
/// is written.
pub fn pack(dir: &Path, opts: &PackOptions<'_>) -> Result<Vec<Egg>> {
    let manifest = load_manifest(dir, opts)?;

    let nothing_chosen = !opts.base && opts.variants.is_empty() && !opts.fat;
    let want_base = opts.base || opts.fat || nothing_chosen;
    let picked = pick_variants(&manifest, &opts.variants, nothing_chosen || opts.fat)?;

    if !opts.no_build {
        run_builds(dir, &manifest, want_base, &picked)?;
    }

    let mut eggs = Vec::new();
    let mut variant_eggs = Vec::new();
    for (target, variant) in &picked {
        let egg = variant_egg(dir, &manifest, target, variant)?;
        variant_eggs.push(egg);
    }
    if !opts.fat {
        eggs.append(&mut variant_eggs.clone());
    }

    if want_base {
        let flat = flatten(&manifest, dir, opts);
        let base = base_egg(dir, &manifest, &flat)?;
        for t in &opts.checks {
            if let Some(files) = base.target(t.name()) {
                t.check(&files).map_err(|problems| Error::Check {
                    target: t.name().to_string(),
                    problems,
                })?;
            }
        }
        eggs.push(if opts.fat {
            base.fatten(&variant_eggs)?
        } else {
            base
        });
    }
    Ok(eggs)
}

fn load_manifest(dir: &Path, opts: &PackOptions<'_>) -> Result<Manifest> {
    let file = dir.join(manifest::FILE);
    if !file.is_file() {
        for t in &opts.checks {
            if let Some(m) = t.derive(dir)? {
                return Ok(m);
            }
        }
        return Err(Error::invalid(
            dir.display(),
            format!("has no {} and no target could derive one", manifest::FILE),
        ));
    }
    match &opts.defaults {
        Some(d) => {
            let text = fs::read_to_string(&file).at_path(&file)?;
            Manifest::parse_with(&text, Some(d))
        }
        None => Manifest::load(dir),
    }
}

/// The variants to write, as target and variant, in manifest order.
fn pick_variants<'m>(
    manifest: &'m Manifest,
    picks: &[VariantPick],
    all: bool,
) -> Result<Vec<(&'m TargetSpec, &'m VariantSpec)>> {
    let mut chosen = Vec::new();
    if picks.is_empty() {
        if all {
            for t in &manifest.targets {
                chosen.extend(t.variants.iter().map(|v| (t, v)));
            }
        }
        return Ok(chosen);
    }
    for p in picks {
        let t = manifest.target(&p.target).ok_or_else(|| {
            Error::invalid("pack", format!("the package has no target {}", p.target))
        })?;
        let v = t
            .variants
            .iter()
            .find(|v| v.os == p.os && v.arch == p.arch)
            .ok_or_else(|| {
                let have: Vec<String> = t.variants.iter().map(VariantSpec::key).collect();
                Error::invalid(
                    "pack",
                    format!(
                        "target {} has no variant {}; it has {}",
                        t.name,
                        manifest::variant_key(p.os.as_deref(), p.arch.as_deref()),
                        if have.is_empty() {
                            "none".to_string()
                        } else {
                            have.join(", ")
                        }
                    ),
                )
            })?;
        if !chosen.iter().any(|(ct, cv): &(&TargetSpec, &VariantSpec)| {
            std::ptr::eq(*ct, t) && std::ptr::eq(*cv, v)
        }) {
            chosen.push((t, v));
        }
    }
    // Manifest order, whatever order they were asked for in.
    let order = |t: &TargetSpec, v: &VariantSpec| {
        let ti = manifest.targets.iter().position(|x| std::ptr::eq(x, t));
        let vi = t.variants.iter().position(|x| std::ptr::eq(x, v));
        (ti, vi)
    };
    chosen.sort_by_key(|(t, v)| order(t, v));
    Ok(chosen)
}

fn run_builds(
    dir: &Path,
    manifest: &Manifest,
    want_base: bool,
    picked: &[(&TargetSpec, &VariantSpec)],
) -> Result<()> {
    for t in &manifest.targets {
        if want_base && let Some(cmd) = &t.build {
            run_build(dir, manifest, t, None, cmd)?;
        }
        for (pt, v) in picked {
            if std::ptr::eq(*pt, t)
                && let Some(cmd) = &v.build
            {
                run_build(dir, manifest, t, Some(v), cmd)?;
            }
        }
    }
    Ok(())
}

fn run_build(
    dir: &Path,
    manifest: &Manifest,
    target: &TargetSpec,
    variant: Option<&VariantSpec>,
    command: &str,
) -> Result<()> {
    let (shell, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    let mut cmd = Command::new(shell);
    cmd.arg(flag)
        .arg(command)
        .current_dir(dir)
        .env("EGG_ID", &manifest.package.id)
        .env("EGG_VERSION", manifest.package.version.to_string())
        .env("EGG_TARGET", &target.name);
    if let Some(v) = variant {
        cmd.env("EGG_OS", v.os.as_deref().unwrap_or(""))
            .env("EGG_ARCH", v.arch.as_deref().unwrap_or(""));
    }
    let fail = |message: String| Error::Build {
        command: command.to_string(),
        message,
    };
    let out = cmd
        .output()
        .map_err(|e| fail(format!("cannot run {shell}: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    let mut detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if detail.is_empty() {
        detail = String::from_utf8_lossy(&out.stdout).trim().to_string();
    }
    const TAIL: usize = 2000;
    if detail.chars().count() > TAIL {
        let skip = detail.chars().count() - TAIL;
        detail = detail.chars().skip(skip).collect();
    }
    let status = out.status.to_string();
    Err(fail(if detail.is_empty() {
        status
    } else {
        format!("{status}: {detail}")
    }))
}

// Reading files.

struct DiskFile {
    rel: String,
    path: PathBuf,
}

/// Every file under `root` in sorted order, refusing symlinks. `skip` says
/// which folders to leave out, by path relative to `root`.
fn walk(root: &Path, skip: &dyn Fn(&str) -> bool) -> Result<Vec<DiskFile>> {
    let mut out = Vec::new();
    walk_into(root, "", skip, &mut out)?;
    Ok(out)
}

fn walk_into(
    dir: &Path,
    prefix: &str,
    skip: &dyn Fn(&str) -> bool,
    out: &mut Vec<DiskFile>,
) -> Result<()> {
    let mut names: Vec<String> = Vec::new();
    for e in fs::read_dir(dir).at_path(dir)? {
        let e = e.at_path(dir)?;
        names.push(e.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    for name in names {
        let path = dir.join(&name);
        let rel = format!("{prefix}{name}");
        let meta = fs::symlink_metadata(&path).at_path(&path)?;
        if meta.file_type().is_symlink() {
            return Err(Error::invalid(
                path.display(),
                "is a symlink; packages cannot hold symlinks",
            ));
        }
        if meta.is_dir() {
            if !skip(&rel) {
                walk_into(&path, &format!("{rel}/"), skip, out)?;
            }
        } else if meta.is_file() {
            out.push(DiskFile { rel, path });
        }
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable(path: &Path) -> Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    Ok(fs::metadata(path).at_path(path)?.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> Result<bool> {
    Ok(false)
}

fn read_entry(at: String, file: &Path) -> Result<Entry> {
    Ok(Entry {
        path: at,
        data: Bytes::from(fs::read(file).at_path(file)?),
        executable: is_executable(file)?,
    })
}

fn normal(p: &str) -> &str {
    p.strip_prefix("./").unwrap_or(p).trim_end_matches('/')
}

/// The shared files of a target, as entries at `<target>/<rel>`.
fn shared_entries(dir: &Path, t: &TargetSpec) -> Result<Vec<Entry>> {
    let root = dir.join(t.dir());
    if !root.is_dir() {
        return Err(Error::invalid(
            format!("target {}", t.name),
            format!("its folder {} does not exist", root.display()),
        ));
    }
    let include: Vec<Glob> = t.include.iter().map(|p| Glob::new(p)).collect();
    let exclude: Vec<Glob> = t.exclude.iter().map(|p| Glob::new(p)).collect();
    let variant_dirs: Vec<String> = t
        .variants
        .iter()
        .map(|v| normal(&t.variant_dir(v)).to_string())
        .collect();
    let target_dir = normal(t.dir()).to_string();
    let skip =
        |rel: &str| rel == VARIANTS_DIR || variant_dirs.contains(&format!("{target_dir}/{rel}"));
    let mut out = Vec::new();
    for f in walk(&root, &skip)? {
        let wanted = include.is_empty() || include.iter().any(|g| g.covers(&f.rel));
        if wanted && !exclude.iter().any(|g| g.covers(&f.rel)) {
            out.push(read_entry(format!("{}/{}", t.name, f.rel), &f.path)?);
        }
    }
    Ok(out)
}

fn variant_egg(dir: &Path, manifest: &Manifest, t: &TargetSpec, v: &VariantSpec) -> Result<Egg> {
    let header = VariantHeader {
        name: manifest.name(),
        version: manifest.version().clone(),
        target: t.name.clone(),
        os: v.os.clone(),
        arch: v.arch.clone(),
    };
    let rel_dir = t.variant_dir(v);
    let root = dir.join(&rel_dir);
    if !root.is_dir() {
        return Err(Error::invalid(
            format!("target {} variant {}", t.name, v.key()),
            format!(
                "folder {} must exist, or the variant needs a build that creates it",
                root.display()
            ),
        ));
    }
    let mut entries = vec![Entry {
        path: VARIANT_FILE.to_string(),
        data: Bytes::from(header.to_kdl()),
        executable: false,
    }];
    for f in walk(&root, &|_| false)? {
        entries.push(read_entry(format!("{}/{}", header.dir(), f.rel), &f.path)?);
    }
    Egg::from_entries(entries)
}

fn base_egg(dir: &Path, source: &Manifest, flat: &Manifest) -> Result<Egg> {
    let mut by_path: BTreeMap<String, Entry> = BTreeMap::new();
    for t in &source.targets {
        for e in shared_entries(dir, t)? {
            by_path.insert(e.path.clone(), e);
        }
    }
    let about = &source.about;
    for p in [&about.icon, &about.readme, &about.changelog]
        .into_iter()
        .flatten()
    {
        let p = normal(p);
        let first = p.split('/').next().unwrap_or("");
        if first == VARIANTS_DIR || p == manifest::FILE || p == VARIANT_FILE {
            return Err(Error::invalid(
                "about",
                format!("{p:?} cannot be a file of the egg"),
            ));
        }
        let file = dir.join(p);
        let meta = fs::symlink_metadata(&file)
            .map_err(|_| Error::invalid("about", format!("{p:?} does not exist in the package")))?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err(Error::invalid(
                "about",
                format!("{p:?} must be a regular file"),
            ));
        }
        if !by_path.contains_key(p) {
            by_path.insert(p.to_string(), read_entry(p.to_string(), &file)?);
        }
    }
    by_path.insert(
        manifest::FILE.to_string(),
        Entry {
            path: manifest::FILE.to_string(),
            data: Bytes::from(flat.to_kdl()),
            executable: false,
        },
    );
    Egg::from_entries(by_path.into_values().collect())
}

// Origin.

/// The manifest as written into the egg: targets and variants have no
/// `path`, and the origin section is filled in.
fn flatten(manifest: &Manifest, dir: &Path, opts: &PackOptions<'_>) -> Manifest {
    let mut m = manifest.clone();
    for t in &mut m.targets {
        t.path = None;
        for v in &mut t.variants {
            v.path = None;
        }
    }
    m.origin = Some(match &opts.origin {
        Some(o) => o.clone(),
        None => origin_of(dir, manifest, opts.now),
    });
    m
}

fn origin_of(dir: &Path, manifest: &Manifest, now: Option<Timestamp>) -> Origin {
    let commit = git(dir, &["rev-parse", "HEAD"]);
    let repository = manifest
        .about
        .repository
        .clone()
        .or_else(|| git(dir, &["remote", "get-url", "origin"]).map(|u| strip_userinfo(&u)));
    let from_env = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .map(Timestamp::from_secs);
    let built = from_env
        .or_else(|| {
            commit.as_ref()?;
            git(dir, &["log", "-1", "--format=%ct"])?
                .parse::<i64>()
                .ok()
                .map(Timestamp::from_secs)
        })
        .or(now)
        .unwrap_or_else(Timestamp::now);
    Origin {
        repository,
        commit,
        built: Some(built),
        hatchery: Some(crate::VERSION.to_string()),
        extra: Vec::new(),
    }
}

/// The trimmed output of a git command, None when git is missing or the
/// command fails.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Removes a user name and password from a remote address, so that
/// credentials never end up in an egg.
fn strip_userinfo(remote: &str) -> String {
    if remote.contains("://") {
        if let Ok(mut u) = url::Url::parse(remote)
            && (!u.username().is_empty() || u.password().is_some())
        {
            let _ = u.set_username("");
            let _ = u.set_password(None);
            return u.to_string();
        }
        return remote.to_string();
    }
    // `user@host:path`
    match (remote.find('@'), remote.find(':')) {
        (Some(at), Some(colon)) if at < colon => remote[at + 1..].to_string(),
        _ => remote.to_string(),
    }
}
