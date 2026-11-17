//! `hatchery verify`.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use hatchery::fetch::{Fetch, Url};
use hatchery::manifest::variant_key;
use hatchery::sign;
use hatchery::{Egg, Timestamp};
use serde_json::Value;

use crate::args::Verify;
use crate::{Fail, Res};

pub fn run(a: Verify) -> Res {
    let egg = Egg::read(&a.egg)?;
    let m = egg.manifest();
    println!("{}", egg.file_name());
    println!("sha256  {}", egg.sha256());
    println!("package {} {}", m.name(), m.package.version);

    let Some(registry) = &a.registry else {
        println!("opened: every entry is safe to unpack");
        return Ok(());
    };
    let key = a
        .key
        .as_deref()
        .map(sign::parse_verifying_key)
        .transpose()?;

    // A fat egg is checked as the base egg it was made from, which is what
    // the index lists.
    let listed = if egg.variant().is_some() {
        egg.clone()
    } else {
        egg.base()?
    };
    let index = read_index(registry, &m.package.group, &m.package.id)?;
    let version = m.package.version.to_string();
    let entry = index
        .get("versions")
        .and_then(|v| v.get(&version))
        .filter(|e| e.get("sha256").is_some() || egg.variant().is_some())
        .ok_or_else(|| format!("{} {version} is not in the index of {registry}", m.name()))?;

    // The entry the egg's own checksum and signature are listed in.
    let (sha, signature, published) = match egg.variant() {
        None => (
            text(entry, "sha256"),
            text(entry, "signature"),
            text(entry, "published"),
        ),
        Some(h) => {
            let wanted = h.key();
            let v = entry
                .get("targets")
                .and_then(|t| t.get(&h.target))
                .and_then(|t| t.get("variants"))
                .and_then(Value::as_array)
                .and_then(|vs| {
                    vs.iter().find(|v| {
                        variant_key(
                            v.get("os").and_then(Value::as_str),
                            v.get("arch").and_then(Value::as_str),
                        ) == wanted
                    })
                })
                .ok_or_else(|| {
                    format!(
                        "the index lists no variant {}.{wanted} of {version}",
                        h.target
                    )
                })?;
            (
                text(v, "sha256"),
                text(v, "signature"),
                text(v, "published").or_else(|| text(entry, "published")),
            )
        }
    };
    let sha = sha.ok_or("the index has no checksum for this egg: it is not published yet")?;
    if !sha.eq_ignore_ascii_case(listed.sha256()) {
        return Err(Fail::from(format!(
            "checksum mismatch: the egg is {}, the index says {sha}",
            listed.sha256()
        )));
    }
    println!("the checksum matches the index");

    if let Some(key) = key {
        let signature = signature.ok_or("the index has no signature for this egg")?;
        let published = published.ok_or("the index has no publish time for this egg")?;
        let published = Timestamp::parse(&published)?;
        let msg = sign::message(
            &m.name(),
            &m.package.version,
            &listed.file_name(),
            listed.sha256(),
            &published,
        );
        sign::verify(&key, &msg, &signature)
            .map_err(|why| format!("signature not valid: {why}"))?;
        println!("the signature is valid for the key");
    }
    Ok(())
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

/// `<registry>/<group>/<id>/index.json`, from a folder or over HTTP.
fn read_index(registry: &str, group: &str, id: &str) -> Res<Value> {
    let rel = format!("{group}/{id}/index.json");
    let bytes = if registry.starts_with("http://") || registry.starts_with("https://") {
        let base = format!("{}/", registry.trim_end_matches('/'));
        let url = Url::parse(&base)
            .and_then(|b| b.join(&rel))
            .map_err(|e| format!("{registry}: {e}"))?;
        let fetch: Arc<dyn Fetch> = Arc::new(hatchery::fetch::HttpFetch::new());
        let got = hatchery::block_on(fetch.get(&url, None))?;
        got.map(|b| b.to_vec())
            .ok_or_else(|| format!("{url}: not found"))?
    } else {
        let path = Path::new(registry).join(&rel);
        fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?
    };
    Ok(serde_json::from_slice(&bytes)?)
}
