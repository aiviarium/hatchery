//! Glob patterns over `/`-separated relative paths: `*` and `?` within one
//! segment, `**` for any number of segments.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glob {
    segments: Vec<String>,
}

impl Glob {
    pub fn new(pattern: &str) -> Glob {
        let p = pattern
            .strip_prefix("./")
            .unwrap_or(pattern)
            .trim_end_matches('/');
        Glob {
            segments: p
                .split('/')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
        }
    }

    /// Whether the whole path matches.
    pub fn matches(&self, path: &str) -> bool {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        match_segments(&self.segments, &parts)
    }

    /// Whether the path or one of the folders it is in matches, so that a
    /// pattern naming a folder takes everything in it.
    pub fn covers(&self, path: &str) -> bool {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        (1..=parts.len()).any(|n| match_segments(&self.segments, &parts[..n]))
    }
}

fn match_segments(pat: &[String], path: &[&str]) -> bool {
    match pat.split_first() {
        None => path.is_empty(),
        Some((first, rest)) if first == "**" => {
            (0..=path.len()).any(|i| match_segments(rest, &path[i..]))
        }
        Some((first, rest)) => match path.split_first() {
            Some((p, prest)) => {
                match_one(first.as_bytes(), p.as_bytes()) && match_segments(rest, prest)
            }
            None => false,
        },
    }
}

fn match_one(pat: &[u8], s: &[u8]) -> bool {
    match pat.split_first() {
        None => s.is_empty(),
        Some((b'*', rest)) => (0..=s.len()).any(|i| match_one(rest, &s[i..])),
        Some((b'?', rest)) => !s.is_empty() && match_one(rest, &s[1..]),
        Some((c, rest)) => s.first() == Some(c) && match_one(rest, &s[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_paths() {
        assert!(Glob::new("agents/*").matches("agents/claude"));
        assert!(!Glob::new("agents/*").matches("agents/a/b"));
        assert!(Glob::new("dist/**").matches("dist/a/b.js"));
        assert!(Glob::new("**/*.map").matches("a/b/c.js.map"));
        assert!(Glob::new("**/*.map").matches("c.js.map"));
        assert!(Glob::new("host.?s").matches("host.js"));
        assert!(!Glob::new("host.js").matches("dist/host.js"));
        assert!(Glob::new("dist").covers("dist/a/b.js"));
        assert!(!Glob::new("dist").matches("dist/a/b.js"));
    }
}
