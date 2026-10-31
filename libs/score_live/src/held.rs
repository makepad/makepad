//! Settings held live: a control on a score's field (an editor panel's
//! knob under a finger) sounding before the source is written. A field
//! held lands where the playing graph reads it, without a new revision:
//!
//! - a Synth's numeric field (`mono.cutoff`): the recipe key it sets
//!   ([`crate::model::SYNTH_KEYS`]) on the patch of every loop that plays
//!   it, from their next notes (`feed::live_params`);
//! - a Shader instrument's param (`acid_dark.cutoff`): the param of every
//!   loop's audio shader, at once on the notes playing;
//! - a Loop's `amp`/`pan`/`mute`/`solo` and an FX's settings: their
//!   controls in the graph's control bank.
//!
//! Anything else (a choice, a count, code) is not held: it sounds when the
//! source is written. The release writes the source once; the revision it
//! makes plays the same value, then the held values are let go.

use crate::model::{InstKind, ScoreDoc, SYNTH_KEYS};

/// Where a held field lands.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A loop's live instrument settings (`<instance>/<loop>`), by key.
    Params { node: String, key: String },
    /// A graph control: `<instance>/<node>`'s `param`.
    Control { node: String, param: String },
}

/// Where `node.field` (a top-level field of a score node) lands when held,
/// or nothing when it can only be written.
pub fn targets(doc: &ScoreDoc, node: &str, field: &str) -> Vec<Target> {
    let playing = |inst: &str| -> Vec<String> { doc.loops.iter().filter(|l| l.play.as_deref() == Some(inst)).map(|l| l.name.clone()).collect() };
    if let Some(inst) = doc.inst(node) {
        return match &inst.kind {
            InstKind::Synth { .. } => match SYNTH_KEYS.iter().find(|(f, _)| *f == field) {
                Some((_, key)) => playing(node).into_iter().map(|l| Target::Params { node: l, key: key.to_string() }).collect(),
                None => Vec::new(),
            },
            InstKind::Shader { shader, .. } if shader.params.iter().any(|(name, _)| name == field) => {
                playing(node).into_iter().map(|l| Target::Params { node: l, key: field.to_string() }).collect()
            }
            _ => Vec::new(),
        };
    }
    if doc.loop_def(node).is_some() && matches!(field, "amp" | "pan" | "mute" | "solo") {
        return vec![Target::Control { node: node.to_string(), param: field.to_string() }];
    }
    if let Some(fx) = doc.fx.iter().find(|f| f.name == node) {
        if fx.shader.is_none() && fx.kind.param_index(field).is_some() {
            return vec![Target::Control { node: node.to_string(), param: field.to_string() }];
        }
    }
    Vec::new()
}

/// A path an editor binds (`mono.cutoff`, `bass.amp`) as (node, field)
/// when it names a top-level field of a score node.
pub fn node_field(path: &str) -> Option<(&str, &str)> {
    let (node, field) = path.split_once('.')?;
    (!field.contains(['.', '['])).then_some((node, field))
}

/// A held value in an offline render: from `at` seconds, `node.field`
/// plays `value` (what a person's control did at that moment).
#[derive(Clone, Debug, PartialEq)]
pub struct Held {
    pub at: f64,
    pub node: String,
    pub field: String,
    pub value: f32,
}
