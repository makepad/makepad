//! What each node's lens shows, made once per change of the score (never
//! per frame). Every card is the same frame (a title column, a body, the
//! tools); the body is what the node is, laid out like a hardware panel:
//!
//! - an instrument is a row of modules in signal-flow order, each a titled
//!   group with its picture and its own knobs: a Synth's oscillator,
//!   filter (its curve), envelope (its ADSR), modulation and output; an
//!   engine with a parameter schema (Morph, and any engine that declares
//!   one) gets the same from its schema's groups, so a new engine's lens
//!   needs no code here; a drum kit is a pad per voice;
//! - a clock is its tempo, signature, swing and the beats of the bar;
//! - a loop is its steps or its roll, its level and pan;
//! - an effect is a picture of what it does, and its knobs.
//!
//! Knobs come from the field tables the panels and the AI share
//! (`makepad_synth_panel::spec`). Their order is fixed per module (the
//! table's, or the module's own: attack, decay, sustain, release), never the
//! order the source happens to write them in.

use super::folds::Folds;
use makepad_look::palette::Palette;
use makepad_score_live::model::{InstKind, ScoreDoc, StreamKind};
use makepad_score_live::source::{self, Outline};
use makepad_score_player::FxKind;
use makepad_edit_values::Taper;
use makepad_synth_panel::spec::{self, ParamSpec, Unit};

/// Rows of editor space a lens takes (every kind the same height).
pub const LENS_ROWS: f64 = 4.0;
/// Knobs a generic module offers at most.
pub const MAX_KNOBS: usize = 6;

/// A knob on a lens: the field it writes, its table entry, its value now.
#[derive(Clone, Debug, PartialEq)]
pub struct Knob {
    pub field: String,
    /// A map field's entry (a drum's `gain: {kick: 0.8}`): the knob writes
    /// `field: {entry: value}`.
    pub entry: Option<String>,
    pub spec: ParamSpec,
    pub value: f64,
    /// What it does, in plain words (its tooltip).
    pub tip: String,
}

impl Knob {
    pub fn norm(&self) -> f64 {
        self.spec.norm(self.value)
    }

    /// Its short label: the table's, else the field's name.
    pub fn label(&self) -> String {
        let label = if self.spec.label.is_empty() { self.field.to_uppercase() } else { self.spec.label.to_uppercase() };
        label.chars().take(8).collect()
    }

    /// The value as a person reads it, kept short for under a knob.
    pub fn readout(&self) -> String {
        let v = self.value;
        if self.spec.is_choice() {
            // An analog slot type reads by its wiring: the prefix would fill the room.
            return self.spec.options.get(v.round().max(0.0) as usize).map_or(String::new(), |o| match o.strip_prefix("analog_") {
                Some(w) => format!("an {w}").chars().take(8).collect(),
                None => o.chars().take(7).collect(),
            });
        }
        match self.spec.unit {
            Unit::Hz if v >= 1000.0 => format!("{:.1}k", v / 1000.0),
            Unit::Hz => format!("{v:.0}"),
            Unit::Secs if v < 1.0 => format!("{:.0}ms", v * 1000.0),
            Unit::Secs => format!("{v:.2}s"),
            Unit::Ms => format!("{v:.0}ms"),
            Unit::Percent => format!("{:.0}%", v * 100.0),
            Unit::Db => format!("{v:+.1}"),
            Unit::Cents => format!("{v:+.0}c"),
            Unit::Semis => format!("{v:+.1}st"),
            Unit::Bool => if v > 0.5 { "on" } else { "off" }.to_string(),
            Unit::Note => self.spec.display(v),
            _ if self.spec.taper == Taper::Int => format!("{v:.0}"),
            _ if v.abs() >= 100.0 => format!("{v:.0}"),
            _ => format!("{v:.2}"),
        }
    }

    /// The value written as the source takes it.
    pub fn text_of(&self, value: f64) -> String {
        self.spec.source_text(value)
    }
}

/// A `hits:` pattern: where its characters are in the source.
#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    pub text: String,
    /// Byte offset of the first character inside the quotes.
    pub at: usize,
}

/// A module's picture, and which of its knobs (indices into the module's
/// knobs) its handles move.
#[derive(Clone, Debug, PartialEq)]
pub enum Graph {
    None,
    /// The envelope: the attack point moves the attack time; the decay
    /// point the decay time and the sustain level; the release point the
    /// release time.
    Env { attack: usize, decay: usize, sustain: usize, release: usize },
    /// The filter's response: its handle moves the cutoff (across) and the
    /// resonance (up), the wheel the drive. `mode` 0 off, 1 lowpass, 2
    /// highpass, 3 bandpass; `q_unit` the resonance is a Q (else 0..1).
    Filter { cutoff: usize, res: Option<usize>, drive: Option<usize>, mode: u8, q_unit: bool },
    /// An oscillator's wave: 0 sine, 1 triangle, 2 saw, 3 square, 4 pulse,
    /// 5 noise, 6 FM, 7 string, 8 a wavetable (`pos` the knob of where in it);
    /// plus 10: drawn only (the mix of outputs has its own knobs, a click on
    /// it picks nothing).
    Wave { shape: u8, pos: Option<usize> },
    /// An LFO's shape (as its options name it) at its rate.
    Lfo { shape: String, rate: Option<usize> },
}

/// One module of an instrument: a titled group, its picture, its knobs.
#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub title: String,
    /// What it is, for its tooltip.
    pub tip: String,
    pub graph: Graph,
    pub knobs: Vec<Knob>,
    /// How much it matters when the card is narrow (0 kept last).
    pub rank: u8,
    /// Chips by its title: what kind of module it is.
    pub picks: Vec<ModulePick>,
}

/// A chip that sets a choice: its label now, what a click writes.
#[derive(Clone, Debug, PartialEq)]
pub struct ModulePick {
    pub field: &'static str,
    pub label: String,
    pub value: String,
    pub tip: String,
}

/// A drum kit's voice: a pad.
#[derive(Clone, Debug, PartialEq)]
pub struct Pad {
    pub name: &'static str,
    pub slot: usize,
    /// The kit has a sound there.
    pub filled: bool,
    pub level: Knob,
    pub tune: Knob,
}

/// What a lens draws besides its frame.
#[derive(Clone, Debug, PartialEq)]
pub enum LensKind {
    Clock { tempo: Knob, sig: (u32, u32), beats_per_bar: f64, tempo_follows: bool },
    Loop { key: u64, mute: bool, solo: bool, beats: Option<f64>, pattern: Option<Pattern>, plays: Option<String> },
    /// An instrument: what it is (its picker), its modules in signal-flow
    /// order, and a kit's pads.
    Inst { label: String, engine: String, modules: Vec<Module>, pads: Vec<Pad> },
    Fx { key: u64, kind: String },
    /// MIDI in, or a stream node shaping what it plays.
    Stream { card: StreamCard },
}

/// What a stream node's card shows besides its knobs. `views` are the
/// loop nodes (`<slot>/<loop>`) its stream reaches, whose live state
/// (`makepad_audio_engine::feed::streams`) the card reads as it paints.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamCard {
    /// A keyboard: its device (a picker over the inputs) and channel, an
    /// activity light and the last notes it played.
    MidiIn { device: String, channel: u8 },
    /// An arpeggiator: its pattern (a picker) and the steps it plays (the
    /// held chord's, else the pattern on a C major triad).
    Arp { pattern: usize, preview: Vec<u8>, views: Vec<String> },
    /// A chord, a transpose or a velocity curve: what it does in words.
    Shape { label: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lens {
    pub node: String,
    pub ty: String,
    /// The 0-based lines of the node's `let` and its last line.
    pub line: usize,
    pub last: usize,
    pub kind: LensKind,
    /// Knobs outside modules (a clock's swing, a loop's level and pan, an
    /// effect's settings).
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

pub(in crate::desk) fn instrument_label(doc: &ScoreDoc, source: &str, outline: &Outline, name: &str, ty: &str) -> String {
    let preset = || outline.node(name).and_then(|n| n.field("preset")).map(|f| f.value.text(source).trim_start_matches('@').trim_matches('"').replace('_', " "));
    match doc.inst(name).map(|i| &i.kind) {
        Some(InstKind::Synth { .. }) => preset().unwrap_or_else(|| "own synth".into()),
        Some(InstKind::Morph { .. }) => preset().unwrap_or_else(|| "init".into()),
        Some(InstKind::Tandem { patch, .. }) => patch.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("init").to_string(),
        Some(InstKind::Drums { kit, .. }) => kit.replace('_', " "),
        Some(InstKind::Ironfish { program, .. }) => makepad_score_player::presets::IRON_PROGRAMS.get(*program).copied().unwrap_or("init").to_string(),
        Some(InstKind::Shader { shader, .. }) => shader.lib.clone().unwrap_or_else(|| "own code".into()).replace('_', " "),
        Some(InstKind::Voice { profile, singers, neural, .. }) => {
            if *neural {
                "neural singer".into()
            } else if *singers > 1 {
                format!("{profile} choir of {singers}")
            } else {
                profile.clone()
            }
        }
        Some(InstKind::Sampler { src, .. }) => src.id.chars().take(14).collect::<String>(),
        Some(InstKind::Native { native, preset, program, bank, .. }) => match native.as_str() {
            "piano" => preset.clone().unwrap_or_else(|| "concert grand".into()).to_lowercase(),
            "drums" => "acoustic kit".into(),
            _ if *bank == 128 => format!("GM kit {program}"),
            _ => makepad_score_live::instruments::GM_PROGRAMS.get(*program as usize).copied().unwrap_or("").to_lowercase(),
        },
        Some(InstKind::Input { .. }) => "audio input".into(),
        Some(InstKind::Midi { channel, port }) => if port.is_empty() { format!("MIDI ch {channel}") } else { format!("{port} ch {channel}") },
        _ => ty.to_lowercase(),
    }
}

/// The engine an instrument plays on, as the picker's chip names it.
fn engine_label(doc: &ScoreDoc, name: &str, ty: &str) -> String {
    match doc.inst(name).map(|i| &i.kind) {
        Some(InstKind::Native { native, bank, .. }) => match native.as_str() {
            "piano" => "Piano".into(),
            "drums" => "Drums".into(),
            _ if *bank == 128 => "GM kit".into(),
            _ => "GM".into(),
        },
        Some(InstKind::Input { .. }) => "Input".into(),
        Some(InstKind::Midi { .. }) => "MIDI".into(),
        _ => ty.to_string(),
    }
}

/// A field's value as written (a number, or `true`/`false`, or `@option`
/// of a choice).
fn written(source: &str, outline: &Outline, node: &str, field: &str, spec: Option<&ParamSpec>) -> Option<f64> {
    let text = outline.node(node)?.field(field)?.value.text(source).trim();
    match text {
        "true" => Some(1.0),
        "false" => Some(0.0),
        t if t.starts_with('@') => spec.and_then(|s| s.options.iter().position(|o| o == &t[1..])).map(|i| i as f64),
        t => t.parse::<f64>().ok(),
    }
}

/// A plain-words line for a knob: what it does, by its name.
fn tip_of(ty: &str, field: &str, spec: &ParamSpec) -> String {
    let suffix = field.rsplit_once('_').map_or(field, |(_, s)| s);
    let plain = match suffix {
        "attack" => "how fast the sound starts",
        "decay" => "how fast it falls to the sustain level",
        "sustain" => "the level held while the note is down",
        "release" => "how long it fades after the note ends",
        "hold" => "how long it stays at the top before the decay",
        "cutoff" => "where the filter starts to cut",
        "res" | "q" => "the peak at the cutoff (resonance)",
        "drive" => "how hard the sound is pushed into saturation",
        "detune" => "how far the voices are pulled apart, in cents",
        "gain" | "level" | "volume" => "its output level",
        "amp" => "the loop's level",
        "pan" => "where it sits, left to right",
        "swing" => "how late the off-beat sixteenths fall",
        "bpm" => "the tempo, in beats a minute",
        "mix" => "how much of the effect is heard",
        "unison" => "how many voices play each note",
        "pos" => "where in the wavetable it reads",
        "rate" => "how fast it moves",
        "glide" => "how long a note slides to the next",
        _ => "",
    };
    let label = if spec.label.is_empty() { field.to_string() } else { spec.label.to_lowercase() };
    if !plain.is_empty() {
        return format!("{label}: {plain}");
    }
    if !spec.about.is_empty() {
        return format!("{label}: {}", spec.about);
    }
    match makepad_score_live::assist::field_doc(ty, field) {
        Some(doc) => format!("{label}: {doc}"),
        None => label,
    }
}

/// One knob for `field` of a node of type `ty`: its value as written,
/// else as the loaded score plays it (`known`), else the table's default.
fn knob(source: &str, outline: &Outline, node: &str, ty: &str, field: &str, known: &dyn Fn(&str) -> Option<f64>) -> Knob {
    let table = spec::spec(ty, field);
    let value = written(source, outline, node, field, table.as_ref());
    let spec = table.unwrap_or_else(|| ParamSpec::generic(field, value.or_else(|| known(field))));
    let v = value.or_else(|| known(field)).unwrap_or(spec.default);
    Knob { field: field.to_string(), entry: None, tip: tip_of(ty, field, &spec), spec, value: v }
}

/// The knobs of a node of type `ty` that are not in modules: the table's
/// numeric fields in the table's order, the written ones and those in
/// `always` (`MAX_KNOBS` at most); fields the table does not know but the
/// source writes get a knob too, after them.
fn loose_knobs(source: &str, outline: &Outline, node: &str, ty: &str, skip: &[&str], always: &[&str], known: &dyn Fn(&str) -> Option<f64>) -> Vec<Knob> {
    let numeric = |s: &ParamSpec| !s.is_choice() && s.unit != Unit::Bool;
    let mut keys: Vec<String> = spec::keys(ty).into_iter().filter(|k| !skip.contains(k)).map(str::to_string).collect();
    if let Some(ctor) = outline.node(node) {
        for f in &ctor.fields {
            if !keys.contains(&f.key) && !skip.contains(&f.key.as_str()) && written(source, outline, node, &f.key, None).is_some() && f.key != "play" && f.key != "at" {
                keys.push(f.key.clone());
            }
        }
    }
    let wanted = |key: &str| always.contains(&key) || outline.node(node).and_then(|n| n.field(key)).is_some();
    let mut made: Vec<Knob> = keys.iter().filter(|k| wanted(k)).map(|k| knob(source, outline, node, ty, k, known)).filter(|k| numeric(&k.spec)).collect();
    // Room left: the table's other fields, in its order.
    if made.len() < MAX_KNOBS.min(always.len().max(1)) {
        for k in keys.iter().filter(|k| !wanted(k)) {
            if made.len() >= MAX_KNOBS.min(always.len().max(1)) {
                break;
            }
            let k = knob(source, outline, node, ty, k, known);
            if numeric(&k.spec) {
                made.push(k);
            }
        }
    }
    made.truncate(MAX_KNOBS);
    made
}

/// A Synth's modules, in signal-flow order, each with its own fixed knobs.
fn synth_modules(source: &str, outline: &Outline, node: &str, layer: &makepad_audio_synth::recipe::Layer) -> Vec<Module> {
    use makepad_audio_synth::dsp::{FilterMode, Wave};
    use makepad_audio_synth::recipe::Source;
    let l = *layer;
    let known = move |k: &str| -> Option<f64> {
        Some(match k {
            "cutoff" => l.cutoff,
            "res" | "q" => l.q,
            "drive" => l.drive,
            "attack" => l.attack,
            "decay" => l.decay,
            "sustain" => l.sustain,
            "release" => l.release,
            "hold" => l.hold.max(0.0),
            "detune" => l.detune_cents,
            // The node's level over its preset's (the recipe's own is apart).
            "gain" | "volume" => 1.0,
            "vib" | "vibrato" => l.vib_hz,
            "vib_depth" => l.vib_depth,
            "fm" | "ratio" => match l.source {
                Source::Fm { ratio, .. } => ratio,
                _ => return None,
            },
            "index" => l.fm_index,
            "pluck" => match l.source {
                Source::Pluck { damping } => damping,
                _ => return None,
            },
            "width" | "pw" => l.width,
            "cutoff_to" if l.cutoff_to.is_finite() => l.cutoff_to,
            _ => return None,
        } as f64)
    };
    let k = |field: &str| knob(source, outline, node, "Synth", field, &known);
    let has = |field: &str| outline.node(node).and_then(|n| n.field(field)).is_some();
    let wave = |w: Wave| match w {
        Wave::Sine => 0,
        Wave::Triangle => 1,
        Wave::Saw | Wave::SoftSaw => 2,
        Wave::Square => 3,
        Wave::Pulse => 4,
    };
    let (shape, osc_title) = match l.source {
        Source::Osc(w) => (wave(w), "OSC"),
        Source::Noise(_) => (5, "NOISE"),
        Source::Fm { .. } => (6, "FM"),
        Source::Pluck { .. } => (7, "STRING"),
        Source::None => (2, "OSC"),
    };
    let mut osc_knobs = vec![k("detune")];
    match l.source {
        Source::Fm { .. } => {
            osc_knobs.push(k("fm"));
            osc_knobs.push(k("index"));
        }
        Source::Pluck { .. } => osc_knobs.push(k("pluck")),
        Source::Osc(Wave::Pulse) => osc_knobs.push(k("width")),
        _ => {}
    }
    let mut modules = vec![Module {
        title: osc_title.into(),
        tip: "the oscillator: the wave the sound starts from (a click on the wave picks the next one)".into(),
        graph: Graph::Wave { shape, pos: None },
        knobs: osc_knobs,
        rank: 2,
        picks: Vec::new(),
    }];
    let mode = match l.filter {
        FilterMode::Off => 0,
        FilterMode::Highpass => 2,
        FilterMode::Bandpass => 3,
        _ => 1,
    };
    modules.push(Module {
        title: match mode {
            2 => "FILTER HP",
            3 => "FILTER BP",
            0 => "FILTER OFF",
            _ => "FILTER LP",
        }
        .into(),
        tip: "the filter: drag the handle across for the cutoff, up for the resonance; the wheel over it sets the drive".into(),
        graph: Graph::Filter { cutoff: 0, res: Some(1), drive: Some(2), mode, q_unit: true },
        knobs: vec![k("cutoff"), k("res"), k("drive")],
        rank: 4,
        picks: Vec::new(),
    });
    modules.push(Module {
        title: "AMP ENV".into(),
        tip: "the amp envelope: drag its points (attack; decay and sustain; release)".into(),
        graph: Graph::Env { attack: 0, decay: 1, sustain: 2, release: 3 },
        knobs: vec![k("attack"), k("decay"), k("sustain"), k("release")],
        rank: 5,
        picks: Vec::new(),
    });
    // Modulation: only what this synth uses or the source writes.
    let mut mods = Vec::new();
    if has("vib") || has("vibrato") || has("vib_depth") || l.vib_depth > 0.0 {
        mods.push(k("vib"));
        mods.push(k("vib_depth"));
    }
    if has("cutoff_to") || has("sweep") {
        mods.push(k("cutoff_to"));
        mods.push(k("sweep"));
    }
    if !mods.is_empty() {
        modules.push(Module { title: "MOD".into(), tip: "modulation: vibrato and the filter's sweep".into(), graph: Graph::None, knobs: mods, rank: 1, picks: Vec::new() });
    }
    modules.push(Module { title: "OUT".into(), tip: "the output level".into(), graph: Graph::None, knobs: vec![k("gain")], rank: 3, picks: Vec::new() });
    modules
}

/// What a schema group is, from its name (`osc_a`, `filter1`, `env2`,
/// `lfo3`, `fx_delay`): its title, its picture kind and the knob suffixes
/// it shows first, in their fixed order.
fn group_shape(group: &str) -> (String, &'static str, &'static [&'static str]) {
    let num = |prefix: &str| group.strip_prefix(prefix).map(|n| n.trim_start_matches('_').to_uppercase());
    if let Some(n) = num("osc") {
        return (format!("OSC {n}"), "osc", &["pos", "level", "unison", "detune", "semi", "warp1_amt"]);
    }
    if let Some(n) = num("filter") {
        return (format!("FILTER {n}"), "filter", &["cutoff", "res", "drive", "key"]);
    }
    if let Some(n) = num("env") {
        let title = if n == "1" { "AMP ENV".to_string() } else { format!("ENV {n}") };
        return (title, "env", &["attack", "decay", "sustain", "release"]);
    }
    if let Some(n) = num("lfo") {
        return (format!("LFO {n}"), "lfo", &["rate", "phase", "var"]);
    }
    if let Some(name) = group.strip_prefix("fx_") {
        return (name.to_uppercase(), "fx", &["mix", "time", "feedback", "decay", "size"]);
    }
    match group {
        "voice" => ("OUT".into(), "out", &["level", "glide"]),
        "sub" => ("SUB".into(), "", &["level", "oct"]),
        "noise" => ("NOISE".into(), "", &["level"]),
        "amp" => ("AMP".into(), "amp", &["gain", "idle"]),
        g => (g.to_uppercase(), "", &[]),
    }
}

/// The modules of an engine with a parameter schema (Morph, and any engine
/// that declares one), from its groups: the groups that sound (switched
/// on, set away from their defaults, or the first oscillator, filter and
/// amp envelope), in the schema's order, which is the signal's.
fn schema_modules(source: &str, outline: &Outline, node: &str, ty: &str, value: &dyn Fn(&str) -> Option<f64>) -> Vec<Module> {
    let groups = spec::groups(ty);
    let mut out: Vec<Module> = Vec::new();
    let numeric = |s: &ParamSpec| !s.is_choice() && s.unit != Unit::Bool;
    for (group, about, keys) in &groups {
        let (title, kind, first) = group_shape(group);
        let suffix = |key: &str| -> String { key.split_once('_').map_or(key.to_string(), |(_, s)| s.to_string()) };
        let key_of = |s: &str| keys.iter().find(|k| suffix(k) == s).map(|k| k.to_string());
        let on = key_of("on").map(|k| value(&k).or_else(|| spec::spec(ty, &k).map(|s| s.default)).unwrap_or(1.0) > 0.5);
        let away = keys.iter().any(|k| {
            let s = spec::spec(ty, k);
            outline.node(node).and_then(|n| n.field(k)).is_some() || matches!((value(k), s), (Some(v), Some(s)) if (v - s.default).abs() > 1e-6)
        });
        let primary = matches!(*group, "osc_a" | "filter1" | "env1" | "voice" | "amp");
        let shown = match on {
            Some(on) => on && (primary || away || kind != "osc"),
            None => primary || away,
        };
        if !shown || (kind == "" && !away) {
            continue;
        }
        // The module's knobs: its main ones in their order, then what is
        // set away from the default, in the schema's order.
        let make = |key: &str| knob(source, outline, node, ty, key, value);
        // An oscillator's mode decides what its main knobs and picture are.
        let osc_mode = key_of("mode").and_then(|k| {
            let s = spec::spec(ty, &k)?;
            let v = value(&k).unwrap_or(s.default);
            s.options.get(v.round().max(0.0) as usize).cloned()
        });
        let analog_osc = kind == "osc" && osc_mode.as_deref() == Some("analog");
        let first: &[&str] = if analog_osc { &["saw", "pulse", "pw", "level"] } else { first };
        // What picks the module's kind: chips by its title.
        let pick_keys: &[&str] = match kind {
            "osc" if analog_osc => &["mode", "sync"],
            "osc" => &["mode"],
            "filter" => &["type"],
            "env" => &["model"],
            "amp" => &["type", "law"],
            _ => &[],
        };
        let mut picks: Vec<ModulePick> = Vec::new();
        for s in pick_keys {
            let Some(field) = keys.iter().copied().find(|k| suffix(k) == *s) else { continue };
            let knob = make(field);
            let at = knob.value.round().max(0.0) as usize;
            let Some(next) = knob.spec.options.get((at + 1) % knob.spec.options.len().max(1)) else { continue };
            picks.push(ModulePick { field, label: knob.readout(), value: format!("@{next}"), tip: knob.tip.clone() });
            // Forty-odd filter types: a second chip goes straight to or from the analog family.
            if kind == "filter" {
                let analog = knob.spec.options.get(at).is_some_and(|o| o.starts_with("analog_"));
                let (label, value) = if analog { ("kit", "@lp24") } else { ("analog", "@analog_lp") };
                picks.push(ModulePick { field, label: label.into(), value: value.into(), tip: "go to the kit's filters or to the analog chip's".into() });
            }
        }
        let mut chosen: Vec<String> = first.iter().filter_map(|s| key_of(s)).collect();
        let cap = match kind {
            "env" => 4,
            "filter" => 3,
            _ => 4,
        };
        for k in keys.iter() {
            if chosen.len() >= cap {
                break;
            }
            if chosen.iter().any(|c| c == k) {
                continue;
            }
            let s = spec::spec(ty, k);
            let set = outline.node(node).and_then(|n| n.field(k)).is_some() || matches!((value(k), &s), (Some(v), Some(s)) if (v - s.default).abs() > 1e-6);
            if set && s.as_ref().is_some_and(numeric) {
                chosen.push(k.to_string());
            }
        }
        // Fixed order: the module's own, then the schema's.
        let rank_of = |k: &str| first.iter().position(|s| suffix(k) == *s).unwrap_or(first.len() + keys.iter().position(|x| *x == k).unwrap_or(0));
        chosen.sort_by_key(|k| rank_of(k));
        let knobs: Vec<Knob> = chosen.iter().map(|k| make(k)).filter(|k| numeric(&k.spec)).take(cap.max(1)).collect();
        if knobs.is_empty() {
            continue;
        }
        let at = |s: &str| knobs.iter().position(|k| suffix(&k.field) == s);
        let graph = match kind {
            "env" => match (at("attack"), at("decay"), at("sustain"), at("release")) {
                (Some(attack), Some(decay), Some(sustain), Some(release)) => Graph::Env { attack, decay, sustain, release },
                _ => Graph::None,
            },
            "filter" => match at("cutoff") {
                Some(cutoff) => {
                    let mode = key_of("type").and_then(|k| {
                        let s = spec::spec(ty, &k)?;
                        let v = value(&k).unwrap_or(s.default);
                        s.options.get(v.round().max(0.0) as usize).cloned()
                    });
                    let mode = mode.as_deref().map(|m| m.strip_prefix("analog_").unwrap_or(m));
                    let mode = match mode {
                        Some(m) if m.starts_with("hp") || m == "sv_hp" => 2,
                        Some(m) if m.starts_with("bp") || m == "sv_bp" || m == "peak" || m == "notch" => 3,
                        _ => 1,
                    };
                    Graph::Filter { cutoff, res: at("res"), drive: at("drive"), mode, q_unit: false }
                }
                None => Graph::None,
            },
            "osc" if analog_osc => {
                let level = |s: &str| key_of(s).and_then(|k| value(&k).or_else(|| spec::spec(ty, &k).map(|s| s.default))).unwrap_or(0.0);
                let shape = if level("pulse") > level("saw") {
                    if (level("pw") - 0.5).abs() < 0.02 { 3 } else { 4 }
                } else if level("tri") > level("saw") {
                    1
                } else {
                    2
                };
                Graph::Wave { shape: shape + 10, pos: None }
            }
            "osc" => Graph::Wave { shape: 8, pos: at("pos") },
            "lfo" => {
                let shape = key_of("shape").and_then(|k| {
                    let s = spec::spec(ty, &k)?;
                    let v = value(&k).unwrap_or(s.default);
                    s.options.get(v.round().max(0.0) as usize).cloned()
                });
                Graph::Lfo { shape: shape.unwrap_or_else(|| "sine".into()), rate: at("rate") }
            }
            _ => Graph::None,
        };
        // What stays when the card is narrow: the amp envelope, the first
        // oscillator and filter, then the output, then the rest.
        let rank = match kind {
            "env" if *group == "env1" => 5,
            "filter" if *group == "filter1" => 4,
            "osc" if *group == "osc_a" => 4,
            "amp" | "out" => 3,
            "osc" | "filter" => 2,
            _ => 1,
        };
        out.push(Module { title, tip: about.to_string(), graph, knobs, rank, picks });
    }
    out
}

/// A kit's pads: every voice slot, its level and tune (`gain: {kick: …}`,
/// `tune: {kick: …}`).
fn pads(slots: &[makepad_score_live::model::SlotDef]) -> Vec<Pad> {
    use makepad_score_player::presets::DRUM_SLOTS;
    DRUM_SLOTS
        .iter()
        .enumerate()
        .map(|(slot, name)| {
            let def = slots.get(slot);
            let level = ParamSpec { label: "LEVEL", min: 0.0, max: 2.0, default: 1.0, ..ParamSpec::generic("gain", Some(1.0)) };
            let tune = ParamSpec { label: "TUNE", min: -12.0, max: 12.0, default: 0.0, unit: Unit::Semis, bipolar: true, ..ParamSpec::generic("tune", Some(0.0)) };
            Pad {
                name,
                slot,
                filled: def.is_some_and(|d| d.recipe.is_some() || d.sample.is_some()),
                level: Knob { field: "gain".into(), entry: Some(name.to_string()), value: def.map_or(1.0, |d| d.gain as f64), spec: level, tip: format!("the {} level", name.replace('_', " ")) },
                tune: Knob { field: "tune".into(), entry: Some(name.to_string()), value: def.map_or(0.0, |d| d.tune as f64), spec: tune, tip: format!("the {} tuning, in semitones", name.replace('_', " ")) },
            }
        })
        .collect()
}

/// A Morph patch's values by key (the loaded score's, preset included).
fn morph_values(patch: &str) -> Option<Vec<f32>> {
    makepad_audio_synth::morph::MorphPatch::parse(patch).ok().map(|p| p.values)
}

/// The slot whose loops the cards' live state is read from.
fn view_of(name: &str) -> String {
    format!("{}/{name}", crate::audio::SLOT)
}

/// A stream node's card, and its fields' values as loaded.
fn stream_card(doc: &ScoreDoc, name: &str, kind: &StreamKind) -> (StreamCard, Box<dyn Fn(&str) -> Option<f64>>) {
    // The loops this node's stream reaches.
    let views: Vec<String> = doc.loops.iter().filter(|l| matches!(doc.resolve_stream(l), Some(Ok(r)) if r.path.iter().any(|n| n == name))).map(|l| view_of(&l.name)).collect();
    match kind.clone() {
        StreamKind::MidiIn { device, channel, bend, latency, .. } => (
            StreamCard::MidiIn { device, channel },
            Box::new(move |k: &str| match k {
                "channel" => Some(channel as f64),
                "bend" => Some(bend as f64),
                "latency" => Some(latency as f64),
                _ => None,
            }),
        ),
        StreamKind::Arp { settings, .. } => {
            // The pattern on a C major triad, over its octaves.
            let mut base: Vec<u8> = (0..settings.octaves.max(1)).flat_map(|o| [60u8, 64, 67].map(|n| (n as i32 + o as i32 * settings.transpose).clamp(0, 127) as u8)).collect();
            if settings.pattern != makepad_audio_synth::kit::arp::PLAYED {
                base.sort_unstable();
            }
            let mut idx = [0u16; 64];
            let n = makepad_audio_synth::kit::arp::order(settings.pattern, base.len(), &mut idx);
            let preview = idx[..n].iter().map(|i| base[*i as usize]).collect();
            (
                StreamCard::Arp { pattern: settings.pattern, preview, views },
                Box::new(move |k: &str| match k {
                    "rate" => Some(settings.step / 4.0),
                    "octaves" => Some(settings.octaves as f64),
                    "gate" => Some(settings.gate as f64),
                    "swing" => Some(settings.swing as f64),
                    "repeats" => Some(settings.repeats as f64),
                    "chance" => Some(settings.chance as f64),
                    "interval" => Some(settings.transpose as f64),
                    _ => None,
                }),
            )
        }
        StreamKind::Chord { tones } => {
            let label = format!("+ {}", tones.iter().filter(|t| **t != 0).map(|t| t.to_string()).collect::<Vec<_>>().join(" "));
            (StreamCard::Shape { label }, Box::new(|_: &str| None))
        }
        StreamKind::Transpose { semis } => (StreamCard::Shape { label: format!("{semis:+} semitones") }, Box::new(move |k: &str| (k == "semis").then_some(semis as f64))),
        StreamKind::Velocity { curve, min, max } => {
            let label = match curve {
                makepad_score_player::stream::VelCurve::Fixed(level) => format!("every key at {:.0}%", level * 100.0),
                c => format!("{} {:.0}..{:.0}%", match c { makepad_score_player::stream::VelCurve::Soft => "soft", makepad_score_player::stream::VelCurve::Hard => "hard", _ => "linear" }, min * 100.0, max * 100.0),
            };
            (StreamCard::Shape { label }, Box::new(move |k: &str| match k {
                "min" => Some(min as f64),
                "max" => Some(max as f64),
                _ => None,
            }))
        }
    }
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
                let tempo = knob(source, outline, name, "Clock", "bpm", &known);
                let kind = LensKind::Clock { tempo, sig: clock.sig, beats_per_bar: doc.beats_per_bar(), tempo_follows: clock.tempo.is_some() };
                (kind, vec![knob(source, outline, name, "Clock", "swing", &known)], hue_of(p, "Clock", 0))
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
                let knobs = vec![knob(source, outline, name, "Loop", "amp", &known), knob(source, outline, name, "Loop", "pan", &known)];
                (kind, knobs, hue)
            }
            other if makepad_score_live::model::STREAM_TYPES.contains(&other) => {
                let Some(node) = doc.stream(name) else { continue };
                let (card, known) = stream_card(doc, name, &node.kind);
                (LensKind::Stream { card }, loose_knobs(source, outline, name, other, &["in"], &spec::keys(other), &*known), hue_of(p, "Clock", 0))
            }
            other => {
                let hue = inst_hue.iter().find(|(n, _)| n == name).map(|(_, h)| *h);
                if let Some(kind) = FxKind::parse(other) {
                    let fx = doc.fx.iter().find(|f| &f.name == name).cloned();
                    let known = move |k: &str| {
                        let i = kind.params().iter().position(|(param, ..)| *param == k)?;
                        fx.as_ref().map(|f| f.params[i] as f64)
                    };
                    let all: Vec<&str> = kind.params().iter().map(|(n, ..)| *n).collect();
                    let knobs = loose_knobs(source, outline, name, other, &["in", "side"], &all, &known);
                    (LensKind::Fx { key: makepad_score_live::key(name), kind: kind.name().to_string() }, knobs, hue_of(p, other, 0))
                } else if let Some(hue) = hue {
                    let label = instrument_label(doc, source, outline, name, other);
                    let engine = engine_label(doc, name, other);
                    let (modules, pads) = match doc.inst(name).map(|i| &i.kind) {
                        Some(InstKind::Synth { recipe, .. }) => {
                            let layer = recipe.layers().first().copied().unwrap_or_default();
                            (synth_modules(source, outline, name, &layer), Vec::new())
                        }
                        Some(InstKind::Drums { slots, .. }) => (Vec::new(), pads(slots)),
                        Some(InstKind::Morph { patch, .. }) => {
                            let values = morph_values(patch);
                            let schema = &makepad_audio_synth::morph::layout().schema;
                            let value = move |k: &str| -> Option<f64> {
                                let i = schema.params.iter().position(|p| p.key == k)?;
                                values.as_ref().and_then(|v| v.get(i)).map(|v| *v as f64)
                            };
                            (schema_modules(source, outline, name, other, &value), Vec::new())
                        }
                        _ if !spec::groups(other).is_empty() => {
                            let none = |_: &str| None;
                            (schema_modules(source, outline, name, other, &none), Vec::new())
                        }
                        _ => {
                            let none = |_: &str| None;
                            let knobs = loose_knobs(source, outline, name, other, &[], &[], &none);
                            let modules = if knobs.is_empty() { Vec::new() } else { vec![Module { title: "SOUND".into(), tip: String::new(), graph: Graph::None, knobs, rank: 3, picks: Vec::new() }] };
                            (modules, Vec::new())
                        }
                    };
                    (LensKind::Inst { label, engine, modules, pads }, Vec::new(), hue)
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

    /// Each module's kind is a chip on its card: that is where a person picks
    /// the analog parts next to the kit's own.
    #[test]
    fn a_morphs_cards_pick_each_modules_kind() {
        let outline = source::outline("");
        let schema = &makepad_audio_synth::morph::layout().schema;
        let values: Vec<(&str, f64)> = vec![("a_mode", 4.0), ("f1_type", schema.param("f1_type").unwrap().options.iter().position(|o| *o == "analog_hp").unwrap() as f64), ("env1_model", 1.0), ("amp_type", 1.0)];
        let value = |k: &str| values.iter().find(|(key, _)| *key == k).map(|(_, v)| *v);
        let modules = schema_modules("", &outline, "lead", "Morph", &value);
        let card = |title: &str| modules.iter().find(|m| m.title == title).unwrap_or_else(|| panic!("{title} is not shown"));
        let fields = |title: &str| card(title).knobs.iter().map(|k| k.field.clone()).collect::<Vec<_>>();
        assert_eq!(fields("OSC A"), ["a_saw", "a_pulse", "a_pw", "a_level"]);
        assert_eq!(fields("FILTER 1"), ["f1_cutoff", "f1_res", "f1_drive"]);
        assert_eq!(fields("AMP ENV"), ["env1_attack", "env1_decay", "env1_sustain", "env1_release"]);
        assert_eq!(fields("AMP"), ["amp_gain", "amp_idle"]);
        // The chips name what is picked and step on; a drawn analog wave is no picker.
        let picks = |title: &str| card(title).picks.iter().map(|p| (p.field, p.label.clone(), p.value.clone())).collect::<Vec<_>>();
        assert_eq!(picks("OSC A")[0], ("a_mode", "analog".to_string(), "@wavetable".to_string()));
        assert_eq!(picks("OSC A")[1].0, "a_sync");
        assert_eq!(picks("FILTER 1")[0], ("f1_type", "an hp".to_string(), "@analog_bp".to_string()));
        assert_eq!(picks("FILTER 1")[1], ("f1_type", "kit".to_string(), "@lp24".to_string()));
        assert_eq!(picks("AMP ENV")[0], ("env1_model", "analog".to_string(), "@curve".to_string()));
        assert_eq!(picks("AMP")[0], ("amp_type", "analog".to_string(), "@clean".to_string()));
        assert!(matches!(card("OSC A").graph, Graph::Wave { shape: 12, pos: None }));
        assert!(matches!(card("FILTER 1").graph, Graph::Filter { mode: 2, .. }));
        assert!(matches!(card("AMP ENV").graph, Graph::Env { .. }));
    }

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
            let modules = match &lens.kind {
                LensKind::Inst { modules, .. } => modules.clone(),
                _ => Vec::new(),
            };
            for knob in lens.knobs.iter().chain(modules.iter().flat_map(|m| m.knobs.iter())) {
                assert!(knob.value.is_finite(), "{}.{}", lens.node, knob.field);
            }
        }
        let clock = lenses.lenses.iter().find(|l| matches!(l.kind, LensKind::Clock { .. })).unwrap();
        assert!(matches!(&clock.kind, LensKind::Clock { tempo, .. } if tempo.field == "bpm"));
    }

    #[test]
    fn midi_in_and_arp_cards_come_from_their_tables() {
        let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet keys = MidiIn{device: \"Keystation\" channel: 2}\nlet arp = Arp{in: keys pattern: @updown rate: 1/16 octaves: 2}\nlet l = Loop{play: s in: arp}\nScore{clock s keys arp l}\n";
        let doc = ScoreDoc::load(src).expect("loads");
        let outline = source::outline(src);
        let lenses = build(&doc, src, &outline, &Folds::default(), &Palette::HOUSE);
        let keys = lenses.lenses.iter().find(|l| l.node == "keys").expect("the MidiIn's lens");
        assert_eq!(keys.kind, LensKind::Stream { card: StreamCard::MidiIn { device: "Keystation".into(), channel: 2 } });
        assert_eq!(keys.knobs[0].field, "channel");
        let arp = lenses.lenses.iter().find(|l| l.node == "arp").expect("the Arp's lens");
        let LensKind::Stream { card: StreamCard::Arp { pattern, preview, views } } = &arp.kind else { panic!("{:?}", arp.kind) };
        assert_eq!((*pattern, preview.as_slice()), (2, &[60u8, 64, 67, 72, 76, 79, 76, 72, 67, 64][..]));
        assert_eq!(views, &[format!("{}/l", crate::audio::SLOT)]);
        let rate = arp.knobs.iter().find(|k| k.field == "rate").expect("a rate knob");
        assert_eq!((rate.readout(), rate.spec.source_text(rate.spec.denorm(rate.norm()))), ("1/16".to_string(), "1/16".to_string()));
    }

    #[test]
    fn a_synth_is_modules_in_signal_order_with_fixed_knobs() {
        let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @acid release: 0.4 cutoff: 900}\nlet l = Loop{play: s hits: \"x...\"}\nScore{clock s l}\n";
        let doc = ScoreDoc::load(src).expect("loads");
        let outline = source::outline(src);
        let lenses = build(&doc, src, &outline, &Folds::default(), &Palette::HOUSE);
        let synth = lenses.lenses.iter().find(|l| l.node == "s").expect("the synth's lens");
        let LensKind::Inst { modules, label, .. } = &synth.kind else { panic!("{:?}", synth.kind) };
        assert_eq!(label, "acid");
        let titles: Vec<&str> = modules.iter().map(|m| m.title.as_str()).collect();
        let at = |t: &str| titles.iter().position(|x| x.starts_with(t)).unwrap_or_else(|| panic!("no {t} in {titles:?}"));
        assert!(at("OSC") < at("FILTER") && at("FILTER") < at("AMP ENV") && at("AMP ENV") < at("OUT"), "{titles:?}");
        let env = &modules[at("AMP ENV")];
        let fields: Vec<&str> = env.knobs.iter().map(|k| k.field.as_str()).collect();
        assert_eq!(fields, ["attack", "decay", "sustain", "release"], "the order is the module's, not the source's");
        assert!((env.knobs[3].value - 0.4).abs() < 1e-9);
        let filter = &modules[at("FILTER")];
        assert_eq!(filter.knobs[0].field, "cutoff");
        assert!((filter.knobs[0].value - 900.0).abs() < 1e-9);
        let lp = lenses.lenses.iter().find(|l| l.node == "l").expect("the loop's lens");
        assert_eq!(lp.hue, synth.hue, "a loop wears its instrument's colour");
        match &lp.kind {
            LensKind::Loop { pattern: Some(p), .. } => assert_eq!(&src[p.at..p.at + 4], "x..."),
            other => panic!("{other:?}"),
        }
    }
}
