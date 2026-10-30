//! A score as the document the editor panel edits (`makepad_editor_panel`'s
//! [`PanelDoc`], its values a [`ValueSource`]): its paths are the score's own names, `node.field`
//! (`mono.cutoff`, `bass.amp`, `echo.mix`) and a field of a record value
//! one level down (`lead.env.attack`); its outline is the score's
//! (`makepad_score_live::source`) plus every top-level `let` (the panel's
//! own `let editor_panel = fn() { … }` among them). A score writes no
//! colours and no keyed values: those parts of the trait say so.

use makepad_edit_values::{Splice, Value, ValueKind, ValueSite, ValueSource};
use makepad_editor_panel::{self as panel, File, PanelDoc};
use makepad_score_live::source::{self, Outline, Tok};
use std::any::Any;
use std::ops::Range;

/// The outline of a score: the nodes, and every top-level `let` by name.
pub struct ScoreCode {
    pub outline: Outline,
    pub lets: Vec<(String, Range<usize>)>,
}

impl ScoreCode {
    pub fn of(text: &str) -> ScoreCode {
        let outline = source::outline(text);
        let lets = top_lets(text, &outline);
        ScoreCode { outline, lets }
    }
}

fn is(text: &str, t: &source::Token, word: &str) -> bool {
    &text[t.start..t.end] == word
}

/// Every top-level `let <name> = …`: from `let` to its last token before
/// the next top-level statement (another `let`, a `use`, the final `Score{`).
fn top_lets(text: &str, outline: &Outline) -> Vec<(String, Range<usize>)> {
    let toks = &outline.tokens;
    let root = outline.root.as_ref().map(|r| r.span.start);
    let mut starts = Vec::new();
    let mut depth = 0i32;
    for (i, t) in toks.iter().enumerate() {
        if t.kind == Tok::Punct {
            match &text[t.start..t.end] {
                "{" | "(" | "[" => depth += 1,
                "}" | ")" | "]" => depth -= 1,
                _ => {}
            }
            continue;
        }
        if depth == 0 && (is(text, t, "let") || is(text, t, "use") || root == Some(t.start)) {
            starts.push(i);
        }
    }
    let mut out = Vec::new();
    for (k, &i) in starts.iter().enumerate() {
        let t = toks[i];
        if !is(text, &t, "let") {
            continue;
        }
        let Some(name) = toks.get(i + 1).filter(|n| n.kind == Tok::Ident) else { continue };
        let next = starts.get(k + 1).copied().unwrap_or(toks.len());
        let last = toks[i..next].iter().rev().find(|t| t.kind != Tok::Comment).map_or(t.end, |t| t.end);
        out.push((text[name.start..name.end].to_string(), t.start..last));
    }
    out
}

fn code_of(code: &dyn Any) -> &ScoreCode {
    code.downcast_ref::<ScoreCode>().expect("a score's outline is a ScoreCode")
}

/// The document itself among the files given.
fn own<'a>(files: &[File<'a>]) -> Option<(usize, &'a str, &'a ScoreCode)> {
    files.iter().enumerate().find(|(_, f)| f.name.is_empty()).or_else(|| files.iter().enumerate().next()).map(|(i, f)| (i, f.text, code_of(f.code)))
}

/// What a value is, as written.
fn kind_of(text: &str) -> ValueKind {
    let t = text.trim();
    let toks = source::lex(t);
    let single = toks.len() == 1 || (toks.len() == 2 && &t[toks[0].start..toks[0].end] == "-" && toks[1].kind == Tok::Num);
    let last = toks.last();
    match (single, last.map(|l| l.kind)) {
        _ if t == "true" || t == "false" => ValueKind::Bool,
        _ if t.starts_with('#') => ValueKind::Colour,
        _ if t.starts_with('[') => ValueKind::List,
        _ if t.starts_with('{') || (toks.len() >= 2 && toks[0].kind == Tok::Ident && &t[toks[1].start..toks[1].end] == "{") && t.ends_with('}') => ValueKind::Record,
        (true, Some(Tok::Num)) => {
            let unit = t.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-');
            match unit {
                "" => ValueKind::Number,
                "s" | "ms" => ValueKind::Time,
                "deg" | "rad" => ValueKind::Angle,
                "px" => ValueKind::Length,
                _ => ValueKind::Code,
            }
        }
        (true, Some(Tok::Id)) => ValueKind::Choice,
        (true, Some(Tok::Str)) => ValueKind::Text,
        (true, Some(Tok::Ident)) => ValueKind::Function,
        _ => ValueKind::Code,
    }
}

/// The fields of a record value (`{a: 1 b: @x}` or `Type{…}`) at `range`:
/// (key, value range).
fn record_fields(text: &str, range: Range<usize>) -> Vec<(String, Range<usize>)> {
    let body = &text[range.clone()];
    let toks = source::lex(body);
    let Some(open) = toks.iter().position(|t| &body[t.start..t.end] == "{") else { return Vec::new() };
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut i = open;
    while i < toks.len() {
        let t = toks[i];
        let word = &body[t.start..t.end];
        match word {
            "{" | "(" | "[" => depth += 1,
            "}" | ")" | "]" => depth -= 1,
            _ => {}
        }
        if depth == 1 && t.kind == Tok::Ident && toks.get(i + 1).is_some_and(|c| &body[c.start..c.end] == ":") {
            // The value: up to the next field at this depth, or the close.
            let first = i + 2;
            let mut j = first;
            let mut d = 0i32;
            while j < toks.len() {
                let w = &body[toks[j].start..toks[j].end];
                match w {
                    "{" | "(" | "[" => d += 1,
                    "}" | ")" | "]" => {
                        if d == 0 {
                            break;
                        }
                        d -= 1;
                    }
                    _ => {}
                }
                if d == 0 && j > first && toks[j].kind == Tok::Ident && toks.get(j + 1).is_some_and(|c| &body[c.start..c.end] == ":") {
                    break;
                }
                j += 1;
            }
            if let (Some(a), Some(b)) = (toks.get(first), toks[..j].iter().rev().find(|t| t.kind != Tok::Comment)) {
                if b.end > a.start {
                    out.push((word.to_string(), range.start + a.start..range.start + b.end));
                }
            }
            i = j;
            continue;
        }
        i += 1;
    }
    out
}

/// The node a path's first name is (`""` and `Score`: the final `Score{}`).
fn node_of<'a>(code: &'a ScoreCode, name: &str) -> Option<&'a source::Ctor> {
    if name.is_empty() || name == "Score" {
        code.outline.root.as_ref()
    } else {
        code.outline.node(name)
    }
}

/// Every bindable path of a score: (path, value range).
fn paths(text: &str, code: &ScoreCode) -> Vec<(String, Range<usize>)> {
    let mut out = Vec::new();
    for ctor in code.outline.nodes.iter() {
        for field in ctor.fields.iter().filter(|f| !f.bare) {
            let path = format!("{}.{}", ctor.name, field.key);
            let range = field.value.start..field.value.end;
            if kind_of(&text[range.clone()]) == ValueKind::Record {
                for (key, sub) in record_fields(text, range.clone()) {
                    out.push((format!("{path}.{key}"), sub));
                }
            }
            out.push((path, range));
        }
    }
    out
}

/// The place `path` names, without nearest names.
fn site_exact(files: &[File], path: &str) -> Result<ValueSite, String> {
    let (file, text, code) = own(files).ok_or("no score")?;
    let mut parts = path.split('.');
    let node_name = parts.next().unwrap_or("");
    let ctor = node_of(code, node_name).ok_or_else(|| format!("no node `{node_name}`"))?;
    let field_name = parts.next().ok_or_else(|| format!("`{path}` names a node, not a value: `{node_name}.<field>`"))?;
    let field = ctor.field(field_name).ok_or_else(|| format!("`{node_name}` writes no `{field_name}`"))?;
    let mut range = field.value.start..field.value.end;
    for key in parts {
        range = record_fields(text, range).into_iter().find(|(k, _)| k == key).map(|(_, r)| r).ok_or_else(|| format!("no `{key}` in `{path}`"))?;
    }
    let value = text[range.clone()].to_string();
    Ok(site_of(path, file, range, value))
}

/// A path's site: its kind and value read from the literal.
fn site_of(path: &str, file: usize, range: Range<usize>, text: String) -> ValueSite {
    let kind = kind_of(&text);
    let value = match kind {
        k if k.is_numeric() => text.trim().trim_end_matches(|c: char| c.is_ascii_alphabetic()).parse::<f64>().map_or_else(|_| Value::Code(text.clone()), Value::Num),
        ValueKind::Bool => Value::Bool(text.trim() == "true"),
        _ => Value::Code(text.clone()),
    };
    ValueSite { id: path.to_string(), kind, span: Some((file, range)), text, value, ..Default::default() }
}

/// A number as written, keeping the unit of the literal it replaces.
fn number_like(site: &ValueSite, n: f64) -> String {
    let unit = site.text.trim().trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-');
    format!("{}{unit}", panel::fmt_num(n))
}

/// The score document kind: install it with `makepad_editor_panel::register`.
pub struct ScoreDocKind;

/// A score's values over the files given (the score itself among them).
pub struct ScoreDocValues<'a> {
    files: &'a [File<'a>],
}

impl ValueSource for ScoreDocValues<'_> {
    fn sites(&self) -> Vec<ValueSite> {
        let Some((file, text, code)) = own(self.files) else { return Vec::new() };
        paths(text, code).into_iter().map(|(path, range)| site_of(&path, file, range.clone(), text[range].to_string())).collect()
    }

    fn resolve(&self, id: &str) -> Result<ValueSite, String> {
        site_exact(self.files, id)
    }

    /// The value written in the form of the literal there, checked to read back.
    fn write(&self, id: &str, value: &Value) -> Result<Vec<Splice>, String> {
        let site = site_exact(self.files, id)?;
        let literal = ScoreDocKind.form(&site, value)?;
        let (file, text, _) = own(self.files).ok_or("no score")?;
        let range = site.range();
        // Read back: the score with it written names the same value there.
        let next = format!("{}{}{}", &text[..range.start], literal, &text[range.end..]);
        let code = ScoreCode::of(&next);
        let back = site_exact(&[File { name: "", text: &next, code: &code }], id)?;
        if back.text.trim() != literal.trim() {
            return Err(format!("`{id}` does not read back as {literal} (it reads {})", back.text));
        }
        Ok(vec![Splice { file, range, text: literal }])
    }

    /// The paths of the node a path names (else the nodes).
    fn nearest(&self, id: &str) -> Vec<String> {
        let Some((_, text, code)) = own(self.files) else { return Vec::new() };
        let node = id.split('.').next().unwrap_or("");
        let near: Vec<String> = paths(text, code).into_iter().map(|(p, _)| p).filter(|p| p.split('.').next() == Some(node)).take(12).collect();
        if near.is_empty() { code.outline.nodes.iter().map(|c| c.name.clone()).collect() } else { near }
    }
}

impl PanelDoc for ScoreDocKind {
    fn outline(&self, text: &str) -> Box<dyn Any> {
        Box::new(ScoreCode::of(text))
    }

    fn let_region(&self, _text: &str, code: &dyn Any, name: &str) -> Option<Range<usize>> {
        code_of(code).lets.iter().find(|(n, _)| n == name).map(|(_, r)| r.clone())
    }

    fn let_of_literal(&self, text: &str, code: &dyn Any, site: &Range<usize>) -> Option<String> {
        let (name, region) = code_of(code).lets.iter().find(|(_, r)| r.start <= site.start && site.end <= r.end)?;
        let head = &text[region.start..site.start];
        let before = head[head.find('=')? + 1..].trim();
        let tag_only = before.is_empty() || (before.starts_with("/**") && before.ends_with("*/") && before.matches("*/").count() == 1);
        let after = text[site.end..region.end].trim();
        (tag_only && after.split("//").next().unwrap_or("").trim().is_empty()).then(|| name.clone())
    }

    /// A name slot (`bass.play`): the other nodes of the same type.
    fn fns_like(&self, files: &[File], path: &str) -> Vec<String> {
        let Ok(site) = site_exact(files, path) else { return Vec::new() };
        let Some((_, _, code)) = own(files) else { return Vec::new() };
        let Some(ty) = code.outline.node(site.text.trim()).map(|c| c.type_name.clone()) else { return Vec::new() };
        code.outline.nodes.iter().filter(|c| c.type_name == ty).map(|c| c.name.clone()).collect()
    }
















    fn values<'a>(&'a self, files: &'a [File<'a>]) -> Box<dyn ValueSource + 'a> {
        Box::new(ScoreDocValues { files })
    }

    fn literal_token(&self, text: &str, range: &Range<usize>) -> Option<Range<usize>> {
        let toks: Vec<_> = source::lex(&text[range.clone()]).into_iter().filter(|t| t.kind != Tok::Comment).collect();
        let at = |a: usize, b: usize| range.start + a..range.start + b;
        match toks.as_slice() {
            [one] => Some(at(one.start, one.end)),
            [minus, num] if &text[range.start + minus.start..range.start + minus.end] == "-" && num.kind == Tok::Num => Some(at(minus.start, num.end)),
            _ => None,
        }
    }

    fn form(&self, site: &ValueSite, value: &Value) -> Result<String, String> {
        Ok(match value {
            Value::Num(n) => match site.kind {
                k if k.is_numeric() => number_like(site, *n),
                ValueKind::Bool => (if *n >= 0.5 { "true" } else { "false" }).to_string(),
                _ => panel::fmt_num(*n),
            },
            Value::Bool(b) => b.to_string(),
            Value::Text(t) if site.kind == ValueKind::Choice || site.kind == ValueKind::Function => {
                if site.kind == ValueKind::Choice && !t.starts_with('@') { format!("@{t}") } else { t.clone() }
            }
            Value::Text(t) => panel::fmt_text(t),
            Value::Code(c) => c.clone(),
            _ => return Err(format!("`{}` takes a {}", site.id, site.kind.name())),
        })
    }
}

/// The score document kind, to install.
pub fn score_doc() -> std::rc::Rc<dyn PanelDoc> {
    std::rc::Rc::new(ScoreDocKind)
}

/// Every path of a score a panel can bind (as the panel kit lists them).
pub fn resolve(source: &str, path: &str) -> Result<ValueSite, String> {
    let code = ScoreCode::of(source);
    site_exact(&[File { name: "", text: source, code: &code }], path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCORE: &str = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet lead = Synth{preset: @pluck cutoff: 1200 env: {attack: 0.01 release: 0.4}}\nlet a = Loop{play: lead amp: 0.8 code: fn(i) { play(@c4) sleep(1) }}\n// the panel\nlet editor_panel = fn() {\n    View{ flow: Down ScoreTree{} }\n}\n\nScore{clock lead a}\n";

    fn files<'a>(text: &'a str, code: &'a ScoreCode) -> [File<'a>; 1] {
        [File { name: "", text, code }]
    }

    #[test]
    fn paths_resolve_to_the_values_as_written() {
        let code = ScoreCode::of(SCORE);
        let f = files(SCORE, &code);
        let doc = ScoreDocKind;
        let values = doc.values(&f);
        let cutoff = values.resolve("lead.cutoff").unwrap();
        assert_eq!((cutoff.text.as_str(), cutoff.kind, cutoff.value.clone()), ("1200", ValueKind::Number, Value::Num(1200.0)));
        assert_eq!(values.resolve("lead.preset").unwrap().kind, ValueKind::Choice);
        assert_eq!(values.resolve("lead.env.release").unwrap().text, "0.4");
        assert_eq!(values.resolve("a.play").unwrap().kind, ValueKind::Function);
        assert!(values.resolve("lead.res").is_err());
        assert!(values.nearest("lead.res").contains(&"lead.cutoff".to_string()));
        assert_eq!(doc.fns_like(&f, "a.play"), vec!["lead".to_string()]);
        let listed: Vec<String> = values.sites().into_iter().map(|e| e.id).collect();
        assert!(listed.contains(&"lead.env.attack".to_string()) && listed.contains(&"a.amp".to_string()), "{listed:?}");
    }

    #[test]
    fn writes_keep_the_form_and_read_back() {
        let code = ScoreCode::of(SCORE);
        let f = files(SCORE, &code);
        let doc = ScoreDocKind;
        let values = doc.values(&f);
        let w = values.write("lead.cutoff", &Value::Num(800.0)).unwrap();
        assert_eq!(w[0].text, "800");
        assert_eq!(&SCORE[w[0].range.clone()], "1200");
        assert_eq!(values.write("lead.preset", &Value::Text("saw".into())).unwrap()[0].text, "@saw");
    }

    #[test]
    fn the_panel_is_a_top_level_let() {
        let code = ScoreCode::of(SCORE);
        let region = doc_region(&code, "editor_panel");
        assert!(SCORE[region.clone()].starts_with("let editor_panel") && SCORE[region].ends_with('}'));
        assert!(SCORE[doc_region(&code, "lead")].ends_with("release: 0.4}}"));
    }

    fn doc_region(code: &ScoreCode, name: &str) -> Range<usize> {
        ScoreDocKind.let_region(SCORE, code, name).unwrap()
    }
}
