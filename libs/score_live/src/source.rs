//! The score's source as text: a small lexer, the outline (every
//! `let name = Type{…}` node, its fields and their byte ranges, the final
//! `Score{…}`), and source-preserving patches. A patch replaces exactly the
//! bytes it is about; every comment, blank line and layout elsewhere stays.
//! Editors, the graph view, the widgets and the AI all edit through these.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tok {
    Ident,
    /// `@name`.
    Id,
    Num,
    Str,
    Punct,
    Comment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: Tok,
    pub start: usize,
    pub end: usize,
}

/// Every token but whitespace, comments included.
pub fn lex(src: &str) -> Vec<Token> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        let kind = if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            Tok::Comment
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(b.len());
            Tok::Comment
        } else if c == b'"' || c == b'\'' {
            i += 1;
            while i < b.len() && b[i] != c {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(b.len());
            Tok::Str
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.' || b[i] == b'_') {
                // `0..4` is a range, not a number with two dots.
                if b[i] == b'.' && b.get(i + 1) == Some(&b'.') {
                    break;
                }
                i += 1;
            }
            Tok::Num
        } else if c == b'@' {
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'#') {
                i += 1;
            }
            Tok::Id
        } else if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] >= 0x80) {
                i += 1;
            }
            Tok::Ident
        } else {
            // Two-character operators stay one token.
            let two = [b"==", b"!=", b"<=", b">=", b"&&", b"||", b"..", b"+=", b"-=", b"*=", b"/="];
            i += if b.get(i + 1).is_some_and(|n| two.iter().any(|t| t[0] == c && t[1] == *n)) { 2 } else { 1 };
            Tok::Punct
        };
        out.push(Token { kind, start, end: i });
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start..self.end]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub key: String,
    pub key_span: Span,
    /// The value's bytes; for a bare name (`Score{kick bass}`) the name.
    pub value: Span,
    pub bare: bool,
}

/// One `let name = Type{…}` node or the final `Score{…}`.
#[derive(Clone, Debug, PartialEq)]
pub struct Ctor {
    pub name: String,
    pub type_name: String,
    /// The whole statement, `let` to the closing brace.
    pub span: Span,
    /// `{` to `}` inclusive.
    pub body: Span,
    pub fields: Vec<Field>,
}

impl Ctor {
    pub fn field(&self, key: &str) -> Option<&Field> {
        self.fields.iter().find(|field| field.key == key && !field.bare)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outline {
    pub nodes: Vec<Ctor>,
    pub root: Option<Ctor>,
    pub tokens: Vec<Token>,
}

impl Outline {
    pub fn node(&self, name: &str) -> Option<&Ctor> {
        self.nodes.iter().find(|node| node.name == name)
    }
}

fn is(src: &str, token: Option<&Token>, text: &str) -> bool {
    token.is_some_and(|t| t.kind != Tok::Comment && t.kind != Tok::Str && &src[t.start..t.end] == text)
}

/// Index of the token closing the bracket opened at `open`.
fn close_of(src: &str, toks: &[Token], open: usize) -> usize {
    let (o, c) = match &src[toks[open].start..toks[open].end] {
        "{" => ("{", "}"),
        "[" => ("[", "]"),
        _ => ("(", ")"),
    };
    let mut depth = 0;
    for (i, t) in toks.iter().enumerate().skip(open) {
        if t.kind != Tok::Punct {
            continue;
        }
        let s = &src[t.start..t.end];
        if s == o {
            depth += 1;
        } else if s == c {
            depth -= 1;
            if depth == 0 {
                return i;
            }
        }
    }
    toks.len() - 1
}

const BINARY: [&str; 16] = ["+", "-", "*", "/", "%", "<", ">", "==", "!=", "<=", ">=", "&&", "||", "..", "|", "&"];

/// Skip one expression from `i`; the index after it.
fn skip_expr(src: &str, toks: &[Token], mut i: usize) -> usize {
    let n = toks.len();
    let comment = |i: usize| toks.get(i).is_some_and(|t| t.kind == Tok::Comment);
    loop {
        while comment(i) {
            i += 1;
        }
        if i >= n {
            return n;
        }
        // Prefix operators.
        while is(src, toks.get(i), "-") || is(src, toks.get(i), "!") {
            i += 1;
        }
        if i >= n {
            return n;
        }
        let t = toks[i];
        let text = &src[t.start..t.end];
        i = match t.kind {
            Tok::Punct if text == "(" || text == "[" || text == "{" => close_of(src, toks, i) + 1,
            Tok::Ident if text == "fn" => {
                let mut j = i + 1;
                if is(src, toks.get(j), "(") {
                    j = close_of(src, toks, j) + 1;
                }
                if is(src, toks.get(j), "{") {
                    j = close_of(src, toks, j) + 1;
                }
                j
            }
            Tok::Punct => return i,
            _ => i + 1,
        };
        // Postfix: `Type{…}`, calls, fields, indexes.
        loop {
            if is(src, toks.get(i), "{") || is(src, toks.get(i), "(") || is(src, toks.get(i), "[") {
                // `{` after a value on a new line would be a new statement;
                // a constructor's brace follows its type on the same line.
                i = close_of(src, toks, i) + 1;
            } else if is(src, toks.get(i), ".") && toks.get(i + 1).is_some_and(|t| t.kind == Tok::Ident) {
                i += 2;
            } else {
                break;
            }
        }
        // A binary operator continues the expression; `key:` does not.
        let op = toks.get(i).filter(|t| t.kind == Tok::Punct).map(|t| &src[t.start..t.end]);
        match op {
            Some(op) if BINARY.contains(&op) => {
                i += 1;
                continue;
            }
            _ => return i,
        }
    }
}

/// The fields of the object body whose `{` is token `open`.
fn fields(src: &str, toks: &[Token], open: usize, close: usize) -> Vec<Field> {
    let mut out = Vec::new();
    let mut i = open + 1;
    while i < close {
        let t = toks[i];
        if t.kind == Tok::Comment || is(src, Some(&t), ",") {
            i += 1;
            continue;
        }
        if t.kind == Tok::Ident && is(src, toks.get(i + 1), ":") {
            let start = i + 2;
            let end = skip_expr(src, toks, start).min(close).max(start + 1);
            // An object left open at the end of the source closes at the
            // last token.
            let last = (end - 1).min(toks.len() - 1);
            out.push(Field {
                key: src[t.start..t.end].to_string(),
                key_span: Span { start: t.start, end: t.end },
                value: Span { start: toks[start.min(close).min(last)].start, end: toks[last].end },
                bare: false,
            });
            i = end;
        } else if t.kind == Tok::Ident && !is(src, toks.get(i + 1), "{") && !is(src, toks.get(i + 1), "(") && !is(src, toks.get(i + 1), ".") {
            let span = Span { start: t.start, end: t.end };
            out.push(Field { key: src[t.start..t.end].to_string(), key_span: span, value: span, bare: true });
            i += 1;
        } else {
            i = skip_expr(src, toks, i).max(i + 1);
        }
    }
    out
}

/// The outline of a score's source. Anything it does not understand is
/// skipped: the loader is the judge of validity, this only finds places.
pub fn outline(src: &str) -> Outline {
    let toks = lex(src);
    let mut out = Outline::default();
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        if t.kind == Tok::Comment {
            i += 1;
            continue;
        }
        if is(src, Some(&t), "use") {
            // `use mod.x.*` to the end of its line.
            let line_end = src[t.start..].find('\n').map_or(src.len(), |e| t.start + e);
            while i < toks.len() && toks[i].start < line_end {
                i += 1;
            }
            continue;
        }
        if is(src, Some(&t), "let") && toks.get(i + 1).is_some_and(|n| n.kind == Tok::Ident) && is(src, toks.get(i + 2), "=") {
            let name = &src[toks[i + 1].start..toks[i + 1].end];
            let value = i + 3;
            let end = skip_expr(src, &toks, value);
            if toks.get(value).is_some_and(|v| v.kind == Tok::Ident) && is(src, toks.get(value + 1), "{") {
                let close = close_of(src, &toks, value + 1);
                out.nodes.push(Ctor {
                    name: name.to_string(),
                    type_name: src[toks[value].start..toks[value].end].to_string(),
                    span: Span { start: t.start, end: toks[close].end },
                    body: Span { start: toks[value + 1].start, end: toks[close].end },
                    fields: fields(src, &toks, value + 1, close),
                });
            }
            i = end.max(i + 1);
            continue;
        }
        if t.kind == Tok::Ident && is(src, toks.get(i + 1), "{") {
            let close = close_of(src, &toks, i + 1);
            let type_name = &src[t.start..t.end];
            if type_name == "Score" {
                out.root = Some(Ctor {
                    name: String::new(),
                    type_name: type_name.to_string(),
                    span: Span { start: t.start, end: toks[close].end },
                    body: Span { start: toks[i + 1].start, end: toks[close].end },
                    fields: fields(src, &toks, i + 1, close),
                });
            }
            i = close + 1;
            continue;
        }
        i = skip_expr(src, &toks, i).max(i + 1);
    }
    out.tokens = toks;
    out
}

/// 1-based line and column of a byte offset.
pub fn line_col(src: &str, at: usize) -> (usize, usize) {
    let at = at.min(src.len());
    let before = &src[..at];
    let line = before.matches('\n').count() + 1;
    let col = before.rfind('\n').map_or(at, |nl| at - nl - 1) + 1;
    (line, col)
}

/// The byte offset of a 1-based line and column.
pub fn offset_of(src: &str, line: usize, col: usize) -> usize {
    let mut at = 0;
    for _ in 1..line.max(1) {
        match src[at..].find('\n') {
            Some(nl) => at += nl + 1,
            None => return src.len(),
        }
    }
    (at + col.max(1) - 1).min(src.len())
}

/// One text change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Splice {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

pub fn apply(src: &str, splice: &Splice) -> String {
    let mut out = String::with_capacity(src.len() + splice.text.len());
    out.push_str(&src[..splice.start]);
    out.push_str(&splice.text);
    out.push_str(&src[splice.end..]);
    out
}

fn find_ctor<'a>(outline: &'a Outline, node: &str) -> Result<&'a Ctor, String> {
    if node.is_empty() || node == "Score" {
        return outline.root.as_ref().ok_or_else(|| "the score has no final `Score{…}`".to_string());
    }
    outline.node(node).ok_or_else(|| {
        let names: Vec<_> = outline.nodes.iter().map(|node| node.name.as_str()).collect();
        format!("no node `{node}` (nodes: {})", names.join(", "))
    })
}

/// The indentation of the line holding `at`.
fn indent_at(src: &str, at: usize) -> &str {
    let line_start = src[..at].rfind('\n').map_or(0, |nl| nl + 1);
    let rest = &src[line_start..];
    &rest[..rest.len() - rest.trim_start_matches([' ', '\t']).len()]
}

/// Set `key: value` on a node (`""` or `"Score"` is the root). An existing
/// value is replaced in place; a new field goes last, matching the node's
/// layout (one line stays one line).
pub fn set_field(src: &str, node: &str, key: &str, value: &str) -> Result<Splice, String> {
    let outline = outline(src);
    let ctor = find_ctor(&outline, node)?;
    if let Some(field) = ctor.field(key) {
        return Ok(Splice { start: field.value.start, end: field.value.end, text: value.to_string() });
    }
    let close = ctor.body.end - 1;
    let body = &src[ctor.body.start..ctor.body.end];
    if body.contains('\n') {
        // After the last field, on its own line, at the fields' indent.
        let last_end = ctor.fields.last().map_or(ctor.body.start + 1, |f| f.value.end);
        let indent = ctor.fields.last().map_or_else(|| format!("{}    ", indent_at(src, ctor.span.start)), |f| indent_at(src, f.key_span.start).to_string());
        Ok(Splice { start: last_end, end: last_end, text: format!("\n{indent}{key}: {value}") })
    } else {
        let before = src[..close].trim_end_matches(' ').len();
        let lead = if src[ctor.body.start + 1..before].trim().is_empty() { "" } else { " " };
        Ok(Splice { start: before, end: close, text: format!("{lead}{key}: {value}") })
    }
}

/// Remove a field and the space before it.
pub fn remove_field(src: &str, node: &str, key: &str) -> Result<Splice, String> {
    let outline = outline(src);
    let ctor = find_ctor(&outline, node)?;
    let field = ctor.fields.iter().find(|field| field.key == key).ok_or_else(|| format!("`{node}` has no `{key}`"))?;
    let start = src[..field.key_span.start].trim_end_matches([' ', '\t']).len();
    let start = if src[start..field.key_span.start].is_empty() { field.key_span.start } else { start };
    // Keep the line break if the field was alone on its line.
    let start = if src[..start].ends_with('\n') && src[field.value.end..].starts_with('\n') { start - 1 } else { start };
    Ok(Splice { start, end: field.value.end, text: String::new() })
}

/// Add `let name = text` before the final `Score{…}` and list the name in
/// it, as one change.
pub fn insert_node(src: &str, name: &str, text: &str) -> Result<String, String> {
    let outline = outline(src);
    if outline.node(name).is_some() {
        return Err(format!("a node `{name}` exists already"));
    }
    if !name.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c == '_') || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("`{name}` is not a node name: lower_snake_case"));
    }
    let root = outline.root.as_ref().ok_or("the score has no final `Score{…}`")?;
    // Into the root's member list first (it is later in the text).
    let listed = add_member(src, root, name);
    let at = src[..root.span.start].rfind('\n').map_or(0, |nl| nl + 1);
    let at = src[..at].trim_end().len();
    let at = if at == 0 { 0 } else { src[at..].find('\n').map_or(at, |nl| at + nl + 1) };
    let prefix = if at == 0 || src[..at].ends_with("\n\n") { "" } else { "\n" };
    Ok(format!("{}{prefix}let {name} = {text}\n\n{}", &listed[..at], listed[at..].trim_start_matches('\n')))
}

fn add_member(src: &str, root: &Ctor, name: &str) -> String {
    let last_bare = root.fields.iter().rev().find(|field| field.bare);
    let splice = match last_bare {
        Some(field) => Splice { start: field.value.end, end: field.value.end, text: format!(" {name}") },
        None => {
            let close = root.body.end - 1;
            let before = src[..close].trim_end().len();
            Splice { start: before, end: before, text: format!(" {name}") }
        }
    };
    apply(src, &splice)
}

/// Remove a node, its root listing and every bare reference to it in other
/// nodes' lists (`in: [a b]`, `loops: [a b]`). A field whose whole value is
/// the node (`play: kit`) is refused: the caller decides what replaces it.
pub fn remove_node(src: &str, name: &str) -> Result<String, String> {
    let outline = outline(src);
    let ctor = outline.node(name).ok_or_else(|| format!("no node `{name}`"))?;
    for other in &outline.nodes {
        for field in other.fields.iter().filter(|f| !f.bare) {
            if field.value.text(src).trim() == name {
                return Err(format!("`{}.{}` is `{name}`: change it first", other.name, field.key));
            }
        }
    }
    let mut splices = Vec::new();
    // The statement with its trailing newline (and a blank line after).
    let start = src[..ctor.span.start].rfind('\n').map_or(0, |nl| nl + 1);
    // Doc comments directly above go with it.
    let mut start = start;
    loop {
        let prev_end = start.saturating_sub(1);
        let prev_start = src[..prev_end].rfind('\n').map_or(0, |nl| nl + 1);
        if start > 0 && src[prev_start..prev_end].trim_start().starts_with("//") {
            start = prev_start;
        } else {
            break;
        }
    }
    let mut end = ctor.span.end;
    if src[end..].starts_with('\n') {
        end += 1;
    }
    if src[end..].starts_with('\n') && (start == 0 || src[..start].ends_with("\n\n")) {
        end += 1;
    }
    splices.push(Splice { start, end, text: String::new() });
    // Bare references in lists and the root.
    for token in &outline.tokens {
        if token.kind == Tok::Ident && &src[token.start..token.end] == name && (token.start < ctor.span.start || token.start >= ctor.span.end) {
            let inside_list = outline.nodes.iter().chain(outline.root.iter()).any(|node| {
                node.fields.iter().any(|f| (f.bare && f.value.start == token.start) || (!f.bare && f.value.start < token.start && token.end <= f.value.end && f.value.text(src).trim_start().starts_with('[')))
            });
            if inside_list {
                let s = src[..token.start].trim_end_matches(' ').len();
                splices.push(Splice { start: s, end: token.end, text: String::new() });
            }
        }
    }
    splices.sort_by_key(|s| std::cmp::Reverse(s.start));
    let mut out = src.to_string();
    for splice in &splices {
        out = apply(&out, splice);
    }
    Ok(out)
}

/// The numeric literal token at `at`, if any (drag-to-scrub).
pub fn number_at(src: &str, at: usize) -> Option<Span> {
    lex(src).into_iter().find(|t| t.kind == Tok::Num && t.start <= at && at <= t.end).map(|t| {
        // A leading minus belongs to the number.
        let start = if src[..t.start].ends_with('-') && !src[..t.start - 1].trim_end().ends_with(|c: char| c.is_ascii_alphanumeric() || c == ')' || c == ']') { t.start - 1 } else { t.start };
        Span { start, end: t.end }
    })
}

/// Format a number the way a person would type it at this precision.
pub fn fmt_num(value: f64, decimals: usize) -> String {
    let s = format!("{value:.decimals$}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    if s == "-0" { "0".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "// Test.\nuse mod.score.*\n\nlet clock = Clock{bpm: 120 sig: [4 4]}\n\n/// The kit.\nlet kit = Drums{kit: @analog_hybrid}\nlet kick = Loop{\n    play: kit\n    beats: 4\n    code: fn(i) { hit(@kick) sleep(1) }\n}\nlet verb = Reverb{in: [kick] mix: 0.2 * 1.5}\n\nScore{label: \"T\" clock kit kick verb}\n";

    #[test]
    fn outline_finds_nodes_fields_and_the_root() {
        let o = outline(SRC);
        let names: Vec<_> = o.nodes.iter().map(|n| (n.name.as_str(), n.type_name.as_str())).collect();
        assert_eq!(names, [("clock", "Clock"), ("kit", "Drums"), ("kick", "Loop"), ("verb", "Reverb")]);
        let kick = o.node("kick").unwrap();
        assert_eq!(kick.field("code").unwrap().value.text(SRC), "fn(i) { hit(@kick) sleep(1) }");
        assert_eq!(o.node("verb").unwrap().field("mix").unwrap().value.text(SRC), "0.2 * 1.5");
        assert_eq!(o.node("clock").unwrap().field("sig").unwrap().value.text(SRC), "[4 4]");
        let root = o.root.unwrap();
        assert_eq!(root.fields.iter().filter(|f| f.bare).map(|f| f.key.as_str()).collect::<Vec<_>>(), ["clock", "kit", "kick", "verb"]);
    }

    #[test]
    fn patches_touch_only_their_bytes() {
        let s = apply(SRC, &set_field(SRC, "clock", "bpm", "128").unwrap());
        assert_eq!(s, SRC.replace("bpm: 120", "bpm: 128"));
        let s = apply(SRC, &set_field(SRC, "clock", "swing", "0.1").unwrap());
        assert!(s.contains("Clock{bpm: 120 sig: [4 4] swing: 0.1}"));
        let s = apply(SRC, &set_field(SRC, "kick", "mute", "true").unwrap());
        assert!(s.contains("    code: fn(i) { hit(@kick) sleep(1) }\n    mute: true\n}"));
        let s = apply(SRC, &remove_field(SRC, "clock", "sig").unwrap());
        assert!(s.contains("Clock{bpm: 120}"), "{s}");
        let s = insert_node(SRC, "hats", "Loop{play: kit hits: \"..x.\"}").unwrap();
        assert!(s.contains("let hats = Loop{play: kit hits: \"..x.\"}\n\nScore{"), "{s}");
        assert!(s.contains("kick verb hats}"));
        let s = remove_node(&s, "verb").unwrap();
        assert!(!s.contains("verb") && s.contains("Score{label: \"T\" clock kit kick hats}"), "{s}");
        assert!(remove_node(SRC, "kit").is_err());
        let s = remove_node(SRC, "kick").unwrap();
        assert!(s.contains("Reverb{in: [] mix") && !s.contains("kick "), "{s}");
        assert!(!s.contains("/// The kit.\nlet kit") || s.contains("let kit"));
    }

    #[test]
    fn numbers_and_positions() {
        let at = SRC.find("0.2").unwrap();
        assert_eq!(number_at(SRC, at + 1).unwrap().text(SRC), "0.2");
        let (line, col) = line_col(SRC, at);
        assert_eq!(offset_of(SRC, line, col), at);
        assert_eq!(fmt_num(0.30000001, 2), "0.3");
        assert_eq!(fmt_num(-0.001, 1), "0");
    }
}

/// A short content key of a source (what a render was made from).
pub fn fnv_hex(src: &str) -> String {
    format!("{:016x}", crate::key(src))
}
