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

/// What a `{` opens, judged from the text before it.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeKind {
    /// `Type{...}` or `name := Type{...}`: a child literal.
    Child { name: Option<String> },
    /// `key: {...}` or `key +: {...}`: a property object, not a child.
    Property { key: String, merge: bool },
    /// `path = Type{...}` or `let x = Type{...}`: an assignment.
    Assign,
    /// Anything else (a block after `do`, a fn body, an argument).
    Other,
}

/// One literal: `start..end` is the whole declaration, `open` its `{` and
/// `close` its `}`, `ty` the type path before the brace.
#[derive(Clone, Debug)]
pub struct Node {
    pub start: usize,
    pub open: usize,
    pub close: usize,
    pub ty: String,
    pub kind: NodeKind,
}

impl Node {
    pub fn end(&self) -> usize {
        self.close + 1
    }
    pub fn range(&self) -> Range<usize> {
        self.start..self.end()
    }
    pub fn name(&self) -> Option<&str> {
        match &self.kind {
            NodeKind::Child { name } => name.as_deref(),
            _ => None,
        }
    }
}

/// Skip a string, comment or `#(...)` placeholder that starts at `i`, or
/// return `None` when nothing opaque starts there.
fn skip_opaque(b: &[u8], i: usize) -> Option<usize> {
    match b[i] {
        b'"' => {
            let mut j = i + 1;
            while j < b.len() {
                match b[j] {
                    b'\\' => j += 2,
                    b'"' => return Some(j + 1),
                    _ => j += 1,
                }
            }
            Some(b.len())
        }
        b'r' if b.get(i + 1) == Some(&b'"') || (b.get(i + 1) == Some(&b'#')) => {
            // Raw string r"..." or r#"..."#.
            let mut hashes = 0;
            let mut j = i + 1;
            while b.get(j) == Some(&b'#') {
                hashes += 1;
                j += 1;
            }
            if b.get(j) != Some(&b'"') {
                return None;
            }
            j += 1;
            while j < b.len() {
                if b[j] == b'"' {
                    let mut k = j + 1;
                    let mut n = 0;
                    while n < hashes && b.get(k) == Some(&b'#') {
                        n += 1;
                        k += 1;
                    }
                    if n == hashes {
                        return Some(k);
                    }
                }
                j += 1;
            }
            Some(b.len())
        }
        b'/' if b.get(i + 1) == Some(&b'/') => {
            let mut j = i;
            while j < b.len() && b[j] != b'\n' {
                j += 1;
            }
            Some(j)
        }
        b'/' if b.get(i + 1) == Some(&b'*') => {
            let mut j = i + 2;
            while j + 1 < b.len() {
                if b[j] == b'*' && b[j + 1] == b'/' {
                    return Some(j + 2);
                }
                j += 1;
            }
            Some(b.len())
        }
        b'#' if b.get(i + 1) == Some(&b'(') => {
            let mut depth = 0usize;
            let mut j = i + 1;
            while j < b.len() {
                if let Some(k) = if b[j] == b'"' || (b[j] == b'/' && j + 1 < b.len()) {
                    skip_opaque(b, j)
                } else {
                    None
                } {
                    j = k;
                    continue;
                }
                match b[j] {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            Some(b.len())
        }
        _ => None,
    }
}

/// The index of the `}` matching the `{` at `open`.
pub fn matching_brace(text: &str, open: usize) -> Option<usize> {
    let b = text.as_bytes();
    if b.get(open) != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut i = open;
    while i < b.len() {
        if let Some(j) = skip_opaque(b, i) {
            i = j;
            continue;
        }
        match b[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Walk back over whitespace from `i` (exclusive), returning the index after
/// the last non-space byte.
fn rskip_ws(b: &[u8], mut i: usize) -> usize {
    while i > 0 && (b[i - 1] == b' ' || b[i - 1] == b'\t') {
        i -= 1;
    }
    i
}

/// The type path ending at `end` (exclusive): idents joined by dots.
fn rpath(b: &[u8], end: usize) -> usize {
    let mut i = end;
    while i > 0 && (is_ident_byte(b[i - 1]) || b[i - 1] == b'.') {
        i -= 1;
    }
    i
}

/// Describe the literal whose `{` is at `open`.
pub fn node_at_brace(text: &str, open: usize) -> Option<Node> {
    let b = text.as_bytes();
    let close = matching_brace(text, open)?;
    let ty_end = rskip_ws(b, open);
    let ty_start = rpath(b, ty_end);
    let ty = text[ty_start..ty_end].to_string();
    // Before the type path: `name :=`, `key:`, `key +:`, `=`, or the start of
    // a statement.
    let before = rskip_ws(b, ty_start);
    let kind = if ty.is_empty() {
        // A bare `{`: `key: {` / `key +: {` / `= {` / `do {`.
        if before >= 2 && &b[before - 2..before] == b"+:" {
            let key_end = rskip_ws(b, before - 2);
            let key_start = rpath(b, key_end);
            NodeKind::Property { key: text[key_start..key_end].to_string(), merge: true }
        } else if before >= 1 && b[before - 1] == b':' {
            let key_end = rskip_ws(b, before - 1);
            let key_start = rpath(b, key_end);
            NodeKind::Property { key: text[key_start..key_end].to_string(), merge: false }
        } else if before >= 1 && b[before - 1] == b'=' {
            NodeKind::Assign
        } else {
            NodeKind::Other
        }
    } else if before >= 2 && &b[before - 2..before] == b":=" {
        let name_end = rskip_ws(b, before - 2);
        let name_start = rpath(b, name_end);
        NodeKind::Child { name: Some(text[name_start..name_end].to_string()) }
    } else if before >= 2 && &b[before - 2..before] == b"+:" {
        let key_end = rskip_ws(b, before - 2);
        let key_start = rpath(b, key_end);
        NodeKind::Property { key: text[key_start..key_end].to_string(), merge: true }
    } else if before >= 1 && b[before - 1] == b':' {
        let key_end = rskip_ws(b, before - 1);
        let key_start = rpath(b, key_end);
        NodeKind::Property { key: text[key_start..key_end].to_string(), merge: false }
    } else if before >= 1 && b[before - 1] == b'=' {
        NodeKind::Assign
    } else if before >= 2 && &b[before - 2..before] == b"do" {
        NodeKind::Other
    } else {
        NodeKind::Child { name: None }
    };
    let start = match &kind {
        NodeKind::Child { name: Some(_) } => {
            let name_end = rskip_ws(b, before - 2);
            rpath(b, name_end)
        }
        NodeKind::Property { .. } => {
            let colon = if before >= 2 && &b[before - 2..before] == b"+:" {
                before - 2
            } else {
                before - 1
            };
            let key_end = rskip_ws(b, colon);
            rpath(b, key_end)
        }
        _ => ty_start,
    };
    Some(Node { start, open, close, ty, kind })
}

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
