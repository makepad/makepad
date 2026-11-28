//! Text-level reading of a Splash body: matching braces, telling a child
//! literal from a property object, listing a node's children, finding a
//! property and its value, and indentation.
//!
//! This is deliberately not a parser. The runtime's parser decides whether an
//! edit is valid (the document's gate runs it); these helpers only find the
//! spans an edit touches, in text that the runtime already compiled, so the
//! shapes they meet are the shapes Splash allows: `name := Type{...}`,
//! `Type{...}`, `key: value`, `key +: {...}`, `mod.x.Y = Type{...}`.

use std::ops::Range;

pub use makepad_widgets_core::design_data::locate::{matching_brace, node_at_brace, Node, NodeKind};
use makepad_widgets_core::design_data::locate::{is_ident_byte, skip_opaque};

/// Every `{` at depth 0 inside `open+1..close`, in order, with the node it
/// opens.
fn braces_at_depth_zero(text: &str, from: usize, to: usize) -> Vec<Node> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = from;
    while i < to {
        if let Some(j) = skip_opaque(b, i) {
            i = j;
            continue;
        }
        if b[i] == b'{' {
            if let Some(node) = node_at_brace(text, i) {
                i = node.close + 1;
                out.push(node);
                continue;
            }
        }
        i += 1;
    }
    out
}

/// The child literals of a node, in source order.
pub fn children(text: &str, node: &Node) -> Vec<Node> {
    braces_at_depth_zero(text, node.open + 1, node.close)
        .into_iter()
        .filter(|n| matches!(n.kind, NodeKind::Child { .. }))
        .collect()
}

/// A property statement inside a node: the key, and the value's span.
#[derive(Clone, Debug)]
pub struct PropSpan {
    pub key_start: usize,
    pub value: Range<usize>,
    pub merge: bool,
}

/// Find `key:` or `key +:` at depth 0 in the node's body.
pub fn find_property(text: &str, node: &Node, key: &str) -> Option<PropSpan> {
    let b = text.as_bytes();
    let mut i = node.open + 1;
    while i < node.close {
        if let Some(j) = skip_opaque(b, i) {
            i = j;
            continue;
        }
        if b[i] == b'{' {
            i = matching_brace(text, i).map(|c| c + 1).unwrap_or(i + 1);
            continue;
        }
        if is_ident_byte(b[i]) && (i == 0 || !(is_ident_byte(b[i - 1]) || b[i - 1] == b'.')) {
            let mut j = i;
            while j < node.close && (is_ident_byte(b[j]) || b[j] == b'.') {
                j += 1;
            }
            let ident = &text[i..j];
            let mut k = j;
            while k < node.close && (b[k] == b' ' || b[k] == b'\t') {
                k += 1;
            }
            let (is_key, merge, after) = if b.get(k) == Some(&b'+') && b.get(k + 1) == Some(&b':') {
                (true, true, k + 2)
            } else if b.get(k) == Some(&b':') && b.get(k + 1) != Some(&b'=') {
                (true, false, k + 1)
            } else {
                (false, false, k)
            };
            if is_key && ident == key {
                let value = value_range(text, after, node.close);
                return Some(PropSpan { key_start: i, value, merge });
            }
            i = j.max(i + 1);
            continue;
        }
        i += 1;
    }
    None
}

/// The value that starts after a `:` at `from`: a brace group (with its type
/// path) to its matching `}`, or a scalar up to the end of the line, a `}`,
/// or the next `key:` on the same line.
fn value_range(text: &str, from: usize, limit: usize) -> Range<usize> {
    let b = text.as_bytes();
    let mut i = from;
    while i < limit && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    let start = i;
    // A group value: `{`, `Type{`, `Type.Path{`.
    let mut j = i;
    while j < limit && (is_ident_byte(b[j]) || b[j] == b'.') {
        j += 1;
    }
    if b.get(j) == Some(&b'{') {
        if let Some(close) = matching_brace(text, j) {
            return start..close + 1;
        }
    }
    let mut i = start;
    let mut end = start;
    while i < limit {
        if let Some(k) = skip_opaque(b, i) {
            i = k;
            end = i;
            continue;
        }
        match b[i] {
            b'\n' | b'}' => break,
            b'(' | b'[' => {
                // Skip a call or array as a unit.
                let (open, close) = (b[i], if b[i] == b'(' { b')' } else { b']' });
                let mut depth = 0usize;
                while i < limit {
                    if let Some(k) = skip_opaque(b, i) {
                        i = k;
                        continue;
                    }
                    if b[i] == open {
                        depth += 1;
                    } else if b[i] == close {
                        depth -= 1;
                        if depth == 0 {
                            i += 1;
                            break;
                        }
                    }
                    i += 1;
                }
                end = i;
                continue;
            }
            b' ' | b'\t' => {
                // Another `key:` on this line ends the value.
                let mut k = i;
                while k < limit && (b[k] == b' ' || b[k] == b'\t') {
                    k += 1;
                }
                let mut m = k;
                while m < limit && (is_ident_byte(b[m]) || b[m] == b'.') {
                    m += 1;
                }
                if m > k {
                    let mut n = m;
                    while n < limit && (b[n] == b' ' || b[n] == b'\t') {
                        n += 1;
                    }
                    if b.get(n) == Some(&b':')
                        || (b.get(n) == Some(&b'+') && b.get(n + 1) == Some(&b':'))
                        || (b.get(n) == Some(&b':') && b.get(n + 1) == Some(&b'='))
                    {
                        break;
                    }
                }
                i += 1;
            }
            _ => {
                i += 1;
                end = i;
            }
        }
    }
    start..end
}

/// The start of the line containing `offset`.
pub fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

/// The end of the line containing `offset` (the index of its `\n`, or the
/// text's end).
pub fn line_end(text: &str, offset: usize) -> usize {
    text[offset..].find('\n').map(|i| offset + i).unwrap_or(text.len())
}

/// The indentation of the line containing `offset`.
pub fn indent_at(text: &str, offset: usize) -> &str {
    let ls = line_start(text, offset);
    let b = text.as_bytes();
    let mut i = ls;
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    &text[ls..i]
}

/// The whole-line span of a node: from its line's indentation to and
/// including the newline after its `}`, when the node owns its lines.
pub fn statement_range(text: &str, node: &Node) -> Range<usize> {
    let ls = line_start(text, node.start);
    let owns_line_start = text[ls..node.start].trim().is_empty();
    let le = line_end(text, node.close);
    let owns_line_end = text[node.end()..le].trim().is_empty();
    if owns_line_start && owns_line_end {
        let end = if le < text.len() { le + 1 } else { le };
        ls..end
    } else {
        node.range()
    }
}

/// Re-indent a snippet: strip the common indentation of its lines and put
/// `indent` in front of each non-empty line.
pub fn reindent(snippet: &str, indent: &str) -> String {
    let lines: Vec<&str> = snippet.lines().collect();
    let common = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if line.trim().is_empty() {
            continue;
        }
        out.push_str(indent);
        out.push_str(&line[common.min(line.len())..]);
    }
    out
}

/// The literal that holds `node` as a child, and the node's index among that
/// parent's children, searched from the top of `body` (a block's body range).
/// `None` for a top-level node, or when `node` is not found under `body`.
pub fn parent_of(text: &str, body: Range<usize>, node: &Node) -> Option<(Node, usize)> {
    let mut level = braces_at_depth_zero(text, body.start, body.end);
    let mut parent: Option<Node> = None;
    loop {
        let Some(holder) = level
            .iter()
            .find(|n| n.start <= node.start && node.end() <= n.end())
            .cloned()
        else {
            return None;
        };
        if holder.open == node.open {
            let parent = parent?;
            let index = children(text, &parent)
                .iter()
                .position(|c| c.open == node.open)?;
            return Some((parent, index));
        }
        level = braces_at_depth_zero(text, holder.open + 1, holder.close);
        parent = Some(holder);
    }
}

/// The `{` of a property value that is an object (`{..}`, `Type{..}`), or
/// `None` for a scalar value.
pub fn value_brace(text: &str, value: &Range<usize>) -> Option<usize> {
    let b = text.as_bytes();
    let mut i = value.start;
    while i < value.end && (is_ident_byte(b[i]) || b[i] == b'.' || b[i] == b' ') {
        i += 1;
    }
    (i < value.end && b[i] == b'{').then_some(i)
}
