//! What a panel knows about a field before it reads the document: its
//! range, how a knob's travel maps onto it, its unit and how it is written
//! back. One table per node type; a control may override any of it
//! (`Knob{param: @cutoff min: 200 max: 4000}`).

use makepad_score_player::FxKind;

/// How a control's travel (0..1) maps onto a field: Log is equal ratios
/// for equal travel (frequencies, times; needs `min > 0`), Int whole numbers.
use makepad_edit_values::Taper;

#[derive(Clone, Debug, PartialEq)]
pub struct ParamSpec {
    pub key: String,
    /// The silkscreen label, upper case on the panel.
    pub label: &'static str,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub taper: Taper,
    pub unit: Unit,
    /// A bipolar field (pan, detune) draws its arc from the middle.
    pub bipolar: bool,
    /// Choices for an option field (`osc: @saw`); empty for numbers.
    pub options: Vec<String>,
    /// The module it belongs to (a Morph's `osc_a`, `filter1`, `lfo2` …;
    /// empty where a node type has no groups): what a lens or a panel
    /// gathers into one module.
    pub group: &'static str,
    /// A mod row may move it (a Morph's mod destinations).
    pub modulatable: bool,
    /// One line about it, for the AI and tooltips.
    pub about: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    None,
    Hz,
    Secs,
    Beats,
    Cents,
    Db,
    Percent,
    Semis,
    /// A true/false field, written `true` or `false`.
    Bool,
    Ms,
    /// A note value (an arp's step), written as a fraction (`1/16`),
    /// stepping through [`NOTE_VALUES`].
    Note,
}

/// The note values a [`Unit::Note`] steps through, slow to fast: whole,
/// half, quarter, quarter triplet, eighth, eighth triplet, sixteenth, …
pub const NOTE_VALUES: [(f64, &str); 11] = [
    (1.0, "1/1"), (0.5, "1/2"), (0.25, "1/4"), (1.0 / 6.0, "1/6"), (0.125, "1/8"), (1.0 / 12.0, "1/12"), (0.0625, "1/16"), (1.0 / 24.0, "1/24"),
    (1.0 / 32.0, "1/32"), (1.0 / 48.0, "1/48"), (1.0 / 64.0, "1/64"),
];

/// The note value nearest `value` (index into [`NOTE_VALUES`]).
pub fn note_index(value: f64) -> usize {
    (0..NOTE_VALUES.len()).min_by(|a, b| (NOTE_VALUES[*a].0.ln() - value.max(1e-6).ln()).abs().total_cmp(&(NOTE_VALUES[*b].0.ln() - value.max(1e-6).ln()).abs())).unwrap_or(6)
}

pub const WAVES: [&str; 6] = ["saw", "square", "pulse", "tri", "sine", "softsaw"];
pub const FILTERS: [&str; 5] = ["ladder", "lp", "bp", "hp", "notch"];
pub const NOISES: [&str; 5] = ["white", "pink", "brown", "blue", "velvet"];
pub const KITS: [&str; 5] = ["analog_80s", "analog_hybrid", "studio", "lofi", "chip"];
pub const LFO_SHAPES: [&str; 5] = ["sine", "tri", "saw", "square", "random"];
/// The chords a `Chord` stream node offers by name (the score's chord kinds).
pub const CHORD_NAMES: [&str; 10] = ["major", "minor", "dim", "aug", "sus2", "sus4", "maj7", "m7", "dom7", "power"];

impl ParamSpec {
    fn num(key: &str, label: &'static str, min: f64, max: f64, default: f64, taper: Taper, unit: Unit) -> ParamSpec {
        ParamSpec { key: key.to_string(), label, min, max, default, taper, unit, bipolar: false, options: Vec::new(), group: "", modulatable: false, about: "" }
    }

    fn choice(key: &str, label: &'static str, options: &[&str]) -> ParamSpec {
        let options: Vec<String> = options.iter().map(|o| o.to_string()).collect();
        ParamSpec { key: key.to_string(), label, min: 0.0, max: (options.len().max(1) - 1) as f64, default: 0.0, taper: Taper::Int, unit: Unit::None, bipolar: false, options, group: "", modulatable: false, about: "" }
    }

    /// Only these choices, in this order (`Switch{param: @osc options: [@saw @square]}`).
    pub fn restrict(&mut self, options: Vec<String>) {
        if self.is_choice() && !options.is_empty() {
            self.max = (options.len() - 1) as f64;
            self.options = options;
        }
    }

    fn bipolar(mut self) -> ParamSpec {
        self.bipolar = true;
        self
    }

    /// A field no table knows: 0..1, or around the value it has.
    pub fn generic(key: &str, value: Option<f64>) -> ParamSpec {
        let (min, max) = match value {
            Some(v) if v > 1.0 => (0.0, (v * 2.0).max(1.0)),
            Some(v) if v < 0.0 => (v * 2.0, -v * 2.0),
            _ => (0.0, 1.0),
        };
        ParamSpec { key: key.to_string(), label: "", min, max, default: value.unwrap_or(0.0), taper: Taper::Linear, unit: Unit::None, bipolar: min < 0.0, options: Vec::new(), group: "", modulatable: false, about: "" }
    }

    pub fn is_choice(&self) -> bool {
        !self.options.is_empty()
    }

    /// Travel 0..1 for a value.
    pub fn norm(&self, value: f64) -> f64 {
        if self.unit == Unit::Note {
            return note_index(value) as f64 / (NOTE_VALUES.len() - 1) as f64;
        }
        if !value.is_finite() || self.max <= self.min {
            return 0.0;
        }
        let v = value.clamp(self.min, self.max);
        match self.taper {
            Taper::Log if self.min > 0.0 => (v / self.min).ln() / (self.max / self.min).ln(),
            _ => (v - self.min) / (self.max - self.min),
        }
    }

    /// The value at travel `t` (0..1).
    pub fn denorm(&self, t: f64) -> f64 {
        let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
        if self.unit == Unit::Note {
            return NOTE_VALUES[(t * (NOTE_VALUES.len() - 1) as f64).round() as usize].0;
        }
        match self.taper {
            Taper::Log if self.min > 0.0 => self.min * (self.max / self.min).powf(t),
            Taper::Int => (self.min + t * (self.max - self.min)).round(),
            _ => self.min + t * (self.max - self.min),
        }
    }

    /// The value as the source writes it: as few digits as the range needs
    /// (a cutoff is `812`, a resonance `0.62`, an attack `0.004`).
    pub fn source_text(&self, value: f64) -> String {
        if let Some(name) = self.options.get(value.round().max(0.0) as usize).filter(|_| self.is_choice()) {
            return format!("@{name}");
        }
        if self.unit == Unit::Bool {
            return if value > 0.5 { "true" } else { "false" }.to_string();
        }
        if self.unit == Unit::Note {
            return NOTE_VALUES[note_index(value)].1.to_string();
        }
        let value = value.clamp(self.min.min(self.max), self.max.max(self.min));
        let decimals = if self.taper == Taper::Int {
            0
        } else {
            let a = value.abs();
            if a >= 100.0 {
                0
            } else if a >= 10.0 {
                1
            } else if a >= 1.0 {
                2
            } else if a >= 0.01 || a == 0.0 {
                // Two significant digits below one, never fewer than two
                // decimals: 0.62, 0.05.
                2 + (-(a.max(1e-9)).log10()).floor().max(0.0) as usize
            } else {
                4
            }
        };
        let text = format!("{value:.decimals$}");
        let text = if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_string() } else { text };
        if text == "-0" { "0".to_string() } else { text }
    }

    /// A typed value (a knob's or a graph point's entry): a number or an
    /// expression with units in the field's own terms (`700ms`, `1.2s`,
    /// `2k`, `80%`, `-3dB`, `+2st`), relative to `current` when it starts
    /// with an operator (`+0.1`, `*2`), clamped to the range. A percent
    /// field reads a bare number above 1 as percent (`80` is 80%); a choice
    /// takes one of its options by name.
    pub fn parse(&self, text: &str, current: f64) -> Result<f64, String> {
        let t = text.trim();
        if self.is_choice() {
            let name = t.trim_start_matches('@').to_ascii_lowercase();
            if let Some(i) = self.options.iter().position(|o| o.to_ascii_lowercase() == name) {
                return Ok(i as f64);
            }
        }
        if self.unit == Unit::Bool {
            match t.to_ascii_lowercase().as_str() {
                "on" | "true" | "yes" | "1" => return Ok(1.0),
                "off" | "false" | "no" | "0" => return Ok(0.0),
                _ => {}
            }
        }
        let unit = match self.unit {
            Unit::Hz => "hz",
            Unit::Secs => "s",
            Unit::Ms => "ms",
            Unit::Percent => "frac",
            Unit::Db => "db",
            Unit::Cents => "ct",
            Unit::Semis => "st",
            _ => "",
        };
        let mut v = makepad_edit_values::number::eval(t, current, unit)?;
        let has_unit = t.chars().any(|c| c.is_ascii_alphabetic() || c == '%');
        if self.unit == Unit::Percent && !has_unit && v.abs() > 1.0 {
            v /= 100.0;
        }
        let v = v.clamp(self.min.min(self.max), self.max.max(self.min));
        Ok(if self.taper == Taper::Int { v.round() } else { v })
    }

    /// The value for a person: `812 Hz`, `3.5 ms`, `62%`.
    pub fn display(&self, value: f64) -> String {
        if self.is_choice() {
            return self.options.get(value.round().max(0.0) as usize).map_or(String::new(), |o| o.to_uppercase());
        }
        match self.unit {
            Unit::Hz if value >= 1000.0 => format!("{:.2} kHz", value / 1000.0),
            Unit::Hz => format!("{value:.0} Hz"),
            Unit::Secs if value < 1.0 => format!("{:.0} ms", value * 1000.0),
            Unit::Secs => format!("{value:.2} s"),
            Unit::Beats => format!("{value:.3} b"),
            Unit::Cents => format!("{value:+.0} ct"),
            Unit::Db => format!("{value:+.1} dB"),
            Unit::Percent => format!("{:.0}%", value * 100.0),
            Unit::Semis => format!("{value:.2} st"),
            Unit::Bool => if value > 0.5 { "ON" } else { "OFF" }.to_string(),
            Unit::Ms => format!("{value:.0} ms"),
            Unit::Note => NOTE_VALUES[note_index(value)].1.to_string(),
            Unit::None if self.taper == Taper::Int => format!("{value:.0}"),
            Unit::None => format!("{value:.2}"),
        }
    }
}

/// A Morph field from the synth kit's schema (the one table its engine,
/// the score and the AI read).
fn morph_spec(key: &str) -> Option<ParamSpec> {
    use makepad_audio_synth::kit::schema::{Curve, Unit as K};
    let p = makepad_audio_synth::morph::layout().schema.param(key)?;
    let taper = match p.curve {
        Curve::Log => Taper::Log,
        Curve::Linear => Taper::Linear,
        Curve::Int | Curve::Choice | Curve::Bool => Taper::Int,
    };
    let unit = match (p.curve, p.unit) {
        (Curve::Bool, _) => Unit::Bool,
        (_, K::Hz) => Unit::Hz,
        (_, K::Secs) => Unit::Secs,
        (_, K::Ms) => Unit::Ms,
        (_, K::Beats) => Unit::Beats,
        (_, K::Cents) => Unit::Cents,
        (_, K::Semis) => Unit::Semis,
        (_, K::Db) => Unit::Db,
        (_, K::Percent) => Unit::Percent,
        (_, K::None) => Unit::None,
    };
    Some(ParamSpec {
        key: key.to_string(),
        label: p.label.as_str(),
        min: p.min as f64,
        max: p.max as f64,
        default: p.default as f64,
        taper,
        unit,
        bipolar: p.bipolar,
        options: p.options.iter().map(|o| o.to_string()).collect(),
        group: p.group.as_str(),
        modulatable: p.modulatable,
        about: p.about,
    })
}

/// A node type's modules: (group, its line, its fields in order), for
/// lenses and panels that draw a module at a time (Morph's; empty for the
/// others).
pub fn groups(ty: &str) -> Vec<(&'static str, &'static str, Vec<&'static str>)> {
    match ty {
        "Morph" => {
            let schema = &makepad_audio_synth::morph::layout().schema;
            schema.groups.iter().map(|g| (g.name.as_str(), g.about, schema.group(&g.name).map(|p| p.key.as_str()).collect())).collect()
        }
        _ => Vec::new(),
    }
}

/// The table for a node of type `ty` (`Synth`, `Drums`, `Loop`, an FX).
pub fn spec(ty: &str, key: &str) -> Option<ParamSpec> {
    if ty == "Morph" {
        return morph_spec(key);
    }
    use Taper::*;
    use Unit::*;
    let s = |label, min, max, default, taper, unit| Some(ParamSpec::num(key, label, min, max, default, taper, unit));
    match ty {
        "Synth" | "Sampler" => match key {
            "osc" | "wave" => Some(ParamSpec::choice(key, "WAVE", &WAVES)),
            "filter" => Some(ParamSpec::choice(key, "MODE", &FILTERS)),
            "noise" => Some(ParamSpec::choice(key, "NOISE", &NOISES)),
            "cutoff" => s("CUTOFF", 30.0, 16000.0, 2000.0, Log, Hz),
            "cutoff_to" => s("SWEEP TO", 30.0, 16000.0, 400.0, Log, Hz),
            "sweep" => s("SWEEP", 0.01, 4.0, 0.3, Log, Secs),
            // The filter's Q, as the recipe takes it.
            "res" | "q" => s("RES", 0.5, 20.0, 0.707, Log, None),
            "attack" => s("ATTACK", 0.001, 4.0, 0.005, Log, Secs),
            "decay" => s("DECAY", 0.005, 8.0, 0.3, Log, Secs),
            "sustain" => s("SUSTAIN", 0.0, 1.0, 0.7, Linear, Percent),
            "release" => s("RELEASE", 0.005, 8.0, 0.2, Log, Secs),
            "hold" => s("HOLD", 0.0, 4.0, 0.0, Linear, Secs),
            "drive" => s("DRIVE", 0.0, 10.0, 0.0, Linear, None),
            "detune" => ParamSpec::num(key, "DETUNE", -50.0, 50.0, 0.0, Linear, Cents).bipolar().into(),
            "vib" | "vibrato" => s("RATE", 0.0, 12.0, 5.0, Linear, Hz),
            "vib_depth" => s("DEPTH", 0.0, 1.0, 0.0, Linear, Semis),
            "width" | "pw" => s("PW", 0.05, 0.95, 0.5, Linear, Percent),
            "fm" | "ratio" => s("RATIO", 0.5, 16.0, 2.0, Log, None),
            "index" => s("INDEX", 0.0, 12.0, 2.0, Linear, None),
            "index_to" => s("INDEX END", 0.0, 12.0, 0.5, Linear, None),
            "pluck" => s("DAMP", 0.0, 1.0, 0.3, Linear, Percent),
            "unison" | "voices" => s("VOICES", 1.0, 6.0, 1.0, Int, None),
            "spread" => s("SPREAD", 0.0, 50.0, 10.0, Linear, Cents),
            "gain" | "volume" => s("LEVEL", 0.0, 2.0, 0.7, Linear, None),
            "root" => s("ROOT", 24.0, 96.0, 60.0, Int, None),
            "mono" => s("MONO", 0.0, 1.0, 0.0, Int, Bool),
            _ => Option::None,
        },
        "Instrument" => match key {
            "kind" => Some(ParamSpec::choice(key, "KIND", &["piano", "drums", "soundfont", "midi", "input"])),
            "preset" => {
                let names: Vec<&str> = makepad_audio_synth::instruments::PianoInstrument::preset_names().collect();
                Some(ParamSpec::choice(key, "VOICING", &names))
            }
            "gain" => s("LEVEL", 0.0, 2.0, 1.0, Linear, None),
            "bank" => s("BANK", 0.0, 128.0, 0.0, Int, None),
            "program" => s("PROGRAM", 0.0, 127.0, 0.0, Int, None),
            "channel" => s("CHANNEL", 1.0, 16.0, 1.0, Int, None),
            _ => Option::None,
        },
        "Drums" => match key {
            "kit" => Some(ParamSpec::choice(key, "KIT", &KITS)),
            _ => Option::None,
        },
        "Loop" => match key {
            "amp" => s("LEVEL", 0.0, 1.0, 0.8, Linear, Percent),
            "pan" => ParamSpec::num(key, "PAN", -1.0, 1.0, 0.0, Linear, None).bipolar().into(),
            "beats" => s("LENGTH", 1.0, 16.0, 4.0, Int, Beats),
            "mute" => s("MUTE", 0.0, 1.0, 0.0, Int, Bool),
            "solo" => s("SOLO", 0.0, 1.0, 0.0, Int, Bool),
            _ => Option::None,
        },
        "Clock" => match key {
            "bpm" => s("TEMPO", 40.0, 220.0, 120.0, Linear, None),
            "swing" => s("SWING", 0.0, 0.5, 0.0, Linear, Percent),
            _ => Option::None,
        },
        // MIDI in and the stream nodes that shape what it plays.
        "MidiIn" => match key {
            "channel" => s("CHANNEL", 0.0, 16.0, 0.0, Int, None),
            "bend" => s("BEND", 0.0, 24.0, 2.0, Int, Semis),
            "latency" => s("LATENCY", 0.0, 50.0, 8.0, Linear, Ms),
            "mpe" => s("MPE", 0.0, 1.0, 0.0, Int, Bool),
            _ => Option::None,
        },
        "Arp" => match key {
            "pattern" => Some(ParamSpec::choice(key, "PATTERN", makepad_audio_synth::kit::arp::PATTERNS)),
            "rate" => s("RATE", 1.0 / 64.0, 1.0, 0.0625, Log, Note),
            "octaves" => s("OCTAVES", 1.0, 4.0, 1.0, Int, None),
            "gate" => s("GATE", 0.01, 1.0, 0.6, Linear, Percent),
            "swing" => s("SWING", 0.0, 0.5, 0.0, Linear, Percent),
            "repeats" => s("REPEATS", 1.0, 4.0, 1.0, Int, None),
            "chance" => s("CHANCE", 0.0, 1.0, 1.0, Linear, Percent),
            "interval" => s("INTERVAL", -24.0, 24.0, 12.0, Int, Semis),
            "latch" => s("LATCH", 0.0, 1.0, 0.0, Int, Bool),
            "sync" => s("SYNC", 0.0, 1.0, 1.0, Int, Bool),
            _ => Option::None,
        },
        "Chord" => match key {
            "chord" => Some(ParamSpec::choice(key, "CHORD", &crate::spec::CHORD_NAMES)),
            _ => Option::None,
        },
        "Transpose" => match key {
            "semis" => ParamSpec::num(key, "SEMIS", -24.0, 24.0, 0.0, Int, Semis).bipolar().into(),
            _ => Option::None,
        },
        "Velocity" => match key {
            "curve" => Some(ParamSpec::choice(key, "CURVE", &makepad_score_player::stream::VelCurve::NAMES)),
            "level" => s("LEVEL", 0.0, 1.0, 0.8, Linear, Percent),
            "min" => s("MIN", 0.0, 1.0, 0.0, Linear, Percent),
            "max" => s("MAX", 0.0, 1.0, 1.0, Linear, Percent),
            _ => Option::None,
        },
        _ => {
            let kind = FxKind::parse(ty)?;
            let (_, default, min, max) = *kind.params().iter().find(|(name, ..)| *name == key)?;
            let (min, max, default) = (min as f64, max as f64, default as f64);
            let (label, taper, unit) = match key {
                "cutoff" => ("CUTOFF", Log, Hz),
                "damp" | "tone" => (if key == "damp" { "DAMP" } else { "TONE" }, Log, Hz),
                "res" => ("RES", Log, None),
                "time" => ("TIME", Log, Beats),
                "feedback" => ("FEEDBACK", Linear, Percent),
                "mix" => ("MIX", Linear, Percent),
                "spread" => ("SPREAD", Linear, Percent),
                "room" => ("SIZE", Linear, Percent),
                "amount" => ("DRIVE", Linear, Percent),
                "bits" => ("BITS", Int, None),
                "down" => ("RATE ÷", Int, None),
                "rate" => ("RATE", Log, Hz),
                "depth" => ("DEPTH", Linear, if kind == FxKind::Duck { Db } else { None }),
                "threshold" => ("THRESH", Linear, Db),
                "ratio" => ("RATIO", Log, None),
                "makeup" => ("MAKEUP", Linear, Db),
                k if k.starts_with('b') && k.contains('_') => ("", Linear, None),
                "release" if kind == FxKind::Comp => ("RELEASE", Log, Ms),
                "attack" => ("ATTACK", Log, Ms),
                "release" => ("RELEASE", Log, Secs),
                "knee" => ("KNEE", Linear, Db),
                "width" => ("WIDTH", Linear, Percent),
                "mid" => ("MID", Linear, Db),
                "side" => ("SIDE", Linear, Db),
                "bass_mono" => ("BASS MONO", Linear, Hz),
                "gain" => ("GAIN", Linear, Percent),
                "pan" => ("PAN", Linear, None),
                _ => ("", Linear, None),
            };
            let mut spec = ParamSpec::num(key, label, min, max, default, taper, unit);
            spec.bipolar = key == "pan";
            Some(spec)
        }
    }
}

/// The spec for a field, falling back to a generic one around its value.
pub fn spec_or_generic(ty: &str, key: &str, value: Option<f64>) -> ParamSpec {
    spec(ty, key).unwrap_or_else(|| ParamSpec::generic(key, value))
}

/// Whether a node type is an instrument (has notes), an effect, or a loop.
pub fn is_instrument(ty: &str) -> bool {
    makepad_score_live::instruments::INSTRUMENT_TYPES.contains(&ty)
}

pub fn is_fx(ty: &str) -> bool {
    FxKind::parse(ty).is_some()
}

/// The fields a node of type `ty` takes, in the order a panel offers them.
pub fn keys(ty: &str) -> Vec<&'static str> {
    match ty {
        "Synth" | "Sampler" => vec![
            "osc", "filter", "cutoff", "res", "drive", "attack", "decay", "sustain", "release", "hold", "detune", "unison", "spread", "vib",
            "vib_depth", "width", "fm", "index", "index_to", "cutoff_to", "sweep", "pluck", "noise", "gain",
        ],
        "Drums" => vec!["kit"],
        "Morph" => makepad_audio_synth::morph::layout().schema.params.iter().map(|p| p.key.as_str()).collect(),
        "Instrument" => vec!["preset", "gain", "bank", "program", "channel"],
        "Loop" => vec!["amp", "pan", "beats", "mute", "solo"],
        "Clock" => vec!["bpm", "swing"],
        "MidiIn" => vec!["channel", "bend", "latency", "mpe"],
        "Arp" => vec!["pattern", "rate", "octaves", "gate", "swing", "repeats", "chance", "interval", "latch", "sync"],
        "Chord" => vec!["chord"],
        "Transpose" => vec!["semis"],
        "Velocity" => vec!["curve", "level", "min", "max"],
        _ => FxKind::parse(ty).map_or_else(Vec::new, |k| k.params().iter().map(|(name, ..)| *name).collect()),
    }
}
