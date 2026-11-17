//! `hatchery registry`.

use std::fs;
use std::path::{Path, PathBuf};

use hatchery::{Config, RegistryConfig};

use crate::args::{RegistryAdd, RegistryCmd};
use crate::{Res, io, table};

/// The configuration to read: the one named, the one found, or an empty
/// one. A file that does not exist is an empty configuration when it came
/// from `HATCHERY_CONFIG` or when `create` is set, so that it can be
/// written.
pub fn load(path: Option<&Path>, create: bool) -> Res<(Config, Option<PathBuf>)> {
    let explicit = path.is_some();
    match path.map(Path::to_path_buf).or_else(Config::find) {
        Some(p) if !p.exists() && (create || !explicit) => Ok((Config::empty(), Some(p))),
        Some(p) => Ok((Config::load(&p)?, Some(p))),
        None => Ok((Config::empty(), None)),
    }
}

pub fn run(config: Option<&Path>, cmd: RegistryCmd) -> Res {
    let create = matches!(cmd, RegistryCmd::Add(_));
    let (mut cfg, found) = load(config, create)?;
    match cmd {
        RegistryCmd::List => {
            let rows: Vec<Vec<String>> = cfg
                .registries
                .iter()
                .map(|r| {
                    let mut extra = Vec::new();
                    if !r.only.is_empty() {
                        extra.push(format!("only={}", r.only.join(",")));
                    }
                    if r.key.is_some() {
                        extra.push("signed".to_string());
                    }
                    if let Some(a) = r.min_age {
                        extra.push(format!("min-age={}", hatchery::time::format_duration(a)));
                    }
                    if let Some(t) = &r.token_env {
                        extra.push(format!("token-env={t}"));
                    }
                    vec![r.kind.clone(), r.location.clone(), extra.join(" ")]
                })
                .collect();
            print!("{}", table(&rows));
            Ok(())
        }
        RegistryCmd::Add(a) => {
            let r = registry_from(a)?;
            let path = found.unwrap_or_else(|| PathBuf::from(hatchery::config::FILE));
            let (kind, location) = (r.kind.clone(), r.location.clone());
            cfg.add_registry(r);
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                io(fs::create_dir_all(dir), dir)?;
            }
            cfg.save(&path)?;
            println!("added {kind} {location} to {}", path.display());
            Ok(())
        }
        RegistryCmd::Remove { location } => {
            let Some(path) = found.filter(|p| p.exists()) else {
                return Err("there is no hatchery.kdl to remove it from".into());
            };
            if !cfg.remove_registry(&location) {
                return Err(format!("{}: no registry {location}", path.display()).into());
            }
            cfg.save(&path)?;
            println!("removed {location} from {}", path.display());
            Ok(())
        }
    }
}

fn registry_from(a: RegistryAdd) -> Res<RegistryConfig> {
    if !matches!(a.kind.as_str(), "git" | "http" | "dir") {
        return Err(format!("{:?} is not a registry kind: use git, http or dir", a.kind).into());
    }
    let mut r = RegistryConfig::new(a.kind, a.location);
    if let Some(o) = a.only {
        r.only = o
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
    }
    if let Some(k) = a.key {
        hatchery::sign::parse_verifying_key(&k)?;
        r.key = Some(k);
    }
    if let Some(m) = a.min_age {
        r.min_age = Some(hatchery::time::parse_duration(&m)?);
    }
    r.token_env = a.token_env;
    Ok(r)
}
