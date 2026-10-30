//! The score's editable values: every field of every node, typed from the
//! spec tables (range, taper, unit, choices), as value sites with ids
//! `node.key` (`acid.cutoff`; the final `Score{…}`'s fields are
//! `Score.key`), and their writes as splices formatted like the score.

use crate::spec::{keys, spec, spec_or_generic, ParamSpec, Taper as SpecTaper, Unit};
use makepad_edit_values::{Live, Range, Splice, Taper, Value, ValueKind, ValueSite, ValueSource};
use makepad_score_live::source::{outline, set_field, Ctor, Outline};

/// A score's values, read once from its source.
pub struct ScoreValues<'a> {
    src: &'a str,
    outline: Outline,
}

impl<'a> ScoreValues<'a> {
    pub fn new(src: &'a str) -> ScoreValues<'a> {
        ScoreValues { src, outline: outline(src) }
    }

    fn nodes(&self) -> impl Iterator<Item = &Ctor> {
        self.outline.nodes.iter().chain(self.outline.root.iter())
    }

    fn node(&self, name: &str) -> Option<&Ctor> {
        self.nodes().find(|n| node_name(n) == name)
    }

    /// The site of `key` on `node` (written or not).
    fn site(&self, node: &Ctor, key: &str) -> Option<ValueSite> {
        let field = node.field(key);
        let written = field.map(|f| f.value.text(self.src).trim());
        let number = written.and_then(|w| w.parse::<f64>().ok());
        let table = spec(&node.type_name, key);
        let known = table.is_some();
        let spec = match table {
            Some(spec) => spec,
            // A field no table knows: typed only when it is a number.
            None => ParamSpec::generic(key, Some(number?)),
        };
        let (kind, unit) = kind_of(&spec);
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
            range: if known { Range::hard(lo, hi) } else { Range::soft(lo, hi) },
            step: if spec.taper == SpecTaper::Int { 1.0 } else { 0.0 },
            taper: match spec.taper {
                SpecTaper::Linear => Taper::Linear,
                SpecTaper::Log => Taper::Log,
                SpecTaper::Int => Taper::Int,
            },
            choices: spec.options.clone(),
            default: Some(default_of(&spec)),
            value,
            about: spec.about.to_string(),
            text: written.unwrap_or("").to_string(),
            span: field.map(|f| (0, f.value.start..f.value.end)),
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
fn kind_of(spec: &ParamSpec) -> (ValueKind, &'static str) {
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
        Unit::None if spec.taper == SpecTaper::Int => (ValueKind::Count, ""),
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

impl ValueSource for ScoreValues<'_> {
    /// Each node's fields: its table's order first, then the other fields
    /// written on it.
    fn sites(&self) -> Vec<ValueSite> {
        let mut out = Vec::new();
        for node in self.nodes() {
            let table = keys(&node.type_name);
            let written = node.fields.iter().filter(|f| !f.bare).map(|f| f.key.as_str()).filter(|k| !table.contains(k));
            for key in table.iter().copied().chain(written) {
                out.extend(self.site(node, key));
            }
        }
        out
    }

    fn resolve(&self, id: &str) -> Result<ValueSite, String> {
        let (node, key) = id.rsplit_once('.').ok_or_else(|| format!("`{id}` is not `node.field`"))?;
        let ctor = self.node(node).ok_or_else(|| format!("no node `{node}`"))?;
        self.site(ctor, key).ok_or_else(|| format!("`{id}` is not a value"))
    }

    fn write(&self, id: &str, value: &Value) -> Result<Vec<Splice>, String> {
        let site = self.resolve(id)?;
        let (node, key) = id.rsplit_once('.').unwrap_or(("", id));
        let ctor = self.node(node).ok_or_else(|| format!("no node `{node}`"))?;
        let spec = spec_or_generic(&ctor.type_name, key, site.value.num());
        let text = match value {
            Value::Num(x) => spec.source_text(site.range.clamp(*x)),
            Value::Choice(i) => format!("@{}", spec.options.get(*i).ok_or_else(|| format!("`{id}` has no choice {i}"))?),
            Value::Bool(b) => b.to_string(),
            Value::Code(code) => code.clone(),
            _ => return Err(format!("`{id}` takes a {}", site.kind.name())),
        };
        let at = if node == "Score" { "" } else { node };
        let s = set_field(self.src, at, key, &text)?;
        Ok(vec![Splice { file: 0, range: s.start..s.end, text: s.text }])
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
}
