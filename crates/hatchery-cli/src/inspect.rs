//! `hatchery inspect`.

use hatchery::manifest::TargetSpec;
use hatchery::{Egg, Requirement};
use serde_json::{Value, json};

use crate::Res;
use crate::args::Inspect;

pub fn run(a: Inspect) -> Res {
    let egg = Egg::read(&a.egg)?;
    if a.json {
        println!("{}", serde_json::to_string_pretty(&to_json(&egg))?);
    } else {
        print!("{}", to_text(&egg));
    }
    Ok(())
}

fn kind(egg: &Egg) -> &'static str {
    if egg.variant().is_some() {
        "variant"
    } else if egg.is_fat() {
        "fat"
    } else {
        "base"
    }
}

fn requirement(r: &Requirement) -> String {
    if r.values.is_empty() {
        r.fact.clone()
    } else {
        format!("{} {}", r.fact, r.values.join(" "))
    }
}

fn deps(group: &str, list: &[hatchery::manifest::Dependency]) -> Vec<String> {
    list.iter()
        .map(|d| format!("{} {}", d.name(group), d.raw))
        .collect()
}

/// The variants of a target as `key`, and whether each is inside the egg.
fn variants(egg: &Egg, t: &TargetSpec) -> Vec<(String, bool)> {
    let inside = egg.embedded_variants();
    t.variants
        .iter()
        .map(|v| {
            let key = v.key();
            let here = inside.iter().any(|(tn, k)| *tn == t.name && *k == key);
            (key, here)
        })
        .collect()
}

fn to_text(egg: &Egg) -> String {
    let m = egg.manifest();
    let group = &m.package.group;
    let mut s = format!("{} {} ({} egg)\n", m.name(), m.package.version, kind(egg));
    let mut line = |k: &str, v: &str| s.push_str(&format!("  {k:<12}{v}\n"));
    if let Some(h) = egg.variant() {
        line("variant", &format!("{} {}", h.target, h.key()));
    }
    let a = &m.about;
    if let Some(v) = &a.name {
        line("name", v);
    }
    if let Some(v) = &a.summary {
        line("about", v);
    }
    if !a.licenses.is_empty() {
        line("license", &a.licenses.join(", "));
    }
    if !a.authors.is_empty() {
        line("authors", &a.authors.join(", "));
    }
    if let Some(v) = &a.homepage {
        line("homepage", v);
    }
    if let Some(v) = &a.repository {
        line("repository", v);
    }
    line("sha256", egg.sha256());
    if let Some(o) = &m.origin {
        let mut parts = Vec::new();
        if let Some(v) = &o.repository {
            parts.push(format!("repository {v}"));
        }
        if let Some(v) = &o.commit {
            parts.push(format!("commit {v}"));
        }
        if let Some(v) = &o.built {
            parts.push(format!("built {v}"));
        }
        if let Some(v) = &o.hatchery {
            parts.push(format!("hatchery {v}"));
        }
        line("origin", &parts.join(", "));
    }
    for t in &m.targets {
        s.push_str(&format!("  target {}\n", t.name));
        let mut item = |k: &str, v: &str| s.push_str(&format!("    {k:<14}{v}\n"));
        if !t.requires.is_empty() {
            let r: Vec<String> = t.requires.iter().map(requirement).collect();
            item("requires", &r.join("; "));
        }
        if !t.dependencies.is_empty() {
            item("dependencies", &deps(group, &t.dependencies).join(", "));
        }
        if !t.optional_dependencies.is_empty() {
            item(
                "optional",
                &deps(group, &t.optional_dependencies).join(", "),
            );
        }
        if let Some(b) = &t.build {
            item("build", b);
        }
        let vs = variants(egg, t);
        if !vs.is_empty() {
            let list: Vec<String> = vs
                .iter()
                .map(|(k, here)| {
                    if egg.is_fat() && *here {
                        format!("{k} (inside)")
                    } else {
                        k.clone()
                    }
                })
                .collect();
            item("variants", &list.join(", "));
        }
    }
    s
}

fn to_json(egg: &Egg) -> Value {
    let m = egg.manifest();
    let group = &m.package.group;
    let targets: Vec<Value> = m
        .targets
        .iter()
        .map(|t| {
            let requires: serde_json::Map<String, Value> = t
                .requires
                .iter()
                .map(|r| (r.fact.clone(), json!(r.values)))
                .collect();
            let vs: Vec<Value> = variants(egg, t)
                .into_iter()
                .map(|(key, inside)| json!({ "key": key, "inside": egg.is_fat() && inside }))
                .collect();
            json!({
                "name": t.name,
                "build": t.build.is_some(),
                "requires": requires,
                "dependencies": deps(group, &t.dependencies),
                "optional-dependencies": deps(group, &t.optional_dependencies),
                "variants": vs,
            })
        })
        .collect();
    let a = &m.about;
    let origin = m.origin.as_ref().map(|o| {
        json!({
            "repository": o.repository,
            "commit": o.commit,
            "built": o.built.map(|t| t.to_string()),
            "hatchery": o.hatchery,
        })
    });
    json!({
        "kind": kind(egg),
        "name": m.name().to_string(),
        "version": m.package.version.to_string(),
        "file": egg.file_name(),
        "sha256": egg.sha256(),
        "variant": egg.variant().map(|h| json!({ "target": h.target, "key": h.key() })),
        "about": {
            "name": a.name, "summary": a.summary, "licenses": a.licenses,
            "authors": a.authors, "homepage": a.homepage,
            "repository": a.repository, "keywords": a.keywords,
        },
        "targets": targets,
        "origin": origin,
    })
}
