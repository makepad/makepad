//! Structural edits. Each op replaces only the bytes that change, parses the
//! result again and checks that the new tree is the old tree with exactly the
//! intended change, so an edit that would change the meaning of its
//! surroundings (a replacement that merges into a neighbouring operator
//! sequence, text that does not parse as one element) is refused, never applied.
//!
//! Ops take the address ([`Loc`]) of the node they work in and the text to
//! put there (written by [`super::write`] or copied from another node);
//! they return the new [`Syntax`] and the address of the changed node.
//!
//! The `*_edits` functions are the same ops as plans: they return the byte
//! replacements for a node of a parsed text and change nothing, so a caller
//! that combines several ops (a layer's keys shifted and its span set) can
//! apply them as one [`Syntax::apply`].

use super::*;

/// Where among a container's elements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum At {
    Start,
    End,
    /// Before the element at this index.
    Before(usize),
    /// After the element at this index.
    After(usize),
    /// Before the first element, after the comment lines that open the
    /// container (a file's header comment stays first).
    AfterHeader,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditError {
    pub message: String,
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

fn err<T>(message: impl Into<String>) -> Result<T, EditError> {
    Err(EditError { message: message.into() })
}

/// The result of an edit.
#[derive(Clone, Debug)]
pub struct Edited {
    pub syntax: Syntax,
    /// The node that was inserted or replaced (the container, for a removal).
    pub loc: Loc,
}

/// A byte range replaced with text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replace {
    pub range: Range<usize>,
    pub text: String,
}

impl Replace {
    pub fn new(range: Range<usize>, text: impl Into<String>) -> Self {
        Self { range, text: text.into() }
    }
}

/// What a tree is, ignoring where it sits and the text between its nodes.
#[derive(Clone, Debug, PartialEq)]
struct Shape {
    label: String,
    names: Vec<String>,
    kids: Vec<Shape>,
}

fn shape(node: &Node, src: &str) -> Shape {
    let mut names = Vec::new();
    let label = match &node.kind {
        Kind::Number(_) | Kind::Str(_) | Kind::Name(_) | Kind::Color(_) | Kind::Ident(_) => format!("{}:{}", node.kind_name(), node.text(src)),
        Kind::Object { ty, fields, .. } => {
            for f in fields {
                names.push(format!("{}{}", f.name.as_deref().unwrap_or(""), f.op));
            }
            format!("object:{}", ty.as_deref().unwrap_or(""))
        }
        Kind::Unary { op, .. } => format!("unary:{op}"),
        Kind::Chain { rest, .. } => {
            names.extend(rest.iter().map(|l| l.op.clone()));
            "operators".to_string()
        }
        Kind::Assign { op, .. } => format!("assign:{op}"),
        Kind::Entry { name, op, .. } => format!("entry:{name}{op}"),
        Kind::Member { name, .. } => format!("member:{name}"),
        Kind::Let { keyword, names: n, .. } => {
            names.extend(n.iter().cloned());
            format!("let:{keyword}")
        }
        Kind::Fn { name, params, .. } => {
            names.extend(params.iter().map(|p| p.name.clone()));
            format!("fn:{}", name.as_deref().unwrap_or(""))
        }
        Kind::Jump { keyword, .. } => format!("jump:{keyword}"),
        Kind::For { binding, .. } => format!("for:{}", &src[binding.clone()]),
        _ => node.kind_name().to_string(),
    };
    Shape { label, names, kids: node.children().into_iter().map(|c| shape(c, src)).collect() }
}

impl Shape {
    fn at_mut(&mut self, loc: &[usize]) -> &mut Shape {
        let mut s = self;
        for &i in loc {
            s = &mut s.kids[i];
        }
        s
    }
}

/// How a container lists its elements.
struct Elements {
    /// Each element's whole span (a field's span includes its name).
    spans: Vec<Range<usize>>,
    /// Just after the opening bracket, and the closing bracket.
    inner: Range<usize>,
    /// Index of the first element among the node's children.
    offset: usize,
    /// Elements are object entries (children are their values).
    fields: bool,
    kind: ContainerKind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ContainerKind {
    Block,
    Object,
    List,
}

fn elements(node: &Node, source_len: usize) -> Option<Elements> {
    Some(match &node.kind {
        Kind::Block(b) => Elements {
            spans: b.stmts.iter().map(|s| s.span.clone()).collect(),
            inner: b.open.map_or(0, |o| o + 1)..b.close.unwrap_or(source_len),
            offset: 0,
            fields: false,
            kind: ContainerKind::Block,
        },
        Kind::Object { fields, open, close, .. } => Elements { spans: fields.iter().map(|f| f.span.clone()).collect(), inner: open + 1..*close, offset: 0, fields: true, kind: ContainerKind::Object },
        Kind::Array { items, open, close } | Kind::Paren { items, open, close } => Elements { spans: items.iter().map(|s| s.span.clone()).collect(), inner: open + 1..*close, offset: 0, fields: false, kind: ContainerKind::List },
        Kind::Call { args, open, close, .. } => Elements { spans: args.iter().map(|s| s.span.clone()).collect(), inner: open + 1..*close, offset: 1, fields: false, kind: ContainerKind::List },
        _ => return None,
    })
}

fn line_start(src: &str, at: usize) -> usize {
    src[..at].rfind('\n').map_or(0, |i| i + 1)
}

fn line_indent(src: &str, at: usize) -> &str {
    let start = line_start(src, at);
    let rest = &src[start..];
    let len = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    &rest[..len]
}

/// Nothing but blanks before `at` on its line.
fn starts_line(src: &str, at: usize) -> bool {
    src[line_start(src, at)..at].chars().all(|c| c == ' ' || c == '\t')
}

/// After `end`, only blanks, one `,` / `;`, and a `//` comment before the
/// line ends. Returns the byte of the line break (or the end of the text) if so.
fn ends_line(src: &str, end: usize, limit: usize) -> Option<usize> {
    let limit = limit.max(end).min(src.len());
    let rest = &src[end..limit];
    let bytes = rest.as_bytes();
    let mut at = 0;
    let mut sep_seen = false;
    while at < bytes.len() {
        match bytes[at] {
            b' ' | b'\t' | b'\r' => at += 1,
            b',' | b';' if !sep_seen => {
                sep_seen = true;
                at += 1;
            }
            b'/' if rest[at..].starts_with("//") => {
                at = rest[at..].find('\n').map_or(bytes.len(), |n| at + n);
            }
            b'\n' => return Some(end + at),
            _ => return None,
        }
    }
    (limit >= src.len()).then_some(end + at)
}

/// The end of the separator (`,` or `;`) that follows `end` on its line,
/// with the blanks around it; `end` itself when there is none.
fn after_separator(src: &str, end: usize) -> usize {
    let rest = &src[end..];
    let blanks = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    match rest[blanks..].chars().next() {
        Some(c @ (',' | ';')) => end + blanks + c.len_utf8(),
        _ => end,
    }
}

/// The first `,` or `;` in `gap`, outside comments.
fn separator_char(gap: &str) -> Option<char> {
    let bytes = gap.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if gap[i..].starts_with("//") {
            i = gap[i..].find('\n').map_or(bytes.len(), |n| i + n);
        } else if gap[i..].starts_with("/*") {
            i = gap[i..].find("*/").map_or(bytes.len(), |n| i + n + 2);
        } else if bytes[i] == b',' || bytes[i] == b';' {
            return Some(bytes[i] as char);
        } else {
            i += 1;
        }
    }
    None
}

impl Elements {
    /// The separator the container's elements use, if any (`,` or `;`).
    fn separator(&self, src: &str) -> Option<char> {
        let n = self.spans.len();
        (0..n).find_map(|i| {
            let gap_end = if i + 1 < n { self.spans[i + 1].start } else { self.inner.end };
            separator_char(&src[self.spans[i].end..gap_end])
        })
    }

    /// The separator written between elements put on one line.
    fn inline_separator(&self, src: &str) -> String {
        match (self.separator(src), self.kind) {
            (Some(c), _) => format!("{c} "),
            (None, ContainerKind::List) => ", ".to_string(),
            (None, _) => " ".to_string(),
        }
    }

    /// Where element `index` starts when it takes whole lines: the first
    /// line of the comment lines directly above it, else its own line.
    fn line_top(&self, src: &str, index: usize) -> usize {
        let mut top = line_start(src, self.spans[index].start);
        // The comments that open a file are its header, not the first statement's.
        if index == 0 && self.kind == ContainerKind::Block && self.inner.start == 0 {
            return top;
        }
        let floor = if index == 0 { self.inner.start } else { self.spans[index - 1].end };
        while top > 0 {
            let prev_start = line_start(src, top - 1);
            if prev_start < floor {
                break;
            }
            let line = src[prev_start..top].trim();
            if line.starts_with("//") {
                top = prev_start;
            } else {
                break;
            }
        }
        top
    }

    /// The lines element `index` owns (its attached comments, its own
    /// lines, the line break), when it starts and ends its lines.
    fn line_extent(&self, src: &str, index: usize) -> Option<Range<usize>> {
        let span = &self.spans[index];
        if !starts_line(src, span.start) {
            return None;
        }
        let limit = if index + 1 < self.spans.len() { self.spans[index + 1].start } else { self.inner.end };
        let le = ends_line(src, span.end, limit)?;
        let end = if le < src.len() { le + 1 } else { le };
        Some(self.line_top(src, index)..end)
    }
}

fn resolve_index(at: At, n: usize) -> Result<usize, EditError> {
    let index = match at {
        At::Start => 0,
        At::End => n,
        At::Before(i) => i,
        At::After(i) => i + 1,
        At::AfterHeader => 0,
    };
    if index > n {
        return err(format!("no position {index} (it holds {n})"));
    }
    Ok(index)
}

fn container(node: &Node, source_len: usize) -> Result<Elements, EditError> {
    elements(node, source_len).ok_or_else(|| EditError { message: format!("a {} holds no elements", node.kind_name()) })
}

/// The replacements that insert `text` as an element of `node` (a block,
/// object, array or call) at `at`. The layout follows the container's: a new
/// line (indented like its neighbour, with the separator the others use)
/// when the neighbour starts its own line, else on the same line.
pub fn insert_element_edits(src: &str, node: &Node, at: At, text: &str) -> Result<Vec<Replace>, EditError> {
    let el = container(node, src.len())?;
    let index = resolve_index(at, el.spans.len())?;
    Ok(insertion(src, &el, index, text, at == At::AfterHeader))
}

fn insertion(src: &str, el: &Elements, index: usize, text: &str, after_header: bool) -> Vec<Replace> {
    let n = el.spans.len();
    let sep = el.separator(src);
    if n == 0 {
        return vec![first_element(src, el, text)];
    }
    if index < n {
        let span = &el.spans[index];
        if starts_line(src, span.start) {
            let indent = line_indent(src, span.start);
            let top = el.line_top(src, index);
            let tail = line_separator(el, sep, &src[span.clone()]);
            let top = if after_header { line_start(src, span.start) } else { top };
            let mut edits = Vec::new();
            // A list element on its own line is separated by a comma from what follows
            // it; the element before the new one gets one too, or `-3` would subtract.
            if el.kind == ContainerKind::List && index > 0 && continues_a_value(text) {
                let prev = &el.spans[index - 1];
                if after_separator(src, prev.end) == prev.end {
                    edits.push(Replace::new(prev.end..prev.end, ","));
                }
            }
            edits.push(Replace::new(top..top, format!("{indent}{text}{tail}\n")));
            return edits;
        }
        let mut edits = Vec::new();
        // On one line too: `a -1` would subtract, so the element before gets its comma.
        if el.kind == ContainerKind::List && index > 0 && continues_a_value(text) {
            let prev = &el.spans[index - 1];
            if after_separator(src, prev.end) == prev.end {
                edits.push(Replace::new(prev.end..prev.end, ","));
            }
        }
        edits.push(Replace::new(span.start..span.start, format!("{text}{}", el.inline_separator(src))));
        return edits;
    }
    let last = &el.spans[n - 1];
    if starts_line(src, last.start) {
        let indent = line_indent(src, last.start);
        let after = after_separator(src, last.end);
        let had_sep = after > last.end;
        let tail_sep = if had_sep { sep.map(String::from).unwrap_or_default() } else { String::new() };
        let mut edits = Vec::new();
        if let (Some(c), false) = (line_separator(el, sep, text).chars().next(), had_sep) {
            edits.push(Replace::new(last.end..last.end, c.to_string()));
        }
        // After the line's trailing comment when the line ends there; else right after the element.
        let at = ends_line(src, last.end, el.inner.end).filter(|le| *le <= el.inner.end.max(after)).unwrap_or(after);
        let at = if at < after { after } else { at };
        edits.push(Replace::new(at..at, format!("\n{indent}{text}{tail_sep}")));
        return edits;
    }
    vec![Replace::new(last.end..last.end, format!("{}{text}", el.inline_separator(src)))]
}

/// Would `text`, written after a value inside brackets, continue that value
/// (`-3` subtracts, `{..}` applies, `[..]` indexes, `(..)` calls) instead of
/// being an element of its own? A new line does not end a value inside brackets.
fn continues_a_value(text: &str) -> bool {
    text.trim_start().chars().next().is_some_and(|c| "([{-+*/%<>=&|^.?:!~@".contains(c))
}

/// The separator an element put on its own line before `next` needs: the
/// container's own when it has one, else a comma only where `next` would
/// otherwise run into it (arrays and argument lists).
fn line_separator(el: &Elements, sep: Option<char>, next: &str) -> String {
    match sep {
        Some(c) => c.to_string(),
        None if el.kind == ContainerKind::List && continues_a_value(next) => ",".to_string(),
        None => String::new(),
    }
}

fn first_element(src: &str, el: &Elements, text: &str) -> Replace {
    let inner = &src[el.inner.clone()];
    if el.inner.start == 0 {
        // The file: one statement on its own line at the end.
        let lead = if src.is_empty() || src.ends_with('\n') { "" } else { "\n" };
        return Replace::new(src.len()..src.len(), format!("{lead}{text}\n"));
    }
    if let Some(last_break) = inner.rfind('\n') {
        // `{\n}`: the element gets a line of its own, one level in.
        let indent = line_indent(src, el.inner.start - 1);
        let at = el.inner.start + last_break;
        return Replace::new(at..at, format!("\n{indent}    {text}"));
    }
    let pad = if el.kind == ContainerKind::Block { " " } else { "" };
    Replace::new(el.inner.clone(), format!("{pad}{text}{pad}"))
}

/// The replacements that remove element `index` of `node`: its lines (with
/// the comment lines directly above it) when it owns them, else it and one
/// separator.
pub fn remove_element_edits(src: &str, node: &Node, index: usize) -> Result<Vec<Replace>, EditError> {
    let el = container(node, src.len())?;
    if index >= el.spans.len() {
        return err(format!("no element {index} (it holds {})", el.spans.len()));
    }
    Ok(vec![removal(src, &el, index)])
}

fn removal(src: &str, el: &Elements, index: usize) -> Replace {
    let n = el.spans.len();
    let span = &el.spans[index];
    if let Some(extent) = el.line_extent(src, index) {
        return Replace::new(extent, "");
    }
    let after = after_separator(src, span.end);
    if index + 1 < n {
        // Up to the next element, when only blanks lie between.
        let blanks_to = after + (src[after..].len() - src[after..].trim_start_matches([' ', '\t']).len());
        return Replace::new(span.start..blanks_to.min(el.spans[index + 1].start), "");
    }
    if index > 0 {
        return Replace::new(el.spans[index - 1].end..span.end, "");
    }
    Replace::new(span.start..after, "")
}

/// The replacements that insert the field `name: text` into the object `node`.
pub fn add_field_edits(src: &str, node: &Node, at: At, name: &str, text: &str) -> Result<Vec<Replace>, EditError> {
    let Some((_, fields)) = node.object() else { return err(format!("a {} has no fields", node.kind_name())) };
    if fields.iter().any(|f| f.name.as_deref() == Some(name)) {
        return err(format!("the object already has a field `{name}`"));
    }
    let key = super::write::key(name).map_err(|e| EditError { message: e })?;
    insert_element_edits(src, node, at, &format!("{key}: {text}"))
}

/// The replacements that remove field `name` of the object `node`.
pub fn remove_field_edits(src: &str, node: &Node, name: &str) -> Result<Vec<Replace>, EditError> {
    let Some((_, fields)) = node.object() else { return err(format!("a {} has no fields", node.kind_name())) };
    let Some(i) = fields.iter().position(|f| f.name.as_deref() == Some(name)) else { return err(format!("the object has no field `{name}`")) };
    remove_element_edits(src, node, i)
}

/// The replacements that move element `from` of `node` to position `to`
/// (an index among the elements after the move). Elements that own their
/// lines move with their comments; elements on a shared line trade places.
pub fn move_element_edits(src: &str, node: &Node, from: usize, to: usize) -> Result<Vec<Replace>, EditError> {
    let el = container(node, src.len())?;
    let n = el.spans.len();
    if from >= n || to >= n {
        return err(format!("no element {} (it holds {n})", from.max(to)));
    }
    if from == to {
        return Ok(Vec::new());
    }
    let own_lines = (0..n).all(|i| el.line_extent(src, i).is_some());
    let mut order: Vec<usize> = (0..n).collect();
    let moved = order.remove(from);
    order.insert(to, moved);
    if own_lines {
        let extents: Vec<Range<usize>> = (0..n).map(|i| el.line_extent(src, i).unwrap()).collect();
        let (start, end) = (extents[0].start, extents[n - 1].end);
        let mut joined = String::new();
        for (slot, &i) in order.iter().enumerate() {
            let text = &src[extents[i].clone()];
            // In a list an element that is no longer last needs its comma (a new line
            // does not end a value inside brackets).
            let needs_comma = el.kind == ContainerKind::List && slot + 1 < n && after_separator(src, el.spans[i].end) == el.spans[i].end && continues_a_value(&src[el.spans[order[slot + 1]].clone()]);
            if needs_comma {
                let cut = el.spans[i].end - extents[i].start;
                joined.push_str(&text[..cut]);
                joined.push(',');
                joined.push_str(&text[cut..]);
            } else {
                joined.push_str(text);
            }
            if slot + 1 < n {
                if !text.ends_with('\n') {
                    joined.push('\n');
                }
                // What lies between the slots (blank lines) stays where it is.
                joined.push_str(&src[extents[slot].end..extents[slot + 1].start]);
            }
        }
        return Ok(vec![Replace::new(start..end, joined)]);
    }
    // One line (or mixed): each element's own text into the slots, separators kept.
    Ok(order.iter().enumerate().filter(|(slot, i)| slot != *i).map(|(slot, &i)| Replace::new(el.spans[slot].clone(), src[el.spans[i].clone()].to_string())).collect())
}

/// The replacements that put a copy of element `index` of `node` right after
/// it, `transform` applied to the copy's text.
pub fn duplicate_element_edits(src: &str, node: &Node, index: usize, transform: &dyn Fn(&str) -> Result<String, EditError>) -> Result<Vec<Replace>, EditError> {
    let el = container(node, src.len())?;
    if index >= el.spans.len() {
        return err(format!("no element {index} (it holds {})", el.spans.len()));
    }
    let copy = transform(&src[el.spans[index].clone()])?;
    let mut edits = insertion(src, &el, index + 1, &copy, false);
    if index + 1 == el.spans.len() {
        return Ok(edits);
    }
    // Between two elements: after the element, on its layout.
    edits.clear();
    let span = &el.spans[index];
    if starts_line(src, span.start) {
        let indent = line_indent(src, span.start);
        let sep = el.separator(src).map(String::from).unwrap_or_default();
        let at = el.line_extent(src, index).map_or(span.end, |e| e.end);
        if at > span.end && src[..at].ends_with('\n') {
            edits.push(Replace::new(at..at, format!("{indent}{copy}{sep}\n")));
        } else {
            edits.push(Replace::new(span.end..span.end, format!("\n{indent}{copy}{sep}")));
        }
    } else {
        edits.push(Replace::new(span.end..span.end, format!("{}{copy}", el.inline_separator(src))));
    }
    Ok(edits)
}

fn first_difference(a: &Shape, b: &Shape) -> String {
    if a.label != b.label || a.names != b.names {
        return format!("expected {} {:?}, found {} {:?}", a.label, a.names, b.label, b.names);
    }
    if a.kids.len() != b.kids.len() {
        return format!("in {}: expected {} parts, found {}", a.label, a.kids.len(), b.kids.len());
    }
    for (x, y) in a.kids.iter().zip(&b.kids) {
        if x != y {
            return first_difference(x, y);
        }
    }
    "none".to_string()
}

impl Syntax {
    /// Splice `edits` (non-overlapping ranges of the current source) and parse
    /// the result. No structural check: use the typed ops below for that.
    pub fn apply(&self, edits: &[Replace]) -> Result<Syntax, EditError> {
        let mut sorted: Vec<&Replace> = edits.iter().collect();
        sorted.sort_by_key(|e| (e.range.start, e.range.end));
        let mut out = String::with_capacity(self.source.len());
        let mut at = 0;
        for e in sorted {
            if e.range.start < at || e.range.end > self.source.len() || e.range.start > e.range.end {
                return err("edits overlap or lie outside the text");
            }
            out.push_str(&self.source[at..e.range.start]);
            out.push_str(&e.text);
            at = e.range.end;
        }
        out.push_str(&self.source[at..]);
        parse(&out).map_err(|e| EditError { message: format!("the edit leaves text that does not parse: {} (line {})", e.message, super::line_col(&out, e.at).0) })
    }

    /// [`apply`](Self::apply) for edits that put `text` over whole nodes:
    /// checked like [`replace`](Self::replace) for every node at once.
    pub fn replace_nodes(&self, replacements: &[(Loc, String)]) -> Result<Syntax, EditError> {
        let mut expected = self.old_shape();
        let mut edits = Vec::new();
        for (loc, text) in replacements {
            let node = self.require(loc)?;
            let new = parse_value(text).map_err(|e| EditError { message: format!("the replacement does not parse: {e}") })?;
            *expected.at_mut(&loc.0) = shape(&new, text);
            edits.push(Replace::new(node.span.clone(), text.clone()));
        }
        self.commit(&edits, expected, Loc::default()).map(|e| e.syntax)
    }

    fn require(&self, loc: &Loc) -> Result<&Node, EditError> {
        self.node(loc).ok_or_else(|| EditError { message: format!("no node at {:?}", loc.0) })
    }

    /// Splice, parse and check the new tree is `expected`.
    fn commit(&self, edits: &[Replace], expected: Shape, loc: Loc) -> Result<Edited, EditError> {
        let syntax = self.apply(edits)?;
        let got = shape(&syntax.root, &syntax.source);
        if got != expected {
            return err(format!("the edit does not leave the intended structure (first difference: {})", first_difference(&expected, &got)));
        }
        Ok(Edited { syntax, loc })
    }

    fn old_shape(&self) -> Shape {
        shape(&self.root, &self.source)
    }

    /// The address of `node`, a node of this tree.
    pub fn loc_of(&self, node: &Node) -> Option<Loc> {
        let mut found = None;
        self.walk(&mut |loc, n| {
            if found.is_none() && std::ptr::eq(n, node) {
                found = Some(loc.clone());
            }
        });
        found
    }

    /// Replace the node at `loc` with `text`, which must parse as one node
    /// that keeps its place in the surrounding structure.
    pub fn replace(&self, loc: &Loc, text: &str) -> Result<Edited, EditError> {
        let node = self.require(loc)?;
        let new = parse_value(text).map_err(|e| EditError { message: format!("the replacement does not parse: {e}") })?;
        let mut expected = self.old_shape();
        *expected.at_mut(&loc.0) = shape(&new, text);
        self.commit(&[Replace::new(node.span.clone(), text)], expected, loc.clone())
    }

    /// Insert `text` as one element of the list, argument list, block or
    /// object at `list` (an object takes a bare entry).
    pub fn insert_element(&self, list: &Loc, at: At, text: &str) -> Result<Edited, EditError> {
        let container_node = self.require(list)?;
        let el = container(container_node, self.source.len())?;
        let new = parse_value(text).map_err(|e| EditError { message: format!("the new element does not parse: {e}") })?;
        let index = resolve_index(at, el.spans.len())?;
        let edits = insertion(&self.source, &el, index, text, at == At::AfterHeader);
        let mut expected = self.old_shape();
        let target = expected.at_mut(&list.0);
        target.kids.insert(el.offset + index, shape(&new, text));
        if el.fields {
            target.names.insert(index, String::new());
        }
        self.commit(&edits, expected, list.child(el.offset + index))
    }

    /// Insert a statement into the block at `block` (use the empty [`Loc`] for the file).
    pub fn insert_statement(&self, block: &Loc, at: At, text: &str) -> Result<Edited, EditError> {
        match self.require(block)?.kind {
            Kind::Block(_) => self.insert_element(block, at, text),
            _ => err("insert_statement needs a block"),
        }
    }

    /// Remove the element at `index` of the list, block or object at `list`.
    pub fn remove_element(&self, list: &Loc, index: usize) -> Result<Edited, EditError> {
        let container_node = self.require(list)?;
        let el = container(container_node, self.source.len())?;
        let edits = remove_element_edits(&self.source, container_node, index)?;
        let mut expected = self.old_shape();
        let target = expected.at_mut(&list.0);
        target.kids.remove(el.offset + index);
        if el.fields {
            target.names.remove(index);
        }
        self.commit(&edits, expected, list.clone())
    }

    /// Delete the statement at `loc`, with its line when it is alone on it.
    pub fn delete_statement(&self, loc: &Loc) -> Result<Edited, EditError> {
        let Some(parent) = loc.parent() else { return err("the file itself is not a statement") };
        let index = *loc.0.last().unwrap();
        match self.require(&parent)?.kind {
            Kind::Block(_) => self.remove_element(&parent, index),
            _ => err("delete_statement needs a statement of a block"),
        }
    }

    /// Set the value of field `name` of the object at `object` to `text`, or
    /// add the field at the end when it is not there.
    pub fn set_field(&self, object: &Loc, name: &str, text: &str) -> Result<Edited, EditError> {
        let node = self.require(object)?;
        let Some((_, fields)) = node.object() else { return err(format!("a {} has no fields", node.kind_name())) };
        match fields.iter().position(|f| f.name.as_deref() == Some(name)) {
            Some(i) => self.replace(&object.child(i), text),
            None => self.add_field(object, At::End, name, text),
        }
    }

    /// Add `name: text` to the object at `object`.
    pub fn add_field(&self, object: &Loc, at: At, name: &str, text: &str) -> Result<Edited, EditError> {
        let node = self.require(object)?;
        let edits = add_field_edits(&self.source, node, at, name, text)?;
        let el = container(node, self.source.len())?;
        let key = super::write::key(name).map_err(|e| EditError { message: e })?;
        let wrapped = format!("{{{key}: {text}}}");
        let new = parse_value(&wrapped).map_err(|e| EditError { message: format!("the field does not parse: {e}") })?;
        let Some(new_field) = new.field(name) else { return err("the field does not parse as one field") };
        if new.object().map_or(0, |(_, f)| f.len()) != 1 {
            return err("the field does not parse as one field");
        }
        let index = resolve_index(at, el.spans.len())?;
        let mut expected = self.old_shape();
        let target = expected.at_mut(&object.0);
        target.kids.insert(index, shape(&new_field.value, &wrapped));
        target.names.insert(index, format!("{name}:"));
        self.commit(&edits, expected, object.child(index))
    }

    /// Remove field `name` of the object at `object`.
    pub fn remove_field(&self, object: &Loc, name: &str) -> Result<Edited, EditError> {
        let node = self.require(object)?;
        let Some((_, fields)) = node.object() else { return err(format!("a {} has no fields", node.kind_name())) };
        let Some(i) = fields.iter().position(|f| f.name.as_deref() == Some(name)) else { return err(format!("the object has no field `{name}`")) };
        self.remove_element(object, i)
    }

    /// Append `text` to the array at `array`.
    pub fn push_item(&self, array: &Loc, text: &str) -> Result<Edited, EditError> {
        self.insert_element(array, At::End, text)
    }

    /// Move element `from` of the list or block at `list` to position `to`.
    pub fn move_element(&self, list: &Loc, from: usize, to: usize) -> Result<Edited, EditError> {
        let container_node = self.require(list)?;
        let el = container(container_node, self.source.len())?;
        let edits = move_element_edits(&self.source, container_node, from, to)?;
        let mut expected = self.old_shape();
        let target = expected.at_mut(&list.0);
        let moved = target.kids.remove(el.offset + from);
        target.kids.insert(el.offset + to, moved);
        if el.fields {
            let name = target.names.remove(from);
            target.names.insert(to, name);
        }
        self.commit(&edits, expected, list.clone())
    }
}
