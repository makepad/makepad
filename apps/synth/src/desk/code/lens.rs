//! What each node's lens shows, made once per change of the score (never
//! per frame): its kind, its knobs and the block it heads. Knobs come from
//! the field tables the panels and the AI share (`makepad_synth_panel::spec`):
//! a node type's numeric fields in the table's order, those written in the
//! source first, so a new component's lens follows its table with no code
//! here; a field no table knows gets a knob around its value.

use super::folds::Folds;
use makepad_look::palette::Palette;
use makepad_score_live::model::{InstKind, ScoreDoc};
use makepad_score_live::source::{self, Outline};
use makepad_score_player::FxKind;
use makepad_synth_panel::spec::{self, ParamSpec, Taper, Unit};

/// Rows of editor space a lens takes.
pub const LENS_ROWS: f64 = 3.0;
/// Knobs a lens offers at most (fewer when it is narrow).
pub const MAX_KNOBS: usize = 6;

/// A knob on a lens: the field it writes, its table entry, its value now.
#[derive(Clone, Debug, PartialEq)]
pub struct Knob {
    pub field: String,
    pub spec: ParamSpec,
    pub value: f64,
}

impl Knob {
    pub fn norm(&self) -> f64 {
        self.spec.norm(self.value)
    }

    /// Its short label: the table's, else the field's name.
    pub fn label(&self) -> String {
        let label = if self.spec.label.is_empty() { self.field.to_uppercase() } else { self.spec.label.to_string() };
        label.chars().take(7).collect()
    }

    /// The value as a person reads it, kept short for under a knob.
    pub fn readout(&self) -> String {
        let v = self.value;
        match self.spec.unit {
            Unit::Hz if v >= 1000.0 => format!("{:.1}k", v / 1000.0),
            Unit::Hz => format!("{v:.0}"),
            Unit::Secs if v < 1.0 => format!("{:.0}ms", v * 1000.0),
            Unit::Secs => format!("{v:.2}s"),
            Unit::Ms => format!("{v:.0}ms"),
            Unit::Percent => format!("{:.0}%", v * 100.0),
            Unit::Db => format!("{v:+.1}"),
            Unit::Cents => format!("{v:+.0}c"),
            _ if self.spec.taper == Taper::Int => format!("{v:.0}"),
            _ if v.abs() >= 100.0 => format!("{v:.0}"),
            _ => format!("{v:.2}"),
        }
    }
}

/// A `hits:` pattern: where its characters are in the source.
#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    pub text: String,
    /// Byte offset of the first character inside the quotes.
    pub at: usize,
}

/// What a lens draws besides its knobs.
#[derive(Clone, Debug, PartialEq)]
pub enum LensKind {
    Clock { bpm: f64, beats_per_bar: f64 },
    Loop { key: u64, mute: bool, solo: bool, beats: Option<f64>, pattern: Option<Pattern>, plays: Option<String> },
    /// A synth: its first layer's envelope and filter.
    Synth { label: String, adsr: [f32; 4], cutoff: f32, res: f32, filter: u8 },
    /// Any other instrument (a new component too): what it is now.
    Inst { label: String },
    Fx { key: u64, kind: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lens {
    pub node: String,
    pub ty: String,
    /// The 0-based lines of the node's `let` and its last line.
    pub line: usize,
    pub last: usize,
    pub kind: LensKind,
    pub knobs: Vec<Knob>,
    /// Its block's code is folded (its first line and the lens show).
    pub code_folded: bool,
    /// Its colour: the category's, a loop the instrument's it plays.
    pub hue: u32,
}

/// The lenses of a score, and the blocks whose lens is folded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lenses {
    pub lenses: Vec<Lens>,
    /// (node, 0-based line of its `let`, its hue).
    pub folded: Vec<(String, usize, u32)>,
}

/// The calm category colours: tempo, instruments, effects, and the loops
/// in the colour of what they play.
pub fn hue_of(p: &Palette, ty: &str, index: usize) -> u32 {
    match ty {
        "Clock" => p.amber,
        "Loop" | "Stinger" => [p.info, p.teal, p.accent_soft, p.link, p.warm, p.learn][index % 6],
        _ if FxKind::parse(ty).is_some() => p.secondary,
        _ => [p.info, p.teal, p.accent_soft, p.link, p.warm, p.learn][index % 6],
    }
}

fn instrument_label(doc: &ScoreDoc, name: &str, ty: &str) -> String {
    match doc.inst(name).map(|i| &i.kind) {
        Some(InstKind::Drums { kit, .. }) => format!("kit {kit}"),
        Some(InstKind::Ironfish { program, .. }) => format!("ironfish {}", makepad_score_player::presets::IRON_PROGRAMS.get(*program).copied().unwrap_or("init")),
        Some(InstKind::Shader { shader, .. }) => format!("shader {}", shader.lib.clone().unwrap_or_else(|| "(own code)".into())),
        Some(InstKind::Voice { profile, singers, .. }) => {
            if *singers > 1 {
                format!("{profile} choir of {singers}")
            } else {
                format!("voice {profile}")
            }
        }
        Some(InstKind::Sampler { src, .. }) => format!("sample {}", src.id.chars().take(14).collect::<String>()),
        Some(InstKind::Native { native, preset, program, .. }) => match native.as_str() {
            "piano" => format!("piano {}", preset.clone().unwrap_or_else(|| "concert grand".into())),
            "drums" => "acoustic kit".into(),
            _ => format!("{} {}", program, makepad_score_live::instruments::GM_PROGRAMS.get(*program as usize).copied().unwrap_or("")),
        },
        Some(InstKind::Input { .. }) => "audio input".into(),
        Some(InstKind::Midi { channel, port }) => format!("midi {port} ch {channel}"),
        _ => ty.to_lowercase(),
    }
}

/// A field's value as written (a number, or `true`/`false`).
fn written(source: &str, outline: &Outline, node: &str, field: &str) -> Option<f64> {
    let text = outline.node(node)?.field(field)?.value.text(source).trim();
    match text {
        "true" => Some(1.0),
        "false" => Some(0.0),
        t => t.parse::<f64>().ok(),
    }
}

/// The knobs of a node of type `ty`: the table's numeric fields, written
/// ones first, then the rest in the table's order, `MAX_KNOBS` at most;
/// fields the table does not know but the source writes get a knob too.
/// `known` reads a field's value from the loaded score when it is not
/// written (a synth's preset, an effect's defaults).
fn knobs(source: &str, outline: &Outline, node: &str, ty: &str, skip: &[&str], known: &dyn Fn(&str) -> Option<f64>) -> Vec<Knob> {
    let numeric = |s: &ParamSpec| !s.is_choice() && s.unit != Unit::Bool;
    let mut keys: Vec<String> = spec::keys(ty).into_iter().filter(|k| !skip.contains(k)).map(str::to_string).collect();
    // Fields the source writes that the table does not list.
    if let Some(ctor) = outline.node(node) {
        for f in &ctor.fields {
            if !keys.contains(&f.key) && !skip.contains(&f.key.as_str()) && written(source, outline, node, &f.key).is_some() && f.key != "play" && f.key != "at" {
                keys.push(f.key.clone());
            }
        }
    }
    let mut made: Vec<(bool, Knob)> = keys
        .into_iter()
        .filter_map(|key| {
            let value = written(source, outline, node, &key);
            let spec = spec::spec_or_generic(ty, &key, value.or_else(|| known(&key)));
            if !numeric(&spec) {
                return None;
            }
            let v = value.or_else(|| known(&key)).unwrap_or(spec.default);
            Some((value.is_some(), Knob { field: key, spec, value: v }))
        })
        .collect();
    // Written first, the table's order kept within each.
    made.sort_by_key(|(written, _)| !*written);
    made.into_iter().take(MAX_KNOBS).map(|(_, k)| k).collect()
}

/// The lenses for `doc`, whose source is `source` (outline `outline`).
pub fn build(doc: &ScoreDoc, source: &str, outline: &Outline, folds: &Folds, p: &Palette) -> Lenses {
    let mut out = Lenses::default();
    let line_of = |at: usize| source::line_col(source, at).0 - 1;
    // An instrument's hue, so a loop wears the colour of what it plays.
    let mut inst_hue: Vec<(String, u32)> = Vec::new();
    let mut inst_count = 0;
    let mut loop_count = 0;
    for (name, ty) in &doc.order {
        if doc.inst(name).is_some() {
            inst_hue.push((name.clone(), hue_of(p, ty, inst_count)));
            inst_count += 1;
        }
    }
    for (name, ty) in &doc.order {
        let Some(ctor) = outline.node(name) else { continue };
        let (line, last) = (line_of(ctor.span.start), line_of(ctor.span.end.saturating_sub(1)));
        let (kind, knobs, hue) = match ty.as_str() {
            "Clock" => {
                let clock = &doc.clock;
                let known = |k: &str| match k {
                    "bpm" => Some(clock.bpm),
                    "swing" => Some(clock.swing),
                    _ => None,
                };
                (LensKind::Clock { bpm: clock.bpm, beats_per_bar: doc.beats_per_bar() }, knobs(source, outline, name, "Clock", &["seed", "sig"], &known), hue_of(p, "Clock", 0))
            }
            "Loop" | "Stinger" => {
                let Some(l) = doc.loop_def(name) else { continue };
                let pattern = ctor.field("hits").filter(|f| f.value.text(source).starts_with('"')).map(|f| Pattern {
                    text: f.value.text(source).trim_matches('"').to_string(),
                    at: f.value.start + 1,
                });
                let hue = l.play.as_ref().and_then(|play| inst_hue.iter().find(|(n, _)| n == play).map(|(_, h)| *h)).unwrap_or_else(|| hue_of(p, "Loop", loop_count));
                loop_count += 1;
                let (amp, pan) = (l.amp as f64, l.pan as f64);
                let known = move |k: &str| match k {
                    "amp" => Some(amp),
                    "pan" => Some(pan),
                    _ => None,
                };
                let kind = LensKind::Loop { key: makepad_score_live::key(name), mute: l.mute, solo: l.solo, beats: l.beats, pattern, plays: l.play.clone() };
                (kind, knobs(source, outline, name, "Loop", &["beats", "mute", "solo", "hits", "code", "swing", "dur"], &known), hue)
            }
            other => {
                let hue = inst_hue.iter().find(|(n, _)| n == name).map(|(_, h)| *h);
                if let Some(kind) = FxKind::parse(other) {
                    let fx = doc.fx.iter().find(|f| &f.name == name).cloned();
                    let known = move |k: &str| {
                        let i = kind.params().iter().position(|(param, ..)| *param == k)?;
                        fx.as_ref().map(|f| f.params[i] as f64)
                    };
                    (LensKind::Fx { key: makepad_score_live::key(name), kind: kind.name().to_string() }, knobs(source, outline, name, other, &["in", "side"], &known), hue_of(p, other, 0))
                } else if let Some(hue) = hue {
                    match doc.inst(name).map(|i| &i.kind) {
                        Some(InstKind::Synth { recipe, .. }) => {
                            let layer = recipe.layers().first().copied().unwrap_or_default();
                            let filter = match layer.filter {
                                makepad_audio_synth::dsp::FilterMode::Off => 0,
                                makepad_audio_synth::dsp::FilterMode::Highpass => 2,
                                makepad_audio_synth::dsp::FilterMode::Bandpass => 3,
                                _ => 1,
                            };
                            let label = ctor.field("preset").map(|f| f.value.text(source).trim_start_matches('@').to_string()).unwrap_or_else(|| "synth".into());
                            let known = move |k: &str| {
                                Some(match k {
                                    "cutoff" => layer.cutoff,
                                    "res" | "q" => layer.q,
                                    "drive" => layer.drive,
                                    "attack" => layer.attack,
                                    "decay" => layer.decay,
                                    "sustain" => layer.sustain,
                                    "release" => layer.release,
                                    "hold" => layer.hold,
                                    "detune" => layer.detune_cents,
                                    "gain" | "volume" => layer.gain,
                                    _ => return None,
                                } as f64)
                            };
                            let kind = LensKind::Synth { label, adsr: [layer.attack, layer.decay, layer.sustain, layer.release], cutoff: layer.cutoff, res: layer.q, filter };
                            (kind, knobs(source, outline, name, "Synth", &[], &known), hue)
                        }
                        _ => {
                            let known = |_: &str| None;
                            (LensKind::Inst { label: instrument_label(doc, name, other) }, knobs(source, outline, name, other, &[], &known), hue)
                        }
                    }
                } else {
                    continue;
                }
            }
        };
        if folds.lens(name) {
            out.folded.push((name.clone(), line, hue));
            continue;
        }
        out.lenses.push(Lens { node: name.clone(), ty: ty.clone(), line, last, kind, knobs, code_folded: folds.code(name), hue });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_node_gets_a_lens_with_knobs_from_the_tables() {
        let src = makepad_score_live::templates::template("techno").expect("the techno template");
        let doc = ScoreDoc::load(src).expect("loads");
        let outline = source::outline(src);
        let lenses = build(&doc, src, &outline, &Folds::default(), &Palette::HOUSE);
        assert!(lenses.lenses.iter().any(|l| matches!(l.kind, LensKind::Clock { .. })));
        assert!(lenses.lenses.iter().any(|l| matches!(l.kind, LensKind::Loop { .. })));
        for lens in &lenses.lenses {
            assert!(lens.knobs.len() <= MAX_KNOBS);
            assert!(lens.line <= lens.last, "{}", lens.node);
            for knob in &lens.knobs {
                assert!(knob.value.is_finite(), "{}.{}", lens.node, knob.field);
            }
        }
        let clock = lenses.lenses.iter().find(|l| matches!(l.kind, LensKind::Clock { .. })).unwrap();
        assert!(clock.knobs.iter().any(|k| k.field == "bpm"));
    }

    #[test]
    fn a_written_field_comes_first() {
        let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @acid release: 0.4}\nlet l = Loop{play: s hits: \"x...\"}\nScore{clock s l}\n";
        let doc = ScoreDoc::load(src).expect("loads");
        let outline = source::outline(src);
        let lenses = build(&doc, src, &outline, &Folds::default(), &Palette::HOUSE);
        let synth = lenses.lenses.iter().find(|l| l.node == "s").expect("the synth's lens");
        assert_eq!(synth.knobs[0].field, "release");
        assert!((synth.knobs[0].value - 0.4).abs() < 1e-9);
        let lp = lenses.lenses.iter().find(|l| l.node == "l").expect("the loop's lens");
        assert_eq!(lp.hue, synth.hue, "a loop wears its instrument's colour");
        match &lp.kind {
            LensKind::Loop { pattern: Some(p), .. } => assert_eq!(&src[p.at..p.at + 4], "x..."),
            other => panic!("{other:?}"),
        }
    }
}
