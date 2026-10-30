//! The Score tools every app's AI offers, run: their definitions are
//! `makepad-score-tools` (re-exported here), the ops and the guide text are
//! here. Stage's control API and Sandbox's chat both call these, so their
//! AIs see the same tool names, descriptions and guide text. What stays in each app is
//! its shell: Stage's open score, transport and library (`score_write`,
//! `score_play`, `score_save`, ...), Sandbox's per-game saves
//! (`world.score`).
//!
//! A tool's `source` is optional: without it an app with a score open
//! checks or hears that one ([`call`] takes it as `open`); an app without
//! one refuses.

use crate::listen::{self, Heard, Level};
use crate::model::Diag;
use crate::NoSamples;
use makepad_strict_json::{self as json, Value};

pub use makepad_score_tools::{find, ScoreToolDef, CHECK, GUIDE, INSTRUMENT_SEARCH, LISTEN, MORPH_PARAMS, TOOLS};

/// The composer guide `score_guide` answers.
pub fn guide() -> String {
    crate::guide::guide()
}

/// Run Score tool `name` (dotted or underscored) with `args` (checked
/// against its schema by the registry). `open` is the app's open score, used when `args` has no
/// `source`. Blocking: `score_listen` renders, so a UI thread hands it to a
/// worker. `Err` is a refusal to show the model.
pub fn call(name: &str, args: &Value, open: Option<&str>) -> Result<Value, String> {
    match find(name).map(|tool| tool.api_name) {
        Some("score_guide") => Ok(json::obj(vec![("guide", json::s(guide()))])),
        Some("score_check") => Ok(check(&source_of(args, open)?)),
        Some("score_listen") => listen_args(args, open).map(|(json, _)| json),
        Some("instrument_search") => Ok(instrument_search(args)),
        Some("morph_params") => Ok(morph_params(args)),
        Some("tandem_params") => Ok(tandem_params(args)),
        Some("tandem_patches") => Ok(tandem_patches(args)),
        _ => Err(format!("no score tool `{name}`")),
    }
}

/// `score_listen`: its answer and what was heard (for a host that also
/// draws it), or its problems as the answer when the score does not load.
pub fn listen_args(args: &Value, open: Option<&str>) -> Result<(Value, Option<Heard>), String> {
    let source = source_of(args, open)?;
    let int = |key: &str| match args.get(key) {
        Some(Value::Int(n)) => Some(*n as f64),
        Some(Value::F64(n)) => Some(*n),
        _ => None,
    };
    let from_bar = int("from_bar").unwrap_or(0.0).clamp(0.0, 10_000.0) as u32;
    let bars = int("bars").unwrap_or(4.0).clamp(1.0, 64.0) as u32;
    let params: Vec<(String, f32)> = match args.get("params") {
        Some(Value::Obj(pairs)) => pairs.iter().filter_map(|(k, v)| match v {
            Value::Int(n) => Some((k.clone(), *n as f32)),
            Value::F64(n) => Some((k.clone(), *n as f32)),
            _ => None,
        }).collect(),
        _ => Vec::new(),
    };
    // Fields held live from a bar on (a control moved mid-render): on the
    // one prepared graph, as the editor panel's knobs land.
    let bar_seconds = crate::model::ScoreDoc::load(&source).map(|d| d.beats_per_bar() * 60.0 / d.clock.bpm.max(1.0)).unwrap_or(2.0);
    let held: Vec<crate::held::Held> = match args.get("held") {
        Some(Value::Arr(items)) => items.iter().filter_map(|item| {
            let (node, field) = crate::held::node_field(item.get("path")?.as_str()?)?;
            let number = |v: &Value| match v {
                Value::Int(n) => Some(*n as f64),
                Value::F64(n) => Some(*n),
                Value::Str(t) => t.trim().parse::<f64>().ok(),
                _ => None,
            };
            let value = number(item.get("value")?)? as f32;
            let at = item.get("at_bar").and_then(number).unwrap_or(0.0) * bar_seconds;
            Some(crate::held::Held { at, node: node.to_string(), field: field.to_string(), value })
        }).collect(),
        _ => Vec::new(),
    };
    // What each held field lands on (none: it is only heard once written).
    let doc = crate::model::ScoreDoc::load(&source).ok();
    let landed: Vec<Value> = held.iter().map(|h| {
        let n = doc.as_ref().map_or(0, |d| crate::held::targets(d, &h.node, &h.field).len());
        json::obj(vec![
            ("path", json::s(format!("{}.{}", h.node, h.field))),
            ("value", Value::F64(h.value as f64)),
            ("from_s", num(h.at)),
            ("lands", json::s(if n > 0 { format!("live on {n} loop(s) or control(s)") } else { "nowhere live: write it to hear it".to_string() })),
        ])
    }).collect();
    match listen::listen(&source, from_bar, bars, params, held, &NoSamples) {
        Ok(heard) => {
            let mut answer = heard_json(&heard);
            if let Value::Obj(fields) = &mut answer {
                fields.push(("held".into(), Value::Arr(landed)));
            }
            Ok((answer, Some(heard)))
        }
        Err(diags) => Ok((json::obj(vec![("ok", Value::Bool(false)), ("problems", problems(&diags, &source))]), None)),
    }
}

/// `morph_params`: Morph's schema as the AI reads it (some groups or all).
pub fn morph_params(args: &Value) -> Value {
    use makepad_audio_synth::{kit::matrix, morph};
    let groups: Vec<String> = match args.get("groups") {
        Some(Value::Arr(items)) => items.iter().filter_map(|g| g.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    };
    let schema = &morph::layout().schema;
    // Without groups: everything but the rack's effects (~30 groups; asked for by name, `fx_*` for all).
    let wanted: Vec<&str> = if groups.is_empty() {
        schema.groups.iter().map(|g| g.name.as_str()).filter(|g| !g.starts_with("fx_")).collect()
    } else {
        groups.iter().map(String::as_str).collect()
    };
    json::obj(vec![
        ("structure", json::s(morph::STRUCTURE.to_string())),
        ("params", json::s(schema.describe(&wanted))),
        ("mods", json::s(matrix::ABOUT.to_string())),
        ("presets", Value::Arr(morph::PRESETS.iter().map(|(n, about, _)| json::s(format!("{n}: {about}"))).collect())),
        ("groups", Value::Arr(schema.groups.iter().map(|g| json::s(g.name.clone())).collect())),
    ])
}

/// `tandem_params`: Tandem's schema as the AI reads it.
pub fn tandem_params(args: &Value) -> Value {
    use makepad_audio_synth::tandem::schema;
    let groups: Vec<String> = match args.get("groups") {
        Some(Value::Arr(items)) => items.iter().filter_map(|g| g.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    };
    let int = |k: &str| match args.get(k) {
        Some(Value::Int(n)) => Some(*n),
        Some(Value::F64(n)) => Some(*n as i64),
        _ => None,
    };
    let s = schema::schema();
    let mut out = vec![("structure", json::s(schema::STRUCTURE.to_string())), ("types", json::s(schema::describe_types()))];
    if !groups.is_empty() {
        let wanted: Vec<&str> = groups.iter().map(String::as_str).collect();
        out.push(("params", json::s(s.describe(&wanted))));
    }
    if let Some(t) = int("osc_type") {
        out.push(("osc", json::s(schema::describe_osc(t.clamp(0, 11) as u8))));
    }
    if let Some(t) = int("fx_type") {
        out.push(("fx", json::s(schema::describe_fx(t.clamp(0, 31) as u8))));
    }
    out.push(("groups", Value::Arr(s.groups.iter().map(|g| json::s(g.name.clone())).collect())));
    json::obj(out)
}

/// `tandem_patches`: library patches by words, or the categories.
pub fn tandem_patches(args: &Value) -> Value {
    let cats = crate::tandem_library::categories();
    if cats.is_empty() {
        return json::obj(vec![("installed", Value::Bool(false)), ("note", json::s("the hybrid patch library is not installed: Resources offers it (an opt-in GPL-3 download)".to_string()))]);
    }
    let query = args.get("query").and_then(Value::as_str).unwrap_or("").to_lowercase();
    let limit = match args.get("limit") {
        Some(Value::Int(n)) => (*n).clamp(1, 100) as usize,
        _ => 20,
    };
    if query.trim().is_empty() {
        let list = cats.iter().map(|c| json::s(format!("{}: {}", c.name, c.patches.len()))).collect();
        return json::obj(vec![("installed", Value::Bool(true)), ("categories", Value::Arr(list))]);
    }
    let words: Vec<&str> = query.split_whitespace().collect();
    let mut hits: Vec<(usize, String)> = Vec::new();
    for c in &cats {
        for p in &c.patches {
            let hay = format!("{} {}", c.name, p.name).to_lowercase();
            let score = words.iter().filter(|w| hay.contains(*w)).count();
            if score > 0 {
                hits.push((score, format!("{}/{}", c.name, p.name)));
            }
        }
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let list = hits.into_iter().take(limit).map(|(_, n)| json::s(n)).collect();
    json::obj(vec![("installed", Value::Bool(true)), ("patches", Value::Arr(list))])
}

/// `instrument_search`: the palette's best matches for a description.
pub fn instrument_search(args: &Value) -> Value {
    let query = args.get("query").and_then(Value::as_str).unwrap_or("");
    let kind = args.get("kind").and_then(Value::as_str).filter(|k| *k != "any");
    let role = args.get("role").and_then(Value::as_str);
    let limit = match args.get("limit") {
        Some(Value::Int(n)) => (*n).clamp(1, 20) as usize,
        Some(Value::F64(n)) => n.clamp(1.0, 20.0) as usize,
        _ => 6,
    };
    let hits = crate::palette::search(query, kind, role, limit);
    let results = hits.iter().map(|hit| {
        let e = &hit.entry;
        let mut fields = vec![
            ("id", json::s(e.id.clone())),
            ("name", json::s(e.name.clone())),
            ("code", json::s(e.code())),
            ("family", json::s(e.family.clone())),
            ("roles", Value::Arr(e.roles.iter().map(|r| json::s(r.clone())).collect())),
            ("words", json::s(e.tags.iter().chain(&e.measured).map(|w| w.replace('_', " ")).collect::<Vec<_>>().join(", "))),
            ("matched", Value::Arr(hit.matched.iter().map(|m| json::s(m.clone())).collect())),
        ];
        if let Some(f) = e.features {
            fields.push(("measured", json::obj(vec![
                ("centroid_hz", num(f.centroid.round() as f64)),
                ("attack_ms", num(f.attack_ms.round() as f64)),
                ("decay_ms", num(f.decay_ms.round() as f64)),
                ("low", num((f.low * 100.0).round() as f64 / 100.0)),
                ("mid", num((f.mid * 100.0).round() as f64 / 100.0)),
                ("high", num((f.high * 100.0).round() as f64 / 100.0)),
                ("lufs", num((f.lufs * 10.0).round() as f64 / 10.0)),
            ])));
            if let Some(gain) = crate::palette::balance_gain(e) {
                fields.push(("gain", num((gain * 100.0).round() as f64 / 100.0)));
            }
        }
        json::obj(fields)
    }).collect();
    json::obj(vec![("results", Value::Arr(results))])
}

/// The source a call names, else the open score; with its last newline
/// (a trailing number without one is dropped by the parser).
fn source_of(args: &Value, open: Option<&str>) -> Result<String, String> {
    let source = match (args.get("source").and_then(Value::as_str), open) {
        (Some(given), _) => given,
        (None, Some(open)) => open,
        (None, None) => return Err("give the score's `source` (no score is open here)".into()),
    };
    Ok(if source.ends_with('\n') { source.to_string() } else { format!("{source}\n") })
}

/// Problems as the tools answer them: line, column, message (with its
/// node) and the line's text, at most 20.
pub fn problems(diags: &[Diag], source: &str) -> Value {
    let lines: Vec<&str> = source.lines().collect();
    Value::Arr(diags.iter().take(20).map(|d| json::obj(vec![
        ("line", Value::Int(d.line as i64)),
        ("col", Value::Int(d.col as i64)),
        ("message", json::s(match &d.node { Some(n) => format!("{n}: {}", d.message), None => d.message.clone() })),
        ("text", json::s(lines.get(d.line.saturating_sub(1) as usize).map_or("", |l| l.trim()))),
    ])).collect())
}

/// Load `source` and run its loop code for 8 bars: the document's
/// problems, or the loops' runtime errors with what the score declares.
pub fn check_diags(source: &str) -> Vec<Diag> {
    match crate::model::load(source) {
        Err(diags) => diags,
        Ok(loaded) => {
            let bar = loaded.doc.beats_per_bar();
            let mut composer = crate::compose::Composer::new(loaded);
            let mut events = Vec::new();
            composer.pump(bar * 8.0, &mut events);
            composer.errors()
        }
    }
}

/// `score_check`'s answer.
pub fn check(source: &str) -> Value {
    let loaded = match crate::model::load(source) {
        Ok(loaded) => loaded,
        Err(diags) => return json::obj(vec![("ok", Value::Bool(false)), ("problems", problems(&diags, source))]),
    };
    let doc = loaded.doc.clone();
    let bar = doc.beats_per_bar();
    let mut composer = crate::compose::Composer::new(loaded);
    let mut events = Vec::new();
    composer.pump(bar * 8.0, &mut events);
    let errors = composer.errors();
    let names = |list: Vec<String>| Value::Arr(list.into_iter().map(json::s).collect());
    json::obj(vec![
        ("ok", Value::Bool(errors.is_empty())),
        ("problems", problems(&errors, source)),
        ("label", json::s(doc.label.clone())),
        ("bpm", num(doc.clock.bpm)),
        ("params", names(doc.clock.params.iter().map(|(n, v)| format!("{n} = {v}")).collect())),
        ("sections", names(doc.sections.iter().map(|s| format!("{} ({} bars{})", s.name, s.bars, s.next.as_ref().map(|n| format!(", next {n}")).unwrap_or_default())).collect())),
        ("arrange", names(doc.arrange.clone())),
        ("stingers", names(doc.loops.iter().filter(|l| l.stinger.is_some()).map(|l| l.name.clone()).collect())),
        ("loops", names(doc.loops.iter().filter(|l| l.stinger.is_none()).map(|l| {
            let follow = l.follow.as_ref().map(|(p, a, b)| format!(" follows {p} {a}..{b}")).unwrap_or_default();
            format!("{}{follow}", l.name)
        }).collect())),
        ("notes_in_8_bars", Value::Int(events.len() as i64)),
    ])
}

fn num(value: f64) -> Value {
    Value::F64((value * 1000.0).round() / 1000.0)
}

/// `score_listen`'s answer for what was heard.
pub fn heard_json(heard: &Heard) -> Value {
    let level = |l: &Level| json::obj(vec![("peak_db", num(l.peak_db as f64)), ("rms_db", num(l.rms_db as f64))]);
    let texts = |list: &[String]| Value::Arr(list.iter().map(|t| json::s(t.clone())).collect());
    json::obj(vec![
        ("ok", Value::Bool(true)),
        ("from_bar", Value::Int(heard.from_bar as i64)),
        ("bars", Value::Int(heard.bars as i64)),
        ("bpm", num(heard.bpm)),
        ("master", level(&heard.master)),
        ("nodes", Value::Obj(heard.nodes.iter().map(|(n, l)| (n.clone(), level(l))).collect())),
        ("bands_db", Value::Obj(heard.bands.iter().map(|(hz, db)| (format!("{hz}"), num(*db as f64))).collect())),
        ("balance", texts(&heard.balance)),
        ("onsets_per_bar", texts(&heard.onsets)),
        ("notes", Value::Obj(heard.notes.iter().map(|(loop_name, notes)| {
            (loop_name.clone(), json::s(notes.iter().take(64).map(|(beat, name, vel)| format!("{}:{name}@{vel}", crate::source::fmt_num(*beat, 2))).collect::<Vec<_>>().join(" ")))
        }).collect())),
        ("per_bar", Value::Arr(heard.per_bar.iter().map(|(rms, bright)| json::obj(vec![("rms_db", num(*rms as f64)), ("bright_hz", num(*bright as f64))])).collect())),
        ("warnings", texts(&heard.warnings)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guide_check_and_listen_answer() {
        let guide = call("score_guide", &Value::Obj(Vec::new()), None).unwrap();
        let text = guide.get("guide").and_then(Value::as_str).unwrap();
        assert!(text.starts_with("# Writing a score"));
        let source = crate::templates::template("techno").unwrap();
        let args = json::obj(vec![("source", json::s(source))]);
        let checked = call("score_check", &args, None).unwrap();
        assert_eq!(checked.get("ok").and_then(Value::as_bool), Some(true), "{}", checked.to_json());
        // The open score stands in for a missing source.
        let open = call("score_check", &Value::Obj(Vec::new()), Some(source)).unwrap();
        assert_eq!(open.to_json(), checked.to_json());
        assert!(call("score_check", &Value::Obj(Vec::new()), None).is_err());
        let broken = call("score_check", &json::obj(vec![("source", json::s("use mod.score.*\nlet x = Synth{preset: \n"))]), None).unwrap();
        assert_eq!(broken.get("ok").and_then(Value::as_bool), Some(false));
        assert!(broken.get("problems").and_then(Value::as_arr).is_some_and(|p| !p.is_empty()));
        let heard = call("score_listen", &json::obj(vec![("source", json::s(source)), ("bars", Value::Int(1))]), None).unwrap();
        assert_eq!(heard.get("ok").and_then(Value::as_bool), Some(true), "{}", heard.to_json());
        assert!(heard.get("master").is_some() && heard.get("bands_db").is_some());
    }
}
