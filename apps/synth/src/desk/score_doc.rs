//! A score as the document the editor panel edits (`makepad_editor_panel`'s
//! [`PanelDoc`], its values a [`ValueSource`]): its paths are the score's own names, `node.field`
//! (`mono.cutoff`, `bass.amp`, `echo.mix`) and a field of a record value
//! one level down (`lead.env.attack`); its outline is the score's
//! (`makepad_score_live::source`) plus every top-level `let` (the panel's
//! own `let editor_panel = fn() { … }` among them). A score writes no
//! colours and no keyed values: those parts of the trait say so.

use makepad_edit_values::{Value, ValueKind, ValueSite, ValueSource};
use makepad_editor_panel::{self as panel, File, PanelDoc};
use makepad_score_live::source::{self, Outline, Tok};
use makepad_synth_panel::values::ScoreValues;
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

/// A number as written, keeping the unit of the literal it replaces.
fn number_like(site: &ValueSite, n: f64) -> String {
    let unit = site.text.trim().trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-');
    format!("{}{unit}", panel::fmt_num(n))
}

/// The score document kind: install it with `makepad_editor_panel::register`.
pub struct ScoreDocKind;

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
        let Some((file, text, code)) = own(files) else { return Vec::new() };
        let Ok(site) = ScoreValues::in_file(text, file).resolve(path) else { return Vec::new() };
        let Some(ty) = code.outline.node(site.text.trim()).map(|c| c.type_name.clone()) else { return Vec::new() };
        code.outline.nodes.iter().filter(|c| c.type_name == ty).map(|c| c.name.clone()).collect()
    }

    fn values<'a>(&'a self, files: &'a [File<'a>]) -> Box<dyn ValueSource + 'a> {
        let (file, text) = own(files).map_or((0, ""), |(file, text, _)| (file, text));
        Box::new(ScoreValues::in_file(text, file))
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
    ScoreValues::new(source).resolve(path)
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
        assert_eq!((cutoff.text.as_str(), cutoff.kind, cutoff.value.clone()), ("1200", ValueKind::Frequency, Value::Num(1200.0)));
        assert_eq!(values.resolve("lead.preset").unwrap().kind, ValueKind::Choice);
        assert_eq!(values.resolve("lead.preset").unwrap().value, Value::Code("@pluck".into()));
        assert_eq!(values.resolve("lead.env").unwrap().kind, ValueKind::Record);
        assert_eq!(values.resolve("lead.env.release").unwrap().kind, ValueKind::Number);
        assert_eq!(values.resolve("lead.env.release").unwrap().text, "0.4");
        assert_eq!(values.resolve("a.play").unwrap().kind, ValueKind::Function);
        assert_eq!(values.resolve("a.code").unwrap().kind, ValueKind::Code);
        assert!(values.resolve("lead.res").unwrap().span.is_none());
        assert!(values.resolve("lead.missing").is_err());
        assert!(values.nearest("lead.res").contains(&"lead.cutoff".to_string()));
        assert_eq!(doc.fns_like(&f, "a.play"), vec!["lead".to_string()]);
        let listed: Vec<String> = values.sites().into_iter().map(|e| e.id).collect();
        assert!(listed.contains(&"lead.env.attack".to_string()) && listed.contains(&"a.amp".to_string()), "{listed:?}");
    }

    #[test]
    fn writes_keep_the_form_and_read_back() {
        let code = ScoreCode::of(SCORE);
        let f = [File { name: "shared", text: SCORE, code: &code }, File { name: "", text: SCORE, code: &code }];
        let doc = ScoreDocKind;
        let values = doc.values(&f);
        let w = values.write("lead.cutoff", &Value::Num(800.0)).unwrap();
        assert_eq!(w[0].file, 1);
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
