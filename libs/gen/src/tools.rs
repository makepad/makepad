//! The `gen.*` tools as this library answers them: the shared text and
//! shapes (`makepad-gen-tools`) plus what needs the catalog.

pub use makepad_gen_tools::*;
use makepad_strict_json::Value as Json;

fn s(text: impl Into<String>) -> Json {
    Json::Str(text.into())
}

/// A recipe literal as JSON.
pub fn literal_json(value: &makepad_flow::Literal) -> Json {
    use makepad_flow::Literal;
    match value {
        Literal::Null => Json::Null,
        Literal::Bool(value) => Json::Bool(*value),
        Literal::Num(value) if value.fract() == 0.0 && value.abs() < 9.0e15 => Json::Int(*value as i64),
        Literal::Num(value) => Json::F64(*value),
        Literal::Str(text) | Literal::Id(text) => Json::Str(text.clone()),
        Literal::Arr(items) => Json::Arr(items.iter().map(literal_json).collect()),
        Literal::Obj(fields) => Json::Obj(fields.iter().map(|(name, value)| (name.clone(), literal_json(value))).collect()),
    }
}

/// A JSON argument as a recipe literal.
pub fn json_literal(value: &Json) -> makepad_flow::Literal {
    use makepad_flow::Literal;
    match value {
        Json::Null => Literal::Null,
        Json::Bool(value) => Literal::Bool(*value),
        Json::Int(value) => Literal::Num(*value as f64),
        Json::F64(value) => Literal::Num(*value),
        Json::Str(text) => Literal::Str(text.clone()),
        Json::Arr(items) => Literal::Arr(items.iter().map(json_literal).collect()),
        Json::Obj(fields) => Literal::Obj(fields.iter().map(|(name, value)| (name.clone(), json_literal(value))).collect()),
    }
}

/// A preset as `gen.presets` lists it; `ready` is what the fleet has now.
pub fn preset_json(info: &crate::catalog::PresetInfo, ready: Option<&Readiness>) -> Json {
    let mut fields = vec![
        ("id".to_string(), s(&info.id)),
        ("kind".to_string(), s(&info.kind)),
        ("label".to_string(), s(&info.label)),
        ("brief".to_string(), s(&info.brief)),
    ];
    if let Some((doc, _)) = &info.prompt {
        fields.push(("prompt".into(), s(doc)));
    }
    if !info.media.is_empty() {
        fields.push(("inputs".into(), Json::Arr(info.media.iter().map(|input| {
            Json::Obj(vec![("name".to_string(), s(&input.node)), ("type".to_string(), s(input.ty.as_str())), ("about".to_string(), s(&input.doc))])
        }).collect())));
    }
    let mut params: Vec<(String, Json)> = info.texts.iter().map(|text| (text.node.clone(), s(&text.default))).collect();
    params.extend(info.params.iter().map(|(name, value)| (name.clone(), literal_json(value))));
    if !params.is_empty() {
        fields.push(("params".into(), Json::Obj(params)));
    }
    fields.push(("models".into(), Json::Arr(info.generators.iter().map(|generator| {
        let model = if generator.model.is_empty() { "the fleet's choice".to_string() } else { generator.model.clone() };
        s(format!("{}: {model}", generator.domain))
    }).collect())));
    if let Some(ready) = ready {
        fields.push(("ready".into(), Json::Bool(ready.missing.is_empty())));
        if !ready.missing.is_empty() {
            fields.push(("missing".into(), Json::Arr(ready.missing.iter().map(s).collect())));
        }
        if let Some(eta) = ready.eta_secs {
            fields.push(("eta_seconds".into(), Json::Int(eta.round() as i64)));
        }
    }
    Json::Obj(fields)
}

/// Whether the fleet can run a preset now, and roughly how long it takes.
#[derive(Clone, Debug, Default)]
pub struct Readiness {
    /// Generators no node can run, as `domain: model`.
    pub missing: Vec<String>,
    pub eta_secs: Option<f64>,
}

