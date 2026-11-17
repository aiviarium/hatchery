//! `fetch`, `add`, `update`, `install`, `remove` and `list`.

use std::fs;
use std::path::Path;

use hatchery::client::Fetched;
use hatchery::fetch::HttpFetch;
use hatchery::{Client, Name, Options, Report, Spec, StaticTarget, Variants};
use serde_json::json;

use crate::args::{Add, Fetch, Install, List, Remove, Source, Tools, Update};
use crate::registry::load;
use crate::{Fail, Res, io, table};

fn client(config: Option<&Path>, tools: &Tools) -> Res<Client> {
    let (cfg, _) = load(config, false)?;
    let mut facts = Vec::new();
    for f in &tools.facts {
        match f.split_once('=') {
            Some((n, v)) if !n.is_empty() => facts.push((n, v)),
            _ => return Err(format!("--fact {f:?}: expected NAME=VALUE").into()),
        }
    }
    let mut b = Client::builder().config(cfg).fetch(HttpFetch::new());
    for name in &tools.targets {
        let mut t = StaticTarget::new(name);
        for (n, v) in &facts {
            t = t.fact(*n, *v);
        }
        if tools.on_demand {
            t = t.variants(Variants::OnDemand);
        }
        b = b.target(t);
    }
    Ok(b.build()?)
}

fn need_target(tools: &Tools) -> Res {
    if tools.targets.is_empty() {
        return Err("name at least one tool to install for, with --target NAME".into());
    }
    Ok(())
}

fn spec(s: &Source) -> Res<Spec> {
    match (&s.git, &s.spec) {
        (Some(url), _) => Ok(Spec::git(url, s.path.clone(), s.git_ref.clone())),
        (None, Some(text)) => Ok(Spec::parse(text)?),
        (None, None) => Err("name a package, or pass --git URL".into()),
    }
}

fn print_report(r: &Report) {
    if r.packages.is_empty() {
        println!("nothing to change in {}", r.dir.display());
    }
    for c in &r.packages {
        match &c.previous {
            Some(p) if *p != c.version => {
                println!("updated {} {p} -> {} ({})", c.name, c.version, c.registry)
            }
            Some(_) => println!("reinstalled {} {} ({})", c.name, c.version, c.registry),
            None => println!("installed {} {} ({})", c.name, c.version, c.registry),
        }
    }
    for w in &r.warnings {
        eprintln!("warning: {w}");
    }
}

pub fn fetch(config: Option<&Path>, a: Fetch) -> Res {
    let tools = Tools {
        targets: Vec::new(),
        facts: Vec::new(),
        on_demand: false,
    };
    let client = client(config, &tools)?;
    let variants: Option<Vec<String>> = if a.variant.is_empty() {
        None
    } else {
        for v in &a.variant {
            if !v.contains('.') {
                return Err(format!("--variant {v:?}: expected TARGET.KEY").into());
            }
        }
        Some(a.variant.clone())
    };
    let opts = Options {
        allow_young: a.allow_young,
    };
    let Fetched { base, variants } =
        hatchery::block_on(client.fetch(&spec(&a.source)?, variants.as_deref(), &opts))?;

    io(fs::create_dir_all(&a.out), &a.out)?;
    let eggs = if a.fat {
        vec![base.fatten(&variants)?]
    } else {
        let mut all = vec![base];
        all.extend(variants);
        all
    };
    for egg in &eggs {
        let path = a.out.join(egg.file_name());
        io(fs::write(&path, egg.bytes()), &path)?;
        println!("{}  {}", egg.file_name(), egg.sha256());
    }
    Ok(())
}

pub fn add(config: Option<&Path>, a: Add) -> Res {
    need_target(&a.tools)?;
    let client = client(config, &a.tools)?;
    let opts = Options {
        allow_young: a.allow_young,
    };
    let r = hatchery::block_on(client.add_with(&spec(&a.source)?, &a.dir, &opts))?;
    print_report(&r);
    Ok(())
}

pub fn update(config: Option<&Path>, a: Update) -> Res {
    need_target(&a.tools)?;
    let client = client(config, &a.tools)?;
    let name = a.name.as_deref().map(Name::parse).transpose()?;
    let r = hatchery::block_on(client.update(name.as_ref(), &a.dir))?;
    print_report(&r);
    Ok(())
}

pub fn install(config: Option<&Path>, a: Install) -> Res {
    need_target(&a.tools)?;
    let client = client(config, &a.tools)?;
    let r = hatchery::block_on(client.install(&a.dir))?;
    print_report(&r);
    Ok(())
}

pub fn remove(config: Option<&Path>, a: Remove) -> Res {
    let tools = Tools {
        targets: Vec::new(),
        facts: Vec::new(),
        on_demand: false,
    };
    let client = client(config, &tools)?;
    let name = Name::parse(&a.name)?;
    client.remove(&name, &a.dir)?;
    println!("removed {name}");
    Ok(())
}

fn source_of(p: &hatchery::LockedPackage) -> String {
    if let Some(git) = &p.git {
        let mut s = git.clone();
        if let Some(path) = &p.path {
            s.push(' ');
            s.push_str(path);
        }
        if let Some(r) = &p.git_ref {
            s.push('@');
            s.push_str(r);
        }
        s
    } else if let Some(r) = &p.registry {
        r.clone()
    } else if let Some(f) = &p.file {
        f.clone()
    } else if let Some(f) = &p.folder {
        f.clone()
    } else {
        String::new()
    }
}

pub fn list(config: Option<&Path>, a: List) -> Res {
    let client = client(config, &a.tools)?;
    if a.offered {
        return offered(&client, &a);
    }
    let installed = client.installed(&a.dir)?;
    if a.json {
        let items: Vec<_> = installed
            .iter()
            .map(|i| {
                json!({
                    "name": i.name.to_string(),
                    "version": i.version.to_string(),
                    "targets": i.targets,
                    "source": source_of(&i.lock),
                    "sha256": i.sha256,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&items).map_err(Fail::from)?
        );
        return Ok(());
    }
    let rows: Vec<Vec<String>> = installed
        .iter()
        .map(|i| {
            vec![
                i.name.to_string(),
                i.version.to_string(),
                i.targets.join(","),
                source_of(&i.lock),
            ]
        })
        .collect();
    print!("{}", table(&rows));
    Ok(())
}

fn offered(client: &Client, a: &List) -> Res {
    let list = hatchery::block_on(client.offered(&a.dir))?;
    if a.json {
        let items: Vec<_> = list
            .iter()
            .map(|o| {
                let versions: Vec<_> = o
                    .versions
                    .iter()
                    .map(|v| {
                        json!({
                            "version": v.version.to_string(),
                            "published": v.published.map(|t| t.to_string()),
                            "withdrawn": v.withdrawn.as_ref().map(|w| json!({ "reason": w.reason })),
                            "held-until": v.held_until.map(|t| t.to_string()),
                            "problem": v.problem,
                        })
                    })
                    .collect();
                json!({
                    "name": o.name.to_string(),
                    "about": { "name": o.about.name, "summary": o.about.summary },
                    "registry": o.registry,
                    "versions": versions,
                    "installed": o.installed.as_ref().map(|v| v.to_string()),
                    "updatable": o.updatable.as_ref().map(|v| v.to_string()),
                    "held-until": o.held_until.map(|t| t.to_string()),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&items).map_err(Fail::from)?
        );
        return Ok(());
    }
    let mut rows = vec![vec![
        "PACKAGE".to_string(),
        "NEWEST".to_string(),
        "INSTALLED".to_string(),
        "UPDATE".to_string(),
        "HELD UNTIL".to_string(),
        "REGISTRY".to_string(),
    ]];
    for o in &list {
        rows.push(vec![
            o.name.to_string(),
            o.versions
                .first()
                .map(|v| v.version.to_string())
                .unwrap_or_default(),
            o.installed
                .as_ref()
                .map(|v| v.to_string())
                .unwrap_or_default(),
            o.updatable
                .as_ref()
                .map(|v| v.to_string())
                .unwrap_or_default(),
            o.held_until.map(|t| t.to_string()).unwrap_or_default(),
            o.registry.clone(),
        ]);
    }
    print!("{}", table(&rows));
    Ok(())
}
