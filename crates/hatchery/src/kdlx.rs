//! Small helpers for reading and writing KDL version 2 documents.

use kdl::{KdlDocument, KdlEntry, KdlNode, KdlValue};

use crate::error::{Error, Result};

/// Parses a KDL v2 document, naming `what` and the line of the first
/// problem in the error.
pub fn parse(text: &str, what: &str) -> Result<KdlDocument> {
    KdlDocument::parse_v2(text).map_err(|e| {
        let detail = e
            .diagnostics
            .first()
            .map(|d| {
                let offset = d.span.offset();
                let line = text.chars().take(offset).filter(|&c| c == '\n').count() + 1;
                let msg = d
                    .message
                    .clone()
                    .or_else(|| d.label.clone())
                    .unwrap_or_else(|| "syntax error".to_string());
                match &d.help {
                    Some(h) => format!("line {line}: {msg} ({h})"),
                    None => format!("line {line}: {msg}"),
                }
            })
            .unwrap_or_else(|| "not a KDL version 2 document".to_string());
        Error::invalid(what, detail)
    })
}

pub fn name(node: &KdlNode) -> &str {
    node.name().value()
}

/// The arguments of a node, in order (entries without a name).
pub fn args(node: &KdlNode) -> impl Iterator<Item = &KdlValue> {
    node.entries()
        .iter()
        .filter(|e| e.name().is_none())
        .map(KdlEntry::value)
}

/// The properties of a node, in order.
pub fn props(node: &KdlNode) -> impl Iterator<Item = (&str, &KdlValue)> {
    node.entries()
        .iter()
        .filter_map(|e| e.name().map(|n| (n.value(), e.value())))
}

pub fn prop<'a>(node: &'a KdlNode, key: &str) -> Option<&'a KdlValue> {
    props(node)
        .filter(|(k, _)| *k == key)
        .map(|(_, v)| v)
        .last()
}

/// A value as text: strings as they are, numbers and booleans written out.
pub fn text(v: &KdlValue) -> Option<String> {
    match v {
        KdlValue::String(s) => Some(s.clone()),
        KdlValue::Integer(i) => Some(i.to_string()),
        KdlValue::Float(f) => Some(f.to_string()),
        KdlValue::Bool(b) => Some(b.to_string()),
        KdlValue::Null => None,
    }
}

pub fn prop_string(node: &KdlNode, key: &str, what: &str) -> Result<Option<String>> {
    match prop(node, key) {
        None => Ok(None),
        Some(v) => text(v).map(Some).ok_or_else(|| {
            Error::invalid(what, format!("{key}= on {} must be a string", name(node)))
        }),
    }
}

/// Every argument as text.
pub fn strings(node: &KdlNode, what: &str) -> Result<Vec<String>> {
    args(node)
        .map(|v| {
            text(v)
                .ok_or_else(|| Error::invalid(what, format!("{} has a #null argument", name(node))))
        })
        .collect()
}

/// The single argument of a node, as text.
pub fn one_string(node: &KdlNode, what: &str) -> Result<String> {
    let all = strings(node, what)?;
    match <[String; 1]>::try_from(all) {
        Ok([s]) => Ok(s),
        Err(v) => Err(Error::invalid(
            what,
            format!("{} takes one value, not {}", name(node), v.len()),
        )),
    }
}

pub fn children(node: &KdlNode) -> &[KdlNode] {
    node.children().map(KdlDocument::nodes).unwrap_or(&[])
}

/// Checks that a node has no properties other than `allowed`.
pub fn only_props(node: &KdlNode, allowed: &[&str], what: &str) -> Result<()> {
    for (k, _) in props(node) {
        if !allowed.contains(&k) {
            return Err(Error::invalid(
                what,
                format!("{} has an unknown property {k}=", name(node)),
            ));
        }
    }
    Ok(())
}

// Writing.

/// A node with string arguments.
pub fn node<S: AsRef<str>>(name: &str, args: &[S]) -> KdlNode {
    let mut n = KdlNode::new(name);
    for a in args {
        n.entries_mut().push(KdlEntry::new(a.as_ref().to_string()));
    }
    n
}

pub fn with_prop(mut n: KdlNode, key: &str, value: impl Into<KdlValue>) -> KdlNode {
    n.entries_mut().push(KdlEntry::new_prop(key, value));
    n
}

pub fn with_children(mut n: KdlNode, nodes: Vec<KdlNode>) -> KdlNode {
    if !nodes.is_empty() {
        let mut doc = KdlDocument::new();
        doc.nodes_mut().extend(nodes);
        n.set_children(doc);
    }
    n
}

/// Writes top-level nodes, autoformatted, with a blank line between them.
pub fn write(nodes: Vec<KdlNode>) -> String {
    let mut out = String::new();
    for (i, n) in nodes.into_iter().enumerate() {
        let mut doc = KdlDocument::new();
        doc.nodes_mut().push(n);
        doc.autoformat();
        if i > 0 {
            out.push('\n');
        }
        out.push_str(doc.to_string().trim_end());
        out.push('\n');
    }
    out
}
