//! `hatchery.kdl`: where eggs are installed from.
//!
//! Tools edit the file through [`Config::add_registry`] and
//! [`Config::remove_registry`], which keep its comments and layout.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use kdl::{KdlDocument, KdlDocumentFormat, KdlEntry, KdlNode};

use crate::error::{Error, IoContext, Result};
use crate::kdlx;
use crate::time::{format_duration, parse_duration};

/// The name of the configuration file, in the current folder and in the user's configuration folder.
pub const FILE: &str = "hatchery.kdl";
/// The variable that points at a different configuration file.
pub const ENV: &str = "HATCHERY_CONFIG";
/// How long a registry read over the network holds a new version when
/// neither the registry nor the `default` line says.
pub const NETWORK_MIN_AGE: Duration = Duration::from_secs(3 * 86_400);

/// The registries an installation searches and how long new versions wait. It is read from `hatchery.kdl`, and editing it keeps the file's comments and layout.
#[derive(Debug, Clone)]
pub struct Config {
    /// The minimum release age of registries that have no `min-age` of their own. None when the file has no `default` line. [`Config::min_age_for`] gives the age that applies.
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
    /// The minimum release age of this registry. None means the `default` line, or the built-in age when there is none.
    pub min_age: Option<Duration>,
    /// The environment variable that carries a token.
    pub token_env: Option<String>,
    /// For a `git` registry with a `key`: whether versions that have only a
    /// tag, and so no signed egg, are still installed. False by default.
    pub unsigned_tags: bool,
    /// Every other property, for kinds a tool adds.
    pub props: BTreeMap<String, String>,
}

impl RegistryConfig {
    /// A registry line with only its kind and location. The other settings are empty.
    pub fn new(kind: impl Into<String>, location: impl Into<String>) -> RegistryConfig {
        RegistryConfig {
            kind: kind.into(),
            location: location.into(),
            only: Vec::new(),
            key: None,
            min_age: None,
            token_env: None,
            unsigned_tags: false,
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
        if self.unsigned_tags {
            n = kdlx::with_prop(n, "unsigned-tags", true);
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

/// Reads one registry line. An error names the line, such as
/// `registry http "https://example.org": key: …`.
fn parse_registry(n: &KdlNode, what: &str) -> Result<RegistryConfig> {
    parse_registry_line(n, what).map_err(|e| match e {
        Error::Invalid { message, .. } => {
            let place = match kdlx::args(n).next().and_then(kdlx::text) {
                Some(loc) => format!("{} {loc:?}", kdlx::name(n)),
                None => kdlx::name(n).to_string(),
            };
            Error::invalid(what, format!("registry {place}: {message}"))
        }
        e => e,
    })
}

fn parse_registry_line(n: &KdlNode, what: &str) -> Result<RegistryConfig> {
    let mut r = RegistryConfig::new(kdlx::name(n), kdlx::one_string(n, what)?);
    for (k, v) in kdlx::props(n) {
        if k == "unsigned-tags" {
            r.unsigned_tags = v
                .as_bool()
                .ok_or_else(|| Error::invalid(what, "unsigned-tags= must be #true or #false"))?;
            continue;
        }
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
                crate::sign::parse_verifying_key(&v).map_err(key_error)?;
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

    /// Reads the text of a `hatchery.kdl`. Fails with [`Error::Invalid`] when the KDL is wrong, a duration is not valid, or a `key` is not an Ed25519 public key.
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

    /// Reads a `hatchery.kdl` from a path. Fails with [`Error::Io`] when the file cannot be read, and with [`Error::Invalid`] naming the path when it is not valid.
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
    /// with the same location. A new line gets the indentation of its
    /// siblings, or two spaces when there are none.
    pub fn add_registry(&mut self, r: RegistryConfig) {
        let mut node = r.to_node();
        node.autoformat();
        let at = self
            .doc
            .nodes()
            .iter()
            .position(|n| kdlx::name(n) == "registries");
        let at = at.unwrap_or_else(|| {
            let top = self.doc.nodes_mut();
            let mut regs = KdlNode::new("registries");
            regs.autoformat();
            if let Some(last) = top.last_mut() {
                // A blank line between the old last node and the new one.
                if let Some(f) = last.format_mut()
                    && f.terminator.is_empty()
                {
                    f.terminator = "\n".to_string();
                }
                if let Some(f) = regs.format_mut() {
                    f.leading = "\n".to_string();
                }
            }
            top.push(regs);
            top.len() - 1
        });
        let children = self.doc.nodes_mut()[at].ensure_children();
        let existing = children.nodes().iter().position(|n| {
            kdlx::args(n).next().and_then(kdlx::text).as_deref() == Some(r.location.as_str())
        });
        match existing {
            Some(i) => {
                let old = children.nodes()[i].format().cloned();
                if let (Some(f), Some(nf)) = (old, node.format_mut()) {
                    nf.leading = f.leading;
                    nf.trailing = f.trailing;
                    nf.terminator = f.terminator;
                    nf.before_terminator = f.before_terminator;
                }
                children.nodes_mut()[i] = node;
            }
            None => push_line(children, node),
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
                while let Some(i) = c.nodes().iter().position(|r| {
                    kdlx::args(r).next().and_then(kdlx::text).as_deref() == Some(location)
                }) {
                    drop_line(c, i);
                }
            }
        }
        self.registries.len() != before
    }

    /// The file's text, with its comments and layout.
    pub fn to_kdl(&self) -> String {
        self.doc.to_string()
    }

    /// Writes the file to `path`, replacing it in one step. Fails with [`Error::Io`] when it cannot be written.
    pub fn save(&self, path: &Path) -> Result<()> {
        crate::fsx::write_atomic(path, self.to_kdl().as_bytes())
    }
}

/// Puts `key: ` before the reason a public key is refused.
fn key_error(e: Error) -> Error {
    match e {
        Error::Invalid { message, .. } => Error::invalid("key", format!("key: {message}")),
        e => e,
    }
}

/// The text before the last newline of a node's leading space, newline
/// included. It holds the comments and blank lines above the node.
fn above(leading: &str) -> &str {
    leading.rfind('\n').map_or("", |i| &leading[..=i])
}

/// Appends a line to a block, written like the lines already there.
fn push_line(children: &mut KdlDocument, mut node: KdlNode) {
    let leading_of = |n: &KdlNode| n.format().map(|f| f.leading.clone()).unwrap_or_default();
    let multiline = match children.nodes().last() {
        Some(_) => children
            .nodes()
            .iter()
            .any(|n| leading_of(n).contains('\n')),
        None => children.format().is_some_and(|f| f.leading.contains('\n')),
    };
    if !multiline && !children.nodes().is_empty() {
        // `registries { dir "/a" }`: stay on the line.
        if let Some(f) = children
            .nodes_mut()
            .last_mut()
            .and_then(KdlNode::format_mut)
        {
            f.terminator = ";".to_string();
            f.before_terminator = String::new();
        }
        if let Some(f) = node.format_mut() {
            f.leading = " ".to_string();
            f.terminator = String::new();
            f.before_terminator = " ".to_string();
        }
        children.nodes_mut().push(node);
        return;
    }
    let indent = match children.nodes().last() {
        Some(n) => {
            let l = leading_of(n);
            l.rsplit('\n').next().unwrap_or("").to_string()
        }
        None => "  ".to_string(),
    };
    if let Some(last) = children
        .nodes_mut()
        .last_mut()
        .and_then(KdlNode::format_mut)
        && !last.terminator.contains('\n')
    {
        last.terminator = "\n".to_string();
    }
    if let Some(f) = node.format_mut() {
        f.leading = indent;
        f.terminator = "\n".to_string();
    }
    if children.nodes().is_empty() {
        if children.format().is_none() {
            children.set_format(KdlDocumentFormat::default());
        }
        if let Some(f) = children.format_mut()
            && !f.leading.contains('\n')
        {
            f.leading = "\n".to_string();
        }
    }
    children.nodes_mut().push(node);
}

/// Removes line `i` of a block. The comments above it stay, and no blank
/// line is left.
fn drop_line(children: &mut KdlDocument, i: usize) {
    let gone = children.nodes_mut().remove(i);
    let leading = gone.format().map(|f| f.leading.clone()).unwrap_or_default();
    let carry = above(&leading).to_string();
    // Blank lines go. The newline after `{` stays when the first line goes.
    let blank = carry.trim().is_empty();
    match children.nodes_mut().get_mut(i) {
        Some(next) => {
            if let Some(f) = next.format_mut() {
                let keep = if blank && (i > 0 || f.leading.contains('\n')) {
                    ""
                } else {
                    &carry
                };
                f.leading = format!("{keep}{}", f.leading);
            }
        }
        None if i == 0 => {
            if let Some(f) = children.format_mut() {
                f.leading = format!("{}{carry}", f.leading);
                if blank && !f.leading.contains('\n') && leading.contains('\n') {
                    f.leading = "\n".to_string();
                }
            }
        }
        None => {
            if !blank && let Some(f) = children.format_mut() {
                f.trailing = format!("{carry}{}", f.trailing);
            }
        }
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
    fn a_bad_key_names_its_registry_line() {
        let text = "registries {\n  dir \"/srv/eggs\"\n  http \"https://eggs.example.org\" key=\"ed25519:9a3f\u{2026}\"\n}\n";
        let e = Config::parse(text).unwrap_err().to_string();
        assert_eq!(
            e,
            "hatchery.kdl: registry http \"https://eggs.example.org\": key: must be ed25519: and 64 hex digits"
        );
    }

    #[test]
    fn added_lines_take_the_indent_of_their_siblings() {
        let add = |text: &str| {
            let mut c = Config::parse(text).unwrap();
            c.add_registry(RegistryConfig::new("dir", "/new"));
            c.to_kdl()
        };
        assert_eq!(
            add("registries {\n  // c\n  dir \"/a\"\n}\n\n// end\n"),
            "registries {\n  // c\n  dir \"/a\"\n  dir \"/new\"\n}\n\n// end\n"
        );
        assert_eq!(
            add("registries {\n\tdir \"/a\"\n}\n"),
            "registries {\n\tdir \"/a\"\n\tdir \"/new\"\n}\n"
        );
        assert_eq!(
            add("registries {\n}\n"),
            "registries {\n  dir \"/new\"\n}\n"
        );
        assert_eq!(
            add("default min-age=\"3d\"\n"),
            "default min-age=\"3d\"\n\nregistries {\n  dir \"/new\"\n}\n"
        );
        assert_eq!(add(""), "registries {\n  dir \"/new\"\n}\n");
    }

    #[test]
    fn removed_lines_leave_no_blank_line() {
        let remove = |text: &str, loc: &str| {
            let mut c = Config::parse(text).unwrap();
            assert!(c.remove_registry(loc));
            c.to_kdl()
        };
        let text = "// top\nregistries {\n  dir \"/a\"\n  dir \"/b\"\n  dir \"/c\"\n}\n";
        assert_eq!(
            remove(text, "/a"),
            "// top\nregistries {\n  dir \"/b\"\n  dir \"/c\"\n}\n"
        );
        assert_eq!(
            remove(text, "/b"),
            "// top\nregistries {\n  dir \"/a\"\n  dir \"/c\"\n}\n"
        );
        assert_eq!(
            remove(text, "/c"),
            "// top\nregistries {\n  dir \"/a\"\n  dir \"/b\"\n}\n"
        );
        assert_eq!(
            remove("registries {\n  dir \"/a\"\n}\n", "/a"),
            "registries {\n}\n"
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
