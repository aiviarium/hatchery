//! `hatchery.kdl`: where eggs are installed from.
//!
//! Tools edit the file through [`Config::add_registry`] and
//! [`Config::remove_registry`], which keep its comments and layout.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use kdl::{KdlDocument, KdlEntry, KdlNode};

use crate::error::{Error, IoContext, Result};
use crate::kdlx;
use crate::time::{format_duration, parse_duration};

pub const FILE: &str = "hatchery.kdl";
/// The variable that points at a different configuration file.
pub const ENV: &str = "HATCHERY_CONFIG";
/// How long a registry read over the network holds a new version when
/// neither the registry nor the `default` line says.
pub const NETWORK_MIN_AGE: Duration = Duration::from_secs(3 * 86_400);

#[derive(Debug, Clone)]
pub struct Config {
    pub default_min_age: Option<Duration>,
    /// In search order.
    pub registries: Vec<RegistryConfig>,
    doc: KdlDocument,
}

/// One line of the `registries` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryConfig {
    /// The node name: `git`, `http`, `dir`, or a kind a tool adds.
    pub kind: String,
    /// The address: a URL or a path.
    pub location: String,
    /// The groups this registry may serve; any when empty.
    pub only: Vec<String>,
    /// An Ed25519 public key, `ed25519:<hex>`. Eggs must then be signed.
    pub key: Option<String>,
    pub min_age: Option<Duration>,
    /// The environment variable that carries a token.
    pub token_env: Option<String>,
    /// Every other property, for kinds a tool adds.
    pub props: BTreeMap<String, String>,
}

impl RegistryConfig {
    pub fn new(kind: impl Into<String>, location: impl Into<String>) -> RegistryConfig {
        RegistryConfig {
            kind: kind.into(),
            location: location.into(),
            only: Vec::new(),
            key: None,
            min_age: None,
            token_env: None,
            props: BTreeMap::new(),
        }
    }

    /// Whether the registry is on this machine: a `dir`, or a `git`
    /// registry given as a path or `file://` URL.
    pub fn is_local(&self) -> bool {
        match self.kind.as_str() {
            "dir" => true,
            "git" => is_local_location(&self.location),
            _ => false,
        }
    }

    /// Whether the registry may serve a group.
    pub fn serves(&self, group: &str) -> bool {
        self.only.is_empty() || self.only.iter().any(|g| g == group)
    }

    fn to_node(&self) -> KdlNode {
        let mut n = kdlx::node(&self.kind, &[&self.location]);
        if !self.only.is_empty() {
            n = kdlx::with_prop(n, "only", self.only.join(","));
        }
        if let Some(k) = &self.key {
            n = kdlx::with_prop(n, "key", k.as_str());
        }
        if let Some(m) = self.min_age {
            n = kdlx::with_prop(n, "min-age", format_duration(m));
        }
        if let Some(t) = &self.token_env {
            n = kdlx::with_prop(n, "token-env", t.as_str());
        }
        for (k, v) in &self.props {
            n.entries_mut()
                .push(KdlEntry::new_prop(k.as_str(), v.as_str()));
        }
        n
    }
}

/// A path or `file://` URL rather than a network address.
pub fn is_local_location(loc: &str) -> bool {
    if loc.starts_with("file://") {
        return true;
    }
    if loc.contains("://") {
        return false;
    }
    // scp-like `host:path`, but not a Windows drive letter.
    match loc.find(':') {
        Some(1) => true,
        Some(i) => loc[..i].contains('/'),
        None => true,
    }
}

fn parse_registry(n: &KdlNode, what: &str) -> Result<RegistryConfig> {
    let mut r = RegistryConfig::new(kdlx::name(n), kdlx::one_string(n, what)?);
    for (k, v) in kdlx::props(n) {
        let v =
            kdlx::text(v).ok_or_else(|| Error::invalid(what, format!("{k}= must be a string")))?;
        match k {
            "only" => {
                r.only = v
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            "key" => {
                crate::sign::parse_verifying_key(&v)?;
                r.key = Some(v);
            }
            "min-age" => r.min_age = Some(parse_duration(&v)?),
            "token-env" => r.token_env = Some(v),
            _ => {
                r.props.insert(k.to_string(), v);
            }
        }
    }
    Ok(r)
}

impl Config {
    /// An empty configuration, with no registries.
    pub fn empty() -> Config {
        Config::parse("").expect("an empty document parses")
    }

    pub fn parse(text: &str) -> Result<Config> {
        let what = FILE;
        let doc = kdlx::parse(text, what)?;
        let mut c = Config {
            default_min_age: None,
            registries: Vec::new(),
            doc: KdlDocument::new(),
        };
        for n in doc.nodes() {
            match kdlx::name(n) {
                "default" => {
                    kdlx::only_props(n, &["min-age"], what)?;
                    if let Some(m) = kdlx::prop_string(n, "min-age", what)? {
                        c.default_min_age = Some(parse_duration(&m)?);
                    }
                }
                "registries" => {
                    for r in kdlx::children(n) {
                        c.registries.push(parse_registry(r, what)?);
                    }
                }
                _ => {}
            }
        }
        c.doc = doc;
        Ok(c)
    }

    pub fn load(path: &Path) -> Result<Config> {
        let text = fs::read_to_string(path).at_path(path)?;
        Config::parse(&text).map_err(|e| match e {
            Error::Invalid { message, .. } => Error::invalid(path.display(), message),
            e => e,
        })
    }

    /// The file the `hatchery` command reads: `HATCHERY_CONFIG`, else
    /// `hatchery.kdl` in the current folder, else the one in the user's
    /// configuration folder. None when there is none.
    pub fn find() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os(ENV) {
            return Some(PathBuf::from(p));
        }
        let here = PathBuf::from(FILE);
        if here.is_file() {
            return Some(here);
        }
        let user = user_config_dir()?.join("hatchery").join(FILE);
        user.is_file().then_some(user)
    }

    /// How long new versions from a registry are held.
    pub fn min_age_for(&self, r: &RegistryConfig) -> Duration {
        r.min_age
            .or(self.default_min_age)
            .unwrap_or(if r.is_local() {
                Duration::ZERO
            } else {
                NETWORK_MIN_AGE
            })
    }

    /// Adds a registry at the end of the search order, or replaces the line
    /// with the same location.
    pub fn add_registry(&mut self, r: RegistryConfig) {
        let node = r.to_node();
        let regs = match self
            .doc
            .nodes()
            .iter()
            .position(|n| kdlx::name(n) == "registries")
        {
            Some(i) => &mut self.doc.nodes_mut()[i],
            None => {
                self.doc.nodes_mut().push(KdlNode::new("registries"));
                let last = self.doc.nodes_mut().last_mut().expect("just pushed");
                last.autoformat();
                last
            }
        };
        let had_children = regs.children().is_some_and(|c| !c.nodes().is_empty());
        let children = regs.ensure_children();
        let existing = children.nodes().iter().position(|n| {
            kdlx::args(n).next().and_then(kdlx::text).as_deref() == Some(r.location.as_str())
        });
        match existing {
            Some(i) => {
                let old = children.nodes()[i].format().cloned();
                let mut node = node;
                node.autoformat();
                if let (Some(f), Some(nf)) = (old, node.format_mut()) {
                    nf.leading = f.leading;
                    nf.trailing = f.trailing;
                }
                children.nodes_mut()[i] = node;
            }
            None => {
                let mut node = node;
                node.autoformat();
                // Indent like the lines already there.
                let leading = children
                    .nodes()
                    .last()
                    .and_then(|n| n.format().map(|f| f.leading.clone()))
                    .map(|l| l.rsplit('\n').next().unwrap_or("").to_string())
                    .unwrap_or_else(|| "    ".to_string());
                if let Some(f) = node.format_mut() {
                    f.leading = leading;
                    f.trailing = "\n".to_string();
                }
                if !had_children && let Some(f) = children.format_mut() {
                    f.leading = "\n".to_string();
                }
                children.nodes_mut().push(node);
                if !had_children && children.format().is_none() {
                    children.autoformat();
                }
            }
        }
        self.registries.retain(|o| o.location != r.location);
        // Keep the order of the file.
        let order: Vec<String> = self
            .doc
            .nodes()
            .iter()
            .filter(|n| kdlx::name(n) == "registries")
            .flat_map(|n| kdlx::children(n).iter())
            .filter_map(|n| kdlx::args(n).next().and_then(kdlx::text))
            .collect();
        self.registries.push(r);
        self.registries.sort_by_key(|r| {
            order
                .iter()
                .position(|l| *l == r.location)
                .unwrap_or(usize::MAX)
        });
    }

    /// Removes the registry with this location. Returns whether there was one.
    pub fn remove_registry(&mut self, location: &str) -> bool {
        let before = self.registries.len();
        self.registries.retain(|r| r.location != location);
        for n in self.doc.nodes_mut() {
            if kdlx::name(n) == "registries"
                && let Some(c) = n.children_mut()
            {
                c.nodes_mut().retain(|r| {
                    kdlx::args(r).next().and_then(kdlx::text).as_deref() != Some(location)
                });
            }
        }
        self.registries.len() != before
    }

    /// The file's text, with its comments and layout.
    pub fn to_kdl(&self) -> String {
        self.doc.to_string()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        crate::fsx::write_atomic(path, self.to_kdl().as_bytes())
    }
}

fn user_config_dir() -> Option<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").filter(|x| !x.is_empty()) {
        return Some(PathBuf::from(x));
    }
    if cfg!(windows) {
        return std::env::var_os("APPDATA").map(PathBuf::from);
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"// hatchery.kdl
default min-age="3d"

registries {
  // our own packages
  git "https://example.org/parrot.git" only="aiviarium" min-age="0d"
  http "https://eggs.example.org" key="ed25519:0000000000000000000000000000000000000000000000000000000000000000" min-age="7d" token-env="EGGS_EXAMPLE_TOKEN"
  dir "/srv/eggs"
}
"#;

    #[test]
    fn parses_registries() {
        let c = Config::parse(CONFIG).unwrap();
        assert_eq!(c.default_min_age, Some(Duration::from_secs(3 * 86_400)));
        assert_eq!(c.registries.len(), 3);
        let git = &c.registries[0];
        assert_eq!(git.kind, "git");
        assert_eq!(git.only, ["aiviarium"]);
        assert!(git.serves("aiviarium") && !git.serves("other"));
        assert_eq!(c.min_age_for(git), Duration::ZERO);
        assert_eq!(
            c.registries[1].token_env.as_deref(),
            Some("EGGS_EXAMPLE_TOKEN")
        );
        assert_eq!(
            c.min_age_for(&c.registries[1]),
            Duration::from_secs(7 * 86_400)
        );
        assert!(c.registries[2].is_local());
        assert_eq!(
            c.min_age_for(&c.registries[2]),
            Duration::from_secs(3 * 86_400)
        );

        let bare =
            Config::parse("registries { http \"https://x.example\"; dir \"/srv\" }").unwrap();
        assert_eq!(bare.min_age_for(&bare.registries[0]), NETWORK_MIN_AGE);
        assert_eq!(bare.min_age_for(&bare.registries[1]), Duration::ZERO);
    }

    #[test]
    fn edits_keep_comments_and_layout() {
        let mut c = Config::parse(CONFIG).unwrap();
        c.add_registry(RegistryConfig::new("dir", "/srv/more"));
        let text = c.to_kdl();
        assert!(text.contains("// hatchery.kdl"), "{text}");
        assert!(text.contains("// our own packages"), "{text}");
        assert!(text.contains("  dir \"/srv/more\"\n"), "{text}");
        let again = Config::parse(&text).unwrap();
        assert_eq!(again.registries.len(), 4);
        assert_eq!(again.registries[3].location, "/srv/more");

        assert!(c.remove_registry("/srv/eggs"));
        assert!(!c.remove_registry("/nowhere"));
        let text = c.to_kdl();
        assert!(!text.contains("/srv/eggs"), "{text}");
        assert!(text.contains("// our own packages"), "{text}");
        assert_eq!(Config::parse(&text).unwrap().registries.len(), 3);

        let mut empty = Config::empty();
        let mut r = RegistryConfig::new("git", "https://example.org/x.git");
        r.min_age = Some(Duration::from_secs(86_400));
        empty.add_registry(r);
        let text = empty.to_kdl();
        let again = Config::parse(&text).unwrap();
        assert_eq!(again.registries.len(), 1, "{text}");
        assert_eq!(
            again.registries[0].min_age,
            Some(Duration::from_secs(86_400))
        );
    }

    #[test]
    fn tells_local_from_network() {
        assert!(is_local_location("/srv/eggs"));
        assert!(is_local_location("./eggs"));
        assert!(is_local_location("file:///srv/repo.git"));
        assert!(is_local_location("C:/eggs"));
        assert!(!is_local_location("https://example.org/x.git"));
        assert!(!is_local_location("git@example.org:org/x.git"));
        assert!(!is_local_location("ssh://git@example.org/x.git"));
    }
}
