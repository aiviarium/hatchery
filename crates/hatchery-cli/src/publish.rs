//! `publish`, `withdraw` and `keygen`.

use std::env;
use std::path::Path;
use std::process::Command;

use hatchery::publish::withdraw_dir;
use hatchery::sign::{self, SigningKey};
use hatchery::{Egg, Name, PublishTo};

use crate::args::{Publish, Withdraw};
use crate::{Fail, Res};

pub fn run(a: Publish) -> Res {
    let mut eggs = a
        .eggs
        .iter()
        .map(|p| Egg::read(p).map_err(Fail::from))
        .collect::<Res<Vec<_>>>()?;
    // A version becomes visible when its base egg is published, so the
    // variant eggs go first. The sort keeps the order of the rest.
    eggs.sort_by_key(|e| e.variant().is_none());

    let key = match &a.sign_key_env {
        Some(var) => {
            let text = env::var(var)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .ok_or_else(|| format!("--sign-key-env {var}: the variable is not set"))?;
            Some(sign::parse_signing_key(&text).map_err(|e| format!("{var}: {e}"))?)
        }
        None => None,
    };

    let to = destination(&a)?;
    for egg in &eggs {
        let p = hatchery::publish(egg, &to, key.as_ref())?;
        println!(
            "{}  {}  {}",
            p.file,
            p.sha256,
            if p.signed { "signed" } else { "not signed" }
        );
    }
    Ok(())
}

fn destination(a: &Publish) -> Res<PublishTo> {
    if let Some(dir) = &a.to {
        return Ok(PublishTo::Dir(dir.clone()));
    }
    if let Some(url) = &a.git {
        return Ok(PublishTo::Git {
            url: url.clone(),
            branch: a.branch.clone().unwrap_or_else(|| "eggs".to_string()),
            path: a.path.clone(),
        });
    }
    let here = env::current_dir().map_err(|e| format!("the current folder: {e}"))?;
    let Some((root, repo)) = hatchery::manifest::find_repo_root(&here)? else {
        return Err("no packages.kdl above this folder: pass --to DIR or --git URL".into());
    };
    let Some(eggs) = repo.release.eggs else {
        return Err(format!(
            "{} has no release.eggs: pass --to DIR or --git URL",
            root.join(hatchery::repo::FILE).display()
        )
        .into());
    };
    Ok(PublishTo::Git {
        url: origin_url(&root)?,
        branch: eggs.branch,
        path: eggs.path,
    })
}

fn origin_url(root: &Path) -> Res<String> {
    let out = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || url.is_empty() {
        return Err(format!(
            "{} has no origin remote to publish to: pass --git URL",
            root.display()
        )
        .into());
    }
    Ok(url)
}

pub fn withdraw(a: Withdraw) -> Res {
    let (name, version) = a
        .name
        .split_once('@')
        .ok_or("expected group/id@version, such as aiviarium/claude@1.3.2")?;
    let name = Name::parse(name)?;
    let version = hatchery::name::parse_version(version)?;
    withdraw_dir(&a.to, &name, &version, a.reason.as_deref())?;
    println!("withdrew {name} {version}");
    Ok(())
}

pub fn keygen() -> Res {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| format!("no random numbers: {e}"))?;
    let key = SigningKey::from_bytes(&seed);
    println!("{}", sign::format_signing_key(&key));
    println!("{}", sign::format_verifying_key(&key.verifying_key()));
    eprintln!(
        "Line 1 is the signing key. Keep it secret, for example as a CI secret, and pass \
         its variable to `hatchery publish --sign-key-env`.\n\
         Line 2 is the public key. Put it in `key=` of the registry line in hatchery.kdl."
    );
    Ok(())
}
