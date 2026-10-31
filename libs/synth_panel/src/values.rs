//! The score's editable values are typed from the spec tables or their
//! literals, with ids `node.key`, `Score.key` and `node.field.key` for
//! record fields. Writes preserve the score's form and are checked to read back.

use crate::spec::{keys, spec, ParamSpec, Unit};
use makepad_edit_values::{apply_splices, Live, Range, Splice, Taper, Value, ValueKind, ValueSite, ValueSource};
use makepad_editor_panel::{fmt_num, fmt_text};
use makepad_score_live::source::{self, outline, set_field, Ctor, Outline, Tok};
use std::ops::Range as TextRange;

/// A score's values, read once from its source.
pub struct ScoreValues<'a> {
    src: &'a str,
    file: usize,
    outline: Outline,
}

impl<'a> ScoreValues<'a> {
    pub fn new(src: &'a str) -> ScoreValues<'a> {
        Self::in_file(src, 0)
    }

    /// Read the score's values at the given file index.
    pub fn in_file(src: &'a str, file: usize) -> ScoreValues<'a> {
        ScoreValues { src, file, outline: outline(src) }
    }

    fn nodes(&self) -> impl Iterator<Item = &Ctor> {
        self.outline.nodes.iter().chain(self.outline.root.iter())
    }

    fn node(&self, name: &str) -> Option<&Ctor> {
        if name.is_empty() || name == "Score" {
            self.outline.root.as_ref()
        } else {
            self.outline.node(name)
        }
    }

    /// The site of `key` on `node` (written or not).
    fn site(&self, node: &Ctor, key: &str) -> Option<ValueSite> {
        let field = node.field(key);
        let written = field.map(|f| f.value.text(self.src).trim());
        let number = written.and_then(|w| w.parse::<f64>().ok());
        let Some(spec) = spec(&node.type_name, key) else {
            let field = field?;
            let range = field.value.start..field.value.end;
            return Some(site_of(&format!("{}.{key}", node_name(node)), self.file, range.clone(), self.src[range].to_string()));
        };
        let (kind, unit) = spec_kind(&spec);
        let value = match (written, spec.is_choice(), spec.unit) {
            (Some(w), true, _) => Value::Choice(spec.options.iter().position(|o| o == w.trim_start_matches('@')).unwrap_or(0)),
            (Some(w), _, Unit::Bool) => Value::Bool(w == "true"),
            (Some(w), _, _) => match number {
                Some(x) => Value::Num(x),
                // An expression: shown as written.
                None => Value::Code(w.to_string()),
            },
            (None, _, _) => default_of(&spec),
        };
        let lo = spec.min.min(spec.max);
        let hi = spec.max.max(spec.min);
        Some(ValueSite {
            id: format!("{}.{key}", node_name(node)),
            label: if spec.label.is_empty() { key.to_string() } else { spec.label.to_string() },
            group: if spec.group.is_empty() { node_name(node).to_string() } else { format!("{}.{}", node_name(node), spec.group) },
            kind,
            unit: unit.to_string(),
            range: Range::hard(lo, hi),
            step: if spec.taper == Taper::Int { 1.0 } else { 0.0 },
            taper: spec.taper,
            choices: spec.options.clone(),
            default: Some(default_of(&spec)),
            value,
            about: spec.about.to_string(),
            text: written.unwrap_or("").to_string(),
            span: field.map(|f| (self.file, f.value.start..f.value.end)),
            live: Live::Reload,
            shapes: vec![node_name(node).to_string()],
            ..Default::default()
        })
    }
}

fn node_name(node: &Ctor) -> &str {
    if node.name.is_empty() {
        "Score"
    } else {
        &node.name
    }
}

/// The kind and unit a spec's field has.
fn spec_kind(spec: &ParamSpec) -> (ValueKind, &'static str) {
    if spec.is_choice() {
        return (ValueKind::Choice, "");
    }
    match spec.unit {
        Unit::Hz => (ValueKind::Frequency, "hz"),
        Unit::Secs => (ValueKind::Time, "s"),
        Unit::Ms => (ValueKind::Time, "ms"),
        Unit::Beats => (ValueKind::Time, "beats"),
        Unit::Cents => (ValueKind::Number, "ct"),
        Unit::Semis => (ValueKind::Number, "st"),
        Unit::Db => (ValueKind::Gain, "db"),
        Unit::Percent => (ValueKind::Ratio, "%"),
        Unit::Bool => (ValueKind::Bool, ""),
        // A note value (an arp's rate), written as a fraction (`1/16`).
        Unit::Note => (ValueKind::Time, ""),
        Unit::None if spec.taper == Taper::Int => (ValueKind::Count, ""),
        Unit::None => (ValueKind::Number, ""),
    }
}

fn default_of(spec: &ParamSpec) -> Value {
    if spec.is_choice() {
        Value::Choice(spec.default.round().max(0.0) as usize)
    } else if spec.unit == Unit::Bool {
        Value::Bool(spec.default > 0.5)
    } else {
        Value::Num(spec.default)
    }
}

/// Read a value's kind from its literal.
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

/// List the keys and value ranges of a record literal at the given range.
fn record_fields(text: &str, range: TextRange<usize>) -> Vec<(String, TextRange<usize>)> {
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
            // The value ends at the next field at this depth, or the close.
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

/// Read a site's kind and value from its literal.
fn site_of(path: &str, file: usize, range: TextRange<usize>, text: String) -> ValueSite {
    let kind = kind_of(&text);
    let value = match kind {
        k if k.is_numeric() => text.trim().trim_end_matches(|c: char| c.is_ascii_alphabetic()).parse::<f64>().map_or_else(|_| Value::Code(text.clone()), Value::Num),
        ValueKind::Bool => Value::Bool(text.trim() == "true"),
        _ => Value::Code(text.clone()),
    };
    let node = path.split('.').next().unwrap_or("");
    let key = path.rsplit('.').next().unwrap_or(path);
    let generic = value.num().map(|n| ParamSpec::generic(key, Some(n)));
    ValueSite {
        id: path.to_string(),
        label: key.to_string(),
        group: node.to_string(),
        kind,
        range: generic.as_ref().map_or_else(Range::default, |s| Range::soft(s.min, s.max)),
        default: value.num().map(Value::Num),
        span: Some((file, range)),
        text,
        value,
        live: Live::Reload,
        shapes: vec![node.to_string()],
        ..Default::default()
    }
}

/// Format a number with the unit of the literal it replaces.
fn number_like(site: &ValueSite, n: f64) -> String {
    let unit = site.text.trim().trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-');
    format!("{}{unit}", fmt_num(n))
}

impl ValueSource for ScoreValues<'_> {
    /// Each node's fields: its table's order first, then the other fields
    /// written on it.
    fn sites(&self) -> Vec<ValueSite> {
        let mut out = Vec::new();
        for node in self.nodes() {
            let table = keys(&node.type_name);
            let written = node.fields.iter().filter(|f| !f.bare).map(|f| f.key.as_str()).filter(|k| !table.contains(k));
            for key in table.iter().copied().chain(written) {
                if let Some(site) = self.site(node, key) {
                    if kind_of(&site.text) == ValueKind::Record {
                        for (key, range) in record_fields(self.src, site.range()) {
                            out.push(site_of(&format!("{}.{key}", site.id), self.file, range.clone(), self.src[range].to_string()));
                        }
                    }
                    out.push(site);
                }
            }
        }
        out
    }

    fn resolve(&self, id: &str) -> Result<ValueSite, String> {
        let mut parts = id.split('.');
        let node = parts.next().unwrap_or("");
        let key = parts.next().ok_or_else(|| format!("`{id}` is not `node.field`"))?;
        let ctor = self.node(node).ok_or_else(|| format!("no node `{node}`"))?;
        if parts.clone().next().is_none() {
            return self.site(ctor, key).ok_or_else(|| format!("`{id}` is not a value"));
        }
        let field = ctor.field(key).ok_or_else(|| format!("`{node}` writes no `{key}`"))?;
        let mut range = field.value.start..field.value.end;
        for key in parts {
            range = record_fields(self.src, range).into_iter().find(|(k, _)| k == key).map(|(_, r)| r).ok_or_else(|| format!("no `{key}` in `{id}`"))?;
        }
        Ok(site_of(id, self.file, range.clone(), self.src[range].to_string()))
    }

    fn write(&self, id: &str, value: &Value) -> Result<Vec<Splice>, String> {
        let site = self.resolve(id)?;
        let (node, key) = id.split_once('.').unwrap_or(("", id));
        let ctor = self.node(node).ok_or_else(|| format!("no node `{node}`"))?;
        let table = if key.contains('.') { None } else { spec(&ctor.type_name, key) };
        let literal = match value {
            Value::Num(n) => match table {
                Some(spec) => spec.source_text(site.range.clamp(*n)),
                None if site.kind.is_numeric() => number_like(&site, *n),
                None if site.kind == ValueKind::Bool => (if *n >= 0.5 { "true" } else { "false" }).to_string(),
                None => fmt_num(*n),
            },
            Value::Choice(i) => format!("@{}", site.choices.get(*i).ok_or_else(|| format!("`{id}` has no choice {i}"))?),
            Value::Bool(b) => b.to_string(),
            Value::Text(t) if site.kind == ValueKind::Choice || site.kind == ValueKind::Function => {
                if site.kind == ValueKind::Choice && !t.starts_with('@') { format!("@{t}") } else { t.clone() }
            }
            Value::Text(t) => fmt_text(t),
            Value::Code(code) => code.clone(),
            _ => return Err(format!("`{id}` takes a {}", site.kind.name())),
        };
        let splice = if let Some((_, range)) = &site.span {
            Splice { file: self.file, range: range.clone(), text: literal.clone() }
        } else {
            let s = set_field(self.src, node, key, &literal)?;
            Splice { file: self.file, ..s }
        };
        let next = apply_splices(self.src, self.file, std::slice::from_ref(&splice))?;
        let back = ScoreValues::in_file(&next, self.file).resolve(id)?;
        if back.text.trim() != literal.trim() {
            return Err(format!("`{id}` does not read back as {literal} (it reads {})", back.text));
        }
        Ok(vec![splice])
    }

    /// Suggest the written paths of the named node, or the node names.
    fn nearest(&self, id: &str) -> Vec<String> {
        let node = id.split('.').next().unwrap_or("");
        let mut near = Vec::new();
        if let Some(ctor) = self.node(node) {
            for field in ctor.fields.iter().filter(|f| !f.bare) {
                let path = format!("{}.{}", node_name(ctor), field.key);
                let range = field.value.start..field.value.end;
                if kind_of(&self.src[range.clone()]) == ValueKind::Record {
                    near.extend(record_fields(self.src, range).into_iter().map(|(key, _)| format!("{path}.{key}")));
                }
                near.push(path);
            }
        }
        near.truncate(12);
        if near.is_empty() { self.outline.nodes.iter().map(|c| c.name.clone()).collect() } else { near }
    }
}

/// Every typed value of the score `src` (the inline code controls' sites).
pub fn sites(src: &str) -> Vec<ValueSite> {
    ScoreValues::new(src).sites()
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_edit_values::apply_splices;

    const SRC: &str = "let acid = Synth{osc: @saw cutoff: 800 res: 0.5}\nlet hats = Drums{kit: @lofi}\nScore{bpm: 120 acid hats}";

    #[test]
    fn spec_fields_are_typed_sites() {
        let v = ScoreValues::new(SRC);
        let cutoff = v.resolve("acid.cutoff").unwrap();
        assert_eq!(cutoff.kind, ValueKind::Frequency);
        assert_eq!(cutoff.unit, "hz");
        assert_eq!(cutoff.value, Value::Num(800.0));
        assert_eq!(&SRC[cutoff.span.clone().unwrap().1], "800");
        let osc = v.resolve("acid.osc").unwrap();
        assert_eq!(osc.kind, ValueKind::Choice);
        assert_eq!(osc.value, Value::Choice(0));
        assert_eq!(osc.choices[0], "saw");
        // Unwritten fields of the table are listed, not placed.
        let release = v.resolve("acid.release").unwrap();
        assert!(release.span.is_none());
        assert!(v.sites().iter().any(|s| s.id == "Score.bpm"));
    }

    #[test]
    fn writes_are_formatted_like_the_score() {
        let v = ScoreValues::new(SRC);
        let out = apply_splices(SRC, 0, &v.write("acid.cutoff", &Value::Num(1234.56)).unwrap()).unwrap();
        assert!(out.contains("cutoff: 1235 "), "{out}");
        let out = apply_splices(SRC, 0, &v.write("acid.osc", &Value::Choice(1)).unwrap()).unwrap();
        assert!(out.contains("osc: @square"), "{out}");
        let out = apply_splices(SRC, 0, &v.write("acid.release", &Value::Num(0.3)).unwrap()).unwrap();
        assert!(out.contains("release: 0.3}"), "{out}");
    }

    #[test]
    fn record_subfields_are_literal_sites() {
        let src = "let lead = Synth{env: {attack: 0.01 release: 0.4s}}\nScore{lead}";
        let v = ScoreValues::in_file(src, 2);
        let release = v.resolve("lead.env.release").unwrap();
        assert_eq!(release.kind, ValueKind::Time);
        assert_eq!(release.value, Value::Num(0.4));
        assert_eq!(release.file(), 2);
        assert_eq!(&src[release.range()], "0.4s");
        assert!(v.sites().contains(&release));
        assert!(v.sites().iter().filter_map(|s| s.span.as_ref()).all(|(file, _)| *file == 2));
        assert!(v.nearest("lead.env.missing").contains(&release.id));

        let edits = v.write("lead.env.release", &Value::Num(12.3456)).unwrap();
        assert_eq!(edits[0].file, 2);
        assert_eq!(edits[0].range, release.range());
        assert_eq!(edits[0].text, "12.3456s");
        let out = apply_splices(src, 2, &edits).unwrap();
        assert_eq!(out, "let lead = Synth{env: {attack: 0.01 release: 12.3456s}}\nScore{lead}");
        assert_eq!(ScoreValues::in_file(&out, 2).resolve("lead.env.release").unwrap().value, Value::Num(12.3456));
        assert!(v.write("lead.env.release", &Value::Code("0.5s extra: 1".into())).is_err());
    }

    #[test]
    fn choices_write_and_read_back() {
        let src = "let lead = Synth{osc: @saw preset: @pluck}\nScore{lead}";
        let v = ScoreValues::in_file(src, 3);
        for (id, value, expected) in [
            ("lead.osc", Value::Choice(1), "@square"),
            ("lead.filter", Value::Choice(2), "@bp"),
            ("lead.preset", Value::Text("saw".into()), "@saw"),
        ] {
            let edits = v.write(id, &value).unwrap();
            assert_eq!(edits[0].file, 3);
            let out = apply_splices(src, 3, &edits).unwrap();
            let back = ScoreValues::in_file(&out, 3).resolve(id).unwrap();
            assert_eq!(back.kind, ValueKind::Choice);
            assert_eq!(back.text, expected);
            assert_eq!(back.file(), 3);
            if matches!(value, Value::Choice(_)) {
                assert_eq!(back.value, value);
            }
        }
        assert!(v.write("lead.osc", &Value::Choice(usize::MAX)).is_err());
        assert!(v.write("lead.osc", &Value::Code("@square extra: 1".into())).is_err());
    }
}
