//! The design operations, as text edits over located nodes.
//!
//! Every operation takes the node it acts on as a [`NodeSpan`] located in the
//! document's CURRENT text, computes one replacement of one byte range, and
//! applies it through [`DesignDoc::edit`]. Untouched bytes are never
//! regenerated: a move carries the node's original bytes to its new place,
//! re-indented; a property edit replaces the value text and nothing else;
//! typed properties are written through `+:` so the inherited fields survive.

use super::doc::{DesignDoc, Hunk};
use super::locate::NodeSpan;
use super::text::{self, Node, NodeKind};

/// Where a child goes among a parent's children.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placement {
    First,
    Last,
    /// Before the child at this index (0-based, in source order).
    Before(usize),
    /// After the child at this index.
    After(usize),
}

/// A design operation. Nodes are the spans the locator produced against the
/// document's current text.
#[derive(Clone, Debug)]
pub enum DesignOp {
    /// Add a child literal (`splash`, e.g. `Button{text: "Save"}` or
    /// `save := Button{text: "Save"}`) to a parent.
    Insert { parent: NodeSpan, placement: Placement, splash: String },
    /// Remove a child literal, with its lines when it owns them.
    Delete { node: NodeSpan },
    /// Move a child literal to another parent and place. The placement
    /// indexes the parent's children as they are once the node is taken out.
    Move { node: NodeSpan, parent: NodeSpan, placement: Placement },
    /// Copy a child literal right after itself; a named node's copy is
    /// renamed `<name>_2` (or the next free suffix).
    Duplicate { node: NodeSpan },
    /// Put the node inside a new container: `wrapper` is the container's
    /// head, e.g. `View{flow: Right}`; the node becomes its only child.
    Wrap { node: NodeSpan, wrapper: String },
    /// Give the node a name (`name := Type{`), change it, or drop it.
    Rename { node: NodeSpan, name: Option<String> },
    /// Set a property on the node: replaces the value of `key:` or
    /// `key +:` when present, else adds a `key: value` line. `merge` writes
    /// `+:` for a new typed property, so a `draw_bg +: {color: #f00}` keeps
    /// the fields it does not name.
    SetProp { node: NodeSpan, key: String, value: String, merge: bool },
}

const INDENT_STEP: &str = "    ";

impl DesignOp {
    /// A short label for the hunk and the undo list.
    pub fn label(&self) -> String {
        match self {
            DesignOp::Insert { splash, .. } => format!("insert {}", first_word(splash)),
            DesignOp::Delete { node } => format!("delete {}", describe(&node.node)),
            DesignOp::Move { node, .. } => format!("move {}", describe(&node.node)),
            DesignOp::Duplicate { node } => format!("duplicate {}", describe(&node.node)),
            DesignOp::Wrap { node, wrapper } => {
                format!("wrap {} in {}", describe(&node.node), first_word(wrapper))
            }
            DesignOp::Rename { node, name } => format!(
                "rename {} to {}",
                describe(&node.node),
                name.as_deref().unwrap_or("(unnamed)")
            ),
            DesignOp::SetProp { node, key, .. } => format!("set {}.{}", describe(&node.node), key),
        }
    }

    /// Apply the operation to the document. The node spans must have been
    /// located in `doc.text()` as it is now; anything else is an error.
    pub fn apply(&self, doc: &mut DesignDoc) -> Result<Hunk, String> {
        let label = self.label();
        match self {
            DesignOp::Insert { parent, placement, splash } => {
                check_span(doc, parent)?;
                let (at, snippet) = insertion(doc.text(), &parent.node, *placement, splash);
                doc.edit(at, at, &snippet, &label)
            }
            DesignOp::Delete { node } => {
                check_span(doc, node)?;
                must_be_child(&node.node)?;
                let range = text::statement_range(doc.text(), &node.node);
                doc.edit(range.start, range.end, "", &label)
            }
            DesignOp::Move { node, parent, placement } => {
                check_span(doc, node)?;
                check_span(doc, parent)?;
                must_be_child(&node.node)?;
                if is_inside(&parent.node, &node.node) {
                    return Err("a node cannot be moved into itself".to_string());
                }
                let text = doc.text();
                let body = text[node.node.range()].to_string();
                let range = text::statement_range(text, &node.node);
                let removed = range.end - range.start;
                // The insertion point is found in the text WITHOUT the node,
                // where the parent's children are what they will be after
                // the move.
                let mut without = text.to_string();
                without.replace_range(range.clone(), "");
                let to_without = |offset: usize| -> usize {
                    if offset >= range.end {
                        offset - removed
                    } else {
                        offset.min(range.start)
                    }
                };
                let parent_after = Node {
                    start: to_without(parent.node.start),
                    open: to_without(parent.node.open),
                    close: to_without(parent.node.close),
                    ty: parent.node.ty.clone(),
                    kind: parent.node.kind.clone(),
                };
                let (at, snippet) = insertion(&without, &parent_after, *placement, &body);
                // One hunk covers the removal and the insertion, so undo is
                // one step: the span from the first to the last touched byte
                // is replaced by the same span of `without`, snippet inside.
                let at_in_text = if at >= range.start { at + removed } else { at };
                let lo = range.start.min(at_in_text);
                let hi = range.end.max(at_in_text);
                let mut middle = String::with_capacity(hi - lo + snippet.len());
                middle.push_str(&without[to_without(lo)..at]);
                middle.push_str(&snippet);
                middle.push_str(&without[at..to_without(hi)]);
                doc.edit(lo, hi, &middle, &label)
            }
            DesignOp::Duplicate { node } => {
                check_span(doc, node)?;
                must_be_child(&node.node)?;
                let text = doc.text();
                let body = &text[node.node.range()];
                let body = match node.node.name() {
                    // `save := Button{..}` becomes `save_2 := Button{..}`.
                    Some(name) => {
                        let after_name = &body[name.len()..];
                        let rest = after_name
                            .split_once(":=")
                            .map(|(_, rest)| rest)
                            .unwrap_or(after_name);
                        format!("{} :={}", free_name(text, name), rest)
                    }
                    None => body.to_string(),
                };
                let range = text::statement_range(text, &node.node);
                let snippet = if owns_lines(&range, &node.node) {
                    format!("{}\n", text::reindent(&body, text::indent_at(text, node.node.start)))
                } else {
                    format!(" {}", body)
                };
                doc.edit(range.end, range.end, &snippet, &label)
            }
            DesignOp::Wrap { node, wrapper } => {
                check_span(doc, node)?;
                must_be_child(&node.node)?;
                let text = doc.text();
                let body = &text[node.node.range()];
                let indent = text::indent_at(text, node.node.start);
                let head = wrapper.trim().trim_end_matches('}').trim_end();
                let head = if head.contains('{') {
                    head.to_string()
                } else {
                    format!("{}{{", head)
                };
                let inner_indent = format!("{}{}", indent, INDENT_STEP);
                let wrapped = format!(
                    "{}\n{}\n{}}}",
                    head,
                    text::reindent(body, &inner_indent),
                    indent
                );
                doc.edit(node.node.start, node.node.end(), &wrapped, &label)
            }
            DesignOp::Rename { node, name } => {
                check_span(doc, node)?;
                let text = doc.text();
                let NodeKind::Child { name: old } = &node.node.kind else {
                    return Err("only a child literal can be renamed".to_string());
                };
                match (old, name) {
                    (None, None) => return Err("the node has no name to drop".to_string()),
                    (_, Some(name)) if !is_ident(name) => {
                        return Err(format!("{:?} is not a valid name", name))
                    }
                    _ => {}
                }
                // From the node's start to its type path: `old :=`, or
                // nothing when it is unnamed.
                let after_old = node.node.start + old.as_ref().map_or(0, |o| o.len());
                let path_start = text[after_old..node.node.open]
                    .find(|c: char| !matches!(c, ' ' | '\t' | ':' | '='))
                    .map_or(node.node.open, |i| after_old + i);
                let replacement = name.as_ref().map_or(String::new(), |n| format!("{} := ", n));
                doc.edit(node.node.start, path_start, &replacement, &label)
            }
            DesignOp::SetProp { node, key, value, merge } => {
                check_span(doc, node)?;
                if !is_path(key) {
                    return Err(format!("{:?} is not a property name", key));
                }
                let text = doc.text();
                let value = value.trim();
                if let Some(prop) = text::find_property(text, &node.node, key) {
                    return doc.edit(prop.value.start, prop.value.end, value, &label);
                }
                let close = node.node.close;
                let line = format!("{}{}{}", key, if *merge { " +: " } else { ": " }, value);
                if !text[node.node.open..close].contains('\n') {
                    // A one-line node stays one line: `{a: 1}` → `{a: 1 b: 2}`.
                    if text[node.node.open + 1..close].trim().is_empty() {
                        let at = node.node.open + 1;
                        doc.edit(at, at, &line, &label)
                    } else {
                        let at = trim_end_offset(text, close);
                        doc.edit(at, at, &format!(" {}", line), &label)
                    }
                } else {
                    // A line of its own before the closing brace.
                    let inner = format!("{}{}", text::indent_at(text, node.node.start), INDENT_STEP);
                    let at = text::line_start(text, close);
                    doc.edit(at, at, &format!("{}{}\n", inner, line), &label)
                }
            }
        }
    }
}

/// The node's span must come from the document's current text.
fn check_span(doc: &DesignDoc, span: &NodeSpan) -> Result<(), String> {
    if span.text != doc.text() {
        return Err("the node was located in an older text; locate it again".to_string());
    }
    Ok(())
}

fn must_be_child(node: &Node) -> Result<(), String> {
    match node.kind {
        NodeKind::Child { .. } => Ok(()),
        NodeKind::Property { ref key, .. } => Err(format!("{} is a property, not a child", key)),
        NodeKind::Assign => Err("the block's root cannot be moved or deleted".to_string()),
        NodeKind::Other => Err("not a widget literal".to_string()),
    }
}

fn is_inside(inner: &Node, outer: &Node) -> bool {
    outer.start <= inner.start && inner.end() <= outer.end()
}

/// Whether a statement range is the node's whole lines rather than the node's
/// own bytes (see [`text::statement_range`]).
fn owns_lines(range: &std::ops::Range<usize>, node: &Node) -> bool {
    *range != node.range()
}

/// Where and what to insert for a child of `parent` at `placement`, in
/// `text`. The snippet is re-indented to the parent's child indentation and
/// takes a line of its own.
fn insertion(text: &str, parent: &Node, placement: Placement, splash: &str) -> (usize, String) {
    let children = text::children(text, parent);
    let parent_indent = text::indent_at(text, parent.start).to_string();
    let child_indent = children
        .first()
        .map(|c| text::indent_at(text, c.start).to_string())
        .unwrap_or_else(|| format!("{}{}", parent_indent, INDENT_STEP));
    let body = text::reindent(splash.trim(), &child_indent);
    if !text[parent.open..parent.close].contains('\n') {
        // A one-line parent, `View{}` or `View{a: 1}`: the child goes on a
        // line of its own before the closing brace, which moves to a line of
        // its own too; what was inside stays on the opening line.
        let inside_empty = text[parent.open + 1..parent.close].trim().is_empty();
        let at = if inside_empty {
            parent.open + 1
        } else {
            trim_end_offset(text, parent.close)
        };
        return (at, format!("\n{}\n{}", body, parent_indent));
    }
    let at = match placement {
        Placement::First => children.first().map(|c| text::line_start(text, c.start)),
        Placement::Before(i) => children.get(i).map(|c| text::line_start(text, c.start)),
        Placement::After(i) => children.get(i).map(|c| after_statement(text, c)),
        Placement::Last => None,
    }
    .unwrap_or_else(|| text::line_start(text, parent.close));
    (at, format!("{}\n", body))
}

/// The offset after the line a child ends on: its statement's end when it
/// owns its lines, else the start of the line after its closing brace.
fn after_statement(text: &str, node: &Node) -> usize {
    let range = text::statement_range(text, node);
    if owns_lines(&range, node) {
        range.end
    } else {
        (text::line_end(text, node.close) + 1).min(text.len())
    }
}

/// The offset before the trailing spaces that end at `offset`.
fn trim_end_offset(text: &str, offset: usize) -> usize {
    let b = text.as_bytes();
    let mut i = offset;
    while i > 0 && (b[i - 1] == b' ' || b[i - 1] == b'\t') {
        i -= 1;
    }
    i
}

/// `name_2`, `name_3`, ... : the first not used as a `:=` name in `text`.
fn free_name(text: &str, name: &str) -> String {
    let base = name
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .trim_end_matches('_');
    for n in 2..1000 {
        let candidate = format!("{}_{}", base, n);
        if !text.contains(&format!("{} :=", candidate)) && !text.contains(&format!("{}:=", candidate))
        {
            return candidate;
        }
    }
    format!("{}_copy", base)
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_path(s: &str) -> bool {
    !s.is_empty() && s.split('.').all(is_ident)
}

fn first_word(s: &str) -> String {
    s.trim()
        .split(|c: char| c == '{' || c.is_whitespace())
        .next()
        .unwrap_or("")
        .to_string()
}

fn describe(node: &Node) -> String {
    match node.name() {
        Some(name) => format!("{} ({})", name, node.ty),
        None => node.ty.clone(),
    }
}
