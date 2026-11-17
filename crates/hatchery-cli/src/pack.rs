//! `hatchery pack`.

use std::fs;
use std::path::Path;

use hatchery::{Egg, PackOptions, VariantPick};

use crate::args::Pack;
use crate::{Fail, Res, io};

pub fn run(a: Pack) -> Res {
    let mut variants = Vec::new();
    for v in group_variants(&a.variant) {
        variants.push(parse_variant(&v)?);
    }
    let checks = a
        .check
        .iter()
        .map(|c| parse_check(c))
        .collect::<Res<Vec<_>>>()?;

    let opts = PackOptions {
        base: a.base,
        variants,
        fat: a.fat,
        no_build: a.no_build,
        ..PackOptions::default()
    };
    let eggs = hatchery::pack(&a.dir, &opts)?;

    if !checks.is_empty() {
        let base = eggs
            .iter()
            .find(|e| e.variant().is_none())
            .ok_or("--check reads the base egg: pack it as well, with --base")?;
        let mut failed = false;
        for (target, schema) in &checks {
            for line in check_target(base, target, schema)? {
                eprintln!("{line}");
                failed = true;
            }
        }
        if failed {
            return Err(Fail::from("the check failed; no egg was written"));
        }
    }

    io(fs::create_dir_all(&a.out), &a.out)?;
    for egg in &eggs {
        let path = a.out.join(egg.file_name());
        io(fs::write(&path, egg.bytes()), &path)?;
        println!("{}  {}", egg.file_name(), egg.sha256());
    }
    Ok(())
}

/// Clap gives the values of every `--variant` in one list. A target name
/// never holds `=` and a value always does, so a value without it starts the
/// next variant.
fn group_variants(vals: &[String]) -> Vec<Vec<String>> {
    let mut groups: Vec<Vec<String>> = Vec::new();
    for v in vals {
        match groups.last_mut() {
            Some(g) if v.contains('=') => g.push(v.clone()),
            _ => groups.push(vec![v.clone()]),
        }
    }
    groups
}

/// `--variant rookery arch=x86_64 os=linux`.
fn parse_variant(vals: &[String]) -> Res<VariantPick> {
    let (target, rest) = vals.split_first().ok_or("--variant needs a target")?;
    let mut pick = VariantPick {
        target: target.clone(),
        os: None,
        arch: None,
    };
    for v in rest {
        match v.split_once('=') {
            Some(("os", x)) if !x.is_empty() => pick.os = Some(x.to_string()),
            Some(("arch", x)) if !x.is_empty() => pick.arch = Some(x.to_string()),
            _ => {
                return Err(Fail::from(format!(
                    "--variant {target}: expected os=VALUE or arch=VALUE, got {v:?}. \
                     Put the folder before --variant."
                )));
            }
        }
    }
    Ok(pick)
}

fn parse_check(c: &str) -> Res<(String, String)> {
    match c.split_once('=') {
        Some((t, s)) if !t.is_empty() && !s.is_empty() => Ok((t.to_string(), s.to_string())),
        _ => Err(Fail::from(format!(
            "--check {c:?}: expected TARGET=SCHEMA, such as crow=schemas/crow.schema.json"
        ))),
    }
}

/// Validates a target's manifest file in the base egg. Returns one line
/// for each error.
fn check_target(egg: &Egg, target: &str, schema_path: &str) -> Res<Vec<String>> {
    let spec = egg
        .manifest()
        .target(target)
        .ok_or_else(|| format!("--check {target}: the package has no target {target}"))?;
    let file = spec
        .manifest
        .clone()
        .unwrap_or_else(|| format!("{target}.json"));
    let files = egg
        .target(target)
        .ok_or_else(|| format!("--check {target}: the egg has no files for {target}"))?;
    let Some(bytes) = files.get(&file) else {
        return Ok(vec![format!("{target}/{file}: no such file")]);
    };

    let schema_text = fs::read_to_string(Path::new(schema_path))
        .map_err(|e| Fail::from(format!("{schema_path}: {e}")))?;
    let schema: serde_json::Value = serde_json::from_str(&schema_text)
        .map_err(|e| Fail::from(format!("{schema_path}: not JSON: {e}")))?;
    let validator = jsonschema::validator_for(&schema)
        .map_err(|e| Fail::from(format!("{schema_path}: not a JSON Schema: {e}")))?;

    let instance: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(e) => return Ok(vec![format!("{target}/{file}: not JSON: {e}")]),
    };
    let mut lines = Vec::new();
    for err in validator.iter_errors(&instance) {
        let at = err.instance_path().to_string();
        let at = if at.is_empty() { "/".to_string() } else { at };
        lines.push(format!("{target}/{file}: {at}: {err}"));
    }
    Ok(lines)
}
