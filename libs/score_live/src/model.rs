//! The typed score: what a document declares, read from its evaluated
//! value, with every problem reported at its line and column. Loading
//! never plays anything; the composer and the graph builder read this.

use crate::source::{self, Outline};
use crate::theory;
use crate::vm::{self, Evaluated, Obj, V};
use makepad_audio_synth::recipe::{Layer, Recipe};
use makepad_score_player::{presets, FxKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diag {
    pub line: u32,
    pub col: u32,
    pub node: Option<String>,
    pub message: String,
}

impl std::fmt::Display for Diag {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match &self.node {
            Some(node) => write!(f, "{}:{}: {}: {}", self.line, self.col, node, self.message),
            None => write!(f, "{}:{}: {}", self.line, self.col, self.message),
        }
    }
}

/// A library asset by exact revision.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AssetRef {
    pub id: String,
    pub rev: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClockDef {
    pub name: String,
    pub bpm: f64,
    pub sig: (u32, u32),
    /// Delay of every off-beat sixteenth, as a fraction of a sixteenth.
    pub swing: f64,
    pub seed: u64,
    pub params: Vec<(String, f32)>,
    pub tempo: Option<TempoFollow>,
    /// MIDI controllers that move params (`midi: {cutoff: 74}`): (param, CC).
    pub midi: Vec<(String, u8)>,
    /// OSC: the UDP port it listens on, and where `osc_send` sends
    /// ("host:port").
    pub osc_in: Option<u16>,
    pub osc_out: Option<String>,
}

impl Default for ClockDef {
    fn default() -> Self {
        ClockDef { name: String::new(), bpm: 120.0, sig: (4, 4), swing: 0.0, seed: 1, params: Vec::new(), tempo: None, midi: Vec::new(), osc_in: None, osc_out: None }
    }
}

/// A Synth's numeric fields and the recipe key each sets on its layers
/// (`res: 5` is `q=5` on every layer): what a Synth writes, and what an
/// editor holding one of them live sets on the playing patch.
pub const SYNTH_KEYS: [(&str, &str); 23] = [
    ("cutoff", "cutoff"), ("res", "q"), ("q", "q"), ("attack", "a"), ("decay", "d"), ("sustain", "s"), ("release", "r"),
    ("drive", "drive"), ("detune", "detune"), ("vib", "vib"), ("vib_depth", "vib_depth"), ("fm", "fm"), ("index", "index"),
    ("pluck", "pluck"), ("width", "width"), ("hold", "hold"), ("sweep", "glide"), ("cutoff_to", "cutoff_to"), ("index_to", "index_to"), ("gain", "gain"),
    ("pwm", "pwm"), ("pwm_depth", "pwm_depth"), ("delay", "delay"),
];

#[derive(Clone, Debug, PartialEq)]
pub enum InstKind {
    Synth { recipe: Recipe, mono: bool },
    Drums { kit: String, slots: Vec<SlotDef> },
    Sampler { src: AssetRef, root: f32, attack: f32, release: f32 },
    /// Ironfish: a program and its settings (one per
    /// `presets::IRON_PARAMS`, NaN where the program's plays).
    Ironfish { program: usize, values: Vec<f32> },
    /// Morph, the wavetable synth: its patch in text form (checked) and the
    /// library samples its three oscillators read (`a_src` …).
    Morph { patch: String, srcs: [Option<AssetRef>; 3] },
    /// Tandem, the hybrid synth that plays the hybrid patch library: the
    /// patch's name (`Category/Name`, empty = the init patch), the file it
    /// was found at (empty when the library is not installed), and the
    /// parameters the score sets over it (storage names, raw values).
    Tandem { patch: String, path: String, set: Vec<(String, f32)> },
    /// An audio shader instrument; `mono` plays one voice, legato.
    Shader { shader: ShaderDef, mono: bool },
    /// Sung voices (`sing(...)`): a singer profile (a built-in name, or a
    /// library singer's `id@rev` with its `asset`), a choir of `singers` in
    /// `parts`, and style values (one per `VOICE_STYLE`, NaN = default);
    /// `neural` (`engine: @neural`): the neural voice sings its lines.
    Voice { profile: String, asset: Option<AssetRef>, singers: u8, parts: u8, style: [f32; 8], neural: bool },
    /// A native Makepad instrument: `piano` (the modelled grand, with a
    /// preset), `drums` (the modelled acoustic kit, General MIDI notes) or
    /// `soundfont` (a bank and program of a library SF2, or of the General
    /// MIDI SoundFont when `font` is none: [`crate::gm_font`]).
    Native { native: String, preset: Option<String>, font: Option<AssetRef>, bank: u16, program: u8, gain: f32 },
    /// Live audio in (`Instrument{kind: @input gain: 1}`): the loops that
    /// play it pass the input (a microphone, a line in) through at `gain`.
    Input { gain: f32 },
    /// Outboard gear (`Instrument{kind: @midi channel: 1 port: "…"}`): the
    /// notes go out as MIDI on `channel` (1..16) to the output `port` names
    /// (empty: every output).
    Midi { channel: u8, port: String },
}

/// One entry of a loop's note list: a note from `beat` for `dur` beats at
/// `vel`, a General MIDI program change, or the sustain pedal to `vel`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListNote {
    pub beat: f64,
    pub note: f32,
    pub dur: f32,
    pub vel: f32,
    pub kind: ListKind,
    /// The note's options (`[beat note dur vel, {pan bend vib arp …}]`): 1 +
    /// an index into the loop's [`LoopDef::note_opts`]; 0 is none.
    pub opts: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListKind {
    Note,
    /// `[beat @program n]`
    Program(u8),
    /// `[beat @pedal v]`: the damper pedal, 0 up .. 1 down (half-pedal
    /// between).
    Pedal,
}

/// A tempo that follows a Clock param: `from` at 0, `to` at 1, reached over
/// `ramp` beats from the beat after the param changes.
#[derive(Clone, Debug, PartialEq)]
pub struct TempoFollow {
    pub param: String,
    pub from: f64,
    pub to: f64,
    pub ramp: f64,
}

/// The style fields of a `Voice`, in the order the engine reads them.
pub const VOICE_STYLE: [&str; 8] = ["vibrato", "vibrato_rate", "vibrato_delay", "portamento", "scoop", "breath", "drift", "jitter"];

/// An audio shader a score plays: a library one by name or its own code,
/// and the settings the score gives its params.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderDef {
    pub lib: Option<String>,
    pub code: String,
    pub params: Vec<(String, f32)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SlotDef {
    pub recipe: Option<Recipe>,
    pub sample: Option<AssetRef>,
    pub gain: f32,
    pub tune: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InstDef {
    pub name: String,
    pub kind: InstKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Sync {
    /// Start with the others at 0, or on the next bar when added live.
    Bar,
    /// Start on another loop's next cycle.
    Loop(String),
    /// Start at once.
    Now,
}

/// A pattern loop: `hits: "x..x"` played on `note` (a drum or a note).
#[derive(Clone, Debug, PartialEq)]
pub struct Hits {
    pub steps: Vec<Option<f32>>,
    /// A drum's kit slot and General MIDI note, or a pitched note.
    pub drum: Option<(u8, u8)>,
    pub note: f32,
    pub step: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoopDef {
    pub name: String,
    /// The instrument node it plays.
    pub play: Option<String>,
    pub beats: Option<f64>,
    pub amp: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub sync: Sync,
    pub hits: Option<Hits>,
    /// The loop body's function (index into the evaluated functions).
    pub code: Option<usize>,
    /// A per-cycle gate function.
    pub when: Option<usize>,
    pub follow: Option<(String, f32, f32)>,
    /// Default note length in beats.
    pub dur: f32,
    /// A stinger: plays once when triggered, quantised to this many beats.
    pub stinger: Option<f64>,
    /// A note list (imported songs): streamed to the player as the clock
    /// reaches it, in beat order, not a whole cycle at once.
    pub notes: Option<std::sync::Arc<[ListNote]>>,
    /// The options of the note list's entries that have them: (pan, opts).
    pub note_opts: std::sync::Arc<[(f32, makepad_score_player::NoteOpts)]>,
    /// Aux sends: (FX, amount). The loop keeps its own path to the master
    /// and feeds each FX this much too.
    pub sends: Vec<(String, f32)>,
    /// Its own swing (else the Clock's), and humanize: (time in beats,
    /// velocity as a fraction), seeded per cycle.
    pub swing: Option<f64>,
    pub humanize: Option<(f64, f32)>,
    /// The `fx(@reverb, {…}, fn() {…})` scopes in its code, as chains from
    /// the outermost effect in. Each plays through its own copy of the
    /// instrument (`scope_play`) into its effect (`scope_fx`), whose output
    /// goes where the loop goes.
    pub scopes: Vec<Vec<FxKind>>,
    /// A MIDI keyboard plays its instrument live: the channel (1..16), 0
    /// for any.
    pub midi_in: Option<u8>,
}

/// The graph node of loop `name`'s scope `k` (1-based) that plays its notes.
pub fn scope_play(name: &str, k: usize) -> String {
    format!("{name}_fx{k}_in")
}

/// The effect node of loop `name`'s scope `k` (1-based).
pub fn scope_fx(name: &str, k: usize) -> String {
    format!("{name}_fx{k}")
}

/// The effect an `fx(@name …)` names (`@reverb`, `@lowpass`, `@echo`, …).
pub fn fx_atom(name: &str) -> Option<FxKind> {
    let mut chars = name.chars();
    let first = chars.next()?.to_ascii_uppercase();
    let kind = FxKind::parse(&format!("{first}{}", chars.as_str()))?;
    (kind != FxKind::Duck).then_some(kind)
}

/// The effects scoped fx() calls may name.
pub const SCOPE_FX: &str = "@lowpass @highpass @bandpass @delay @echo @reverb @drive @crush @chorus @comp @bus @limiter";

/// The `fx(@kind` scopes in loop code, as chains from the outermost in (in
/// order of appearance), and the names that are no effect.
pub fn fx_scopes(code: &str) -> (Vec<Vec<FxKind>>, Vec<String>) {
    let b = code.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let (mut chains, mut unknown): (Vec<Vec<FxKind>>, Vec<String>) = (Vec::new(), Vec::new());
    let mut stack: Vec<(FxKind, i32)> = Vec::new();
    let mut depth = 0i32;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'f' if b[i..].starts_with(b"fx(") && (i == 0 || !ident(b[i - 1])) => {
                depth += 1;
                let mut j = i + 3;
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if b.get(j) == Some(&b'@') {
                    let end = (j + 1..b.len()).find(|&k| !ident(b[k])).unwrap_or(b.len());
                    let name = &code[j + 1..end];
                    match fx_atom(name) {
                        Some(kind) => {
                            stack.push((kind, depth));
                            let chain: Vec<FxKind> = stack.iter().map(|(k, _)| *k).collect();
                            if !chains.contains(&chain) {
                                chains.push(chain);
                            }
                        }
                        None => unknown.push(name.to_string()),
                    }
                    i = end;
                    continue;
                }
                i += 3;
                continue;
            }
            b'(' => depth += 1,
            b')' => {
                if stack.last().is_some_and(|(_, d)| *d == depth) {
                    stack.pop();
                }
                depth -= 1;
            }
            _ => {}
        }
        i += 1;
    }
    (chains, unknown)
}

#[derive(Clone, Debug, PartialEq)]
pub struct FxDef {
    pub name: String,
    pub kind: FxKind,
    pub params: [f32; makepad_score_player::fx::MAX_FX_PARAMS],
    pub inputs: Vec<String>,
    pub side: Option<String>,
    /// A shader FX (`ShaderFx{…}`): its program; `kind` is then Bus.
    pub shader: Option<ShaderDef>,
    /// Settings that follow a Clock param: (setting index, how).
    pub follow: Vec<(usize, TempoFollow)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SectionDef {
    pub name: String,
    pub bars: f64,
    pub loops: Vec<String>,
    pub next: Option<String>,
    /// The tempo it plays at (from its first beat, reached over `ramp`
    /// beats); None keeps the tempo playing.
    pub bpm: Option<f64>,
    pub ramp: f64,
    /// Tempo points inside it, as (beat from its start, bpm), in beat
    /// order: a performer's rubato, step by step.
    pub tempo: Vec<(f64, f64)>,
    /// Clock params it sets as it starts (`set: {build: 1}`): loops read
    /// them, FX settings that follow them move.
    pub set: Vec<(String, f32)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScoreDoc {
    pub source: String,
    pub label: String,
    pub clock: ClockDef,
    pub insts: Vec<InstDef>,
    pub loops: Vec<LoopDef>,
    pub fx: Vec<FxDef>,
    pub sections: Vec<SectionDef>,
    pub arrange: Vec<String>,
    pub master: f32,
    /// The master limiter's ceiling, dBFS (`Score{ceiling: -1.5}`).
    pub ceiling: f32,
    /// Node names in source order, with their type.
    pub order: Vec<(String, String)>,
    /// Canvas positions written as `at: {x: 120 y: 80}` (`at(ring, i)` is the
    /// loops' ring lookup, so a position is a plain record).
    pub positions: Vec<(String, (f64, f64))>,
    pub outline: Outline,
    /// `Score{rights: {license credits source terms}}`: the terms the score
    /// is published under (an adaptation carries its original's).
    pub rights: Option<ScoreRights>,
}

/// A score's licence: an SPDX-style identifier (`CC-BY-SA-3.0-DE`), the
/// credit line it requires, where the original comes from and the terms'
/// URL.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScoreRights {
    pub license: String,
    pub credits: String,
    pub source: String,
    pub terms: String,
}

impl ScoreDoc {
    pub fn inst(&self, name: &str) -> Option<&InstDef> {
        self.insts.iter().find(|inst| inst.name == name)
    }

    /// Every node that plays a loop's notes (each has a feed): the loops,
    /// and their fx() scopes.
    pub fn feed_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        for l in &self.loops {
            out.push(l.name.clone());
            out.extend((1..=l.scopes.len()).map(|k| scope_play(&l.name, k)));
        }
        out
    }

    pub fn loop_def(&self, name: &str) -> Option<&LoopDef> {
        self.loops.iter().find(|l| l.name == name)
    }

    pub fn beats_per_bar(&self) -> f64 {
        self.clock.sig.0.max(1) as f64 * 4.0 / self.clock.sig.1.max(1) as f64
    }

    /// The arrangement's length in beats (0 when the score has none).
    pub fn arrangement_beats(&self) -> f64 {
        self.arrange.iter().filter_map(|name| self.sections.iter().find(|s| &s.name == name)).map(|s| s.bars * self.beats_per_bar()).sum()
    }

    /// Every library asset the score plays: its samples and its singers.
    pub fn dependencies(&self) -> Vec<AssetRef> {
        let mut out = self.samples();
        out.extend(self.singers());
        out.extend(self.files());
        out.sort_by(|a, b| (&a.id, &a.rev).cmp(&(&b.id, &b.rev)));
        out.dedup();
        out
    }

    /// The library samples its samplers and kit slots play.
    pub fn samples(&self) -> Vec<AssetRef> {
        let mut out: Vec<AssetRef> = Vec::new();
        for inst in &self.insts {
            match &inst.kind {
                InstKind::Sampler { src, .. } => out.push(src.clone()),
                InstKind::Morph { srcs, .. } => out.extend(srcs.iter().flatten().cloned()),
                InstKind::Drums { slots, .. } => out.extend(slots.iter().filter_map(|s| s.sample.clone())),
                InstKind::Synth { .. } | InstKind::Tandem { .. } | InstKind::Ironfish { .. } | InstKind::Shader { .. } | InstKind::Voice { .. } | InstKind::Native { .. } | InstKind::Midi { .. } | InstKind::Input { .. } => {}
            }
        }
        out.sort_by(|a, b| (&a.id, &a.rev).cmp(&(&b.id, &b.rev)));
        out.dedup();
        out
    }

    /// Whether a `@soundfont` instrument names no library SF2 and so plays
    /// the General MIDI SoundFont ([`crate::gm_font`]).
    pub fn plays_gm_font(&self) -> bool {
        self.insts.iter().any(|inst| matches!(&inst.kind, InstKind::Native { native, font: None, .. } if native == "soundfont"))
    }

    /// The library files its native instruments load (SoundFonts).
    pub fn files(&self) -> Vec<AssetRef> {
        let mut out: Vec<AssetRef> = self.insts.iter().filter_map(|inst| match &inst.kind {
            InstKind::Native { font, .. } => font.clone(),
            _ => None,
        }).collect();
        out.sort_by(|a, b| (&a.id, &a.rev).cmp(&(&b.id, &b.rev)));
        out.dedup();
        out
    }

    /// The library singers (voice profile assets) its voices sing with.
    pub fn singers(&self) -> Vec<AssetRef> {
        let mut out: Vec<AssetRef> = self.insts.iter().filter_map(|inst| match &inst.kind {
            InstKind::Voice { asset, .. } => asset.clone(),
            _ => None,
        }).collect();
        out.sort_by(|a, b| (&a.id, &a.rev).cmp(&(&b.id, &b.rev)));
        out.dedup();
        out
    }
}

/// A loaded score: the typed document and the live VM its functions run in.
pub struct Loaded {
    pub doc: ScoreDoc,
    pub vm: Evaluated,
}

impl ScoreDoc {
    /// Load and check; the VM is dropped (see [`load`] to keep it).
    pub fn load(source: &str) -> Result<ScoreDoc, Vec<Diag>> {
        load(source).map(|loaded| loaded.doc)
    }
}

struct Reader<'a> {
    src: &'a str,
    outline: &'a Outline,
    diags: Vec<Diag>,
}

impl Reader<'_> {
    fn err(&mut self, node: &str, key: Option<&str>, message: impl Into<String>) {
        let (line, col) = self.at(node, key);
        self.diags.push(Diag { line, col, node: (!node.is_empty()).then(|| node.to_string()), message: message.into() });
    }

    fn at(&self, node: &str, key: Option<&str>) -> (u32, u32) {
        let ctor = if node.is_empty() { self.outline.root.as_ref() } else { self.outline.node(node) };
        let Some(ctor) = ctor else { return (1, 1) };
        let at = key.and_then(|key| ctor.fields.iter().find(|f| f.key == key)).map_or(ctor.span.start, |f| f.key_span.start);
        let (line, col) = source::line_col(self.src, at);
        (line as u32, col as u32)
    }

    /// The node name an object value is (by where it was made).
    fn name_of(&self, value: &V) -> Option<String> {
        let V::Obj(obj) = value else { return None };
        let (line, col) = obj.loc?;
        let at = source::offset_of(self.src, line as usize, col as usize);
        self.outline.nodes.iter().filter(|n| n.span.start <= at && at < n.span.end).min_by_key(|n| n.span.end - n.span.start).map(|n| n.name.clone())
    }

    /// A reference field: a node, or a list of nodes.
    fn refs(&mut self, node: &str, key: &str, value: Option<&V>) -> Vec<String> {
        match value {
            None | Some(V::Nil) => Vec::new(),
            Some(V::Arr(items)) => items.iter().filter_map(|item| {
                let name = self.name_of(item);
                if name.is_none() {
                    self.err(node, Some(key), format!("`{key}` lists nodes by name: [a b]"));
                }
                name
            }).collect(),
            Some(value) => match self.name_of(value) {
                Some(name) => vec![name],
                None => {
                    self.err(node, Some(key), format!("`{key}` names a node: {key}: other"));
                    Vec::new()
                }
            },
        }
    }

    fn num(&mut self, node: &str, obj: &Obj, key: &str, default: f64, lo: f64, hi: f64) -> f64 {
        match obj.get(key) {
            None | Some(V::Nil) => default,
            Some(value) => match value.num() {
                Some(n) if n.is_finite() => {
                    if n < lo || n > hi {
                        self.err(node, Some(key), format!("`{key}` is {n}; it goes from {lo} to {hi}"));
                    }
                    n.clamp(lo, hi)
                }
                _ => {
                    self.err(node, Some(key), format!("`{key}` is a number"));
                    default
                }
            },
        }
    }

    /// A Morph: its fields as patch text, checked by the engine's own
    /// parser (the reason comes back at the field it names).
    fn morph(&mut self, node: &str, obj: &Obj) -> InstKind {
        use makepad_audio_synth::morph::MorphPatch;
        let word = |v: &V| -> Option<String> {
            match v {
                V::Num(n) => Some(format!("{n}")),
                V::Bool(b) => Some(b.to_string()),
                V::Str(s) | V::Id(s) => Some(s.trim_start_matches('@').to_string()),
                _ => None,
            }
        };
        let mut tokens: Vec<String> = Vec::new();
        let mut srcs: [Option<AssetRef>; 3] = [None, None, None];
        for (key, value) in &obj.fields {
            let key = key.as_str();
            match key {
                "at" => {}
                "a_src" | "b_src" | "c_src" => {
                    let o = ["a_src", "b_src", "c_src"].iter().position(|k| *k == key).unwrap_or(0);
                    srcs[o] = self.asset(node, key, Some(value));
                }
                "mods" => match value {
                    V::Arr(rows) => {
                        for row in rows {
                            match row {
                                V::Arr(items) => {
                                    let words: Vec<String> = items.iter().filter_map(word).collect();
                                    tokens.push(format!("mod={}", words.join(",")));
                                }
                                _ => self.err(node, Some(key), "`mods` is a list of rows: [[@lfo1 @a_pos 0.4] [@env2 @f1_cutoff 0.5]]"),
                            }
                        }
                    }
                    V::Nil => {}
                    _ => self.err(node, Some(key), "`mods` is a list of rows: [[@lfo1 @a_pos 0.4] [@env2 @f1_cutoff 0.5]]"),
                },
                "fx" | "macro_names" => match value {
                    V::Arr(items) => {
                        let words: Vec<String> = items.iter().filter_map(word).map(|w| w.replace([',', ' '], "_")).collect();
                        tokens.push(format!("{key}={}", words.join(",")));
                    }
                    _ => self.err(node, Some(key), format!("`{key}` is a list: [@delay @reverb]")),
                },
                k if k.ends_with("_points") => match value {
                    V::Arr(points) => {
                        let mut out = Vec::new();
                        for p in points {
                            match p {
                                V::Arr(xyz) if xyz.len() >= 2 && xyz.iter().all(|v| v.num().is_some()) => {
                                    out.push(xyz.iter().take(3).map(|v| format!("{}", v.num().unwrap_or(0.0))).collect::<Vec<_>>().join(":"));
                                }
                                _ => self.err(node, Some(key), format!("`{key}` is [[x y curve] ...], x and y 0..1")),
                            }
                        }
                        tokens.push(format!("{key}={}", out.join(",")));
                    }
                    _ => self.err(node, Some(key), format!("`{key}` is [[x y curve] ...], x and y 0..1")),
                },
                k if k.ends_with("_frames") => {
                    let frame = |v: &V| -> Option<String> {
                        match v {
                            V::Arr(amps) if amps.iter().all(|a| a.num().is_some()) => Some(amps.iter().map(|a| format!("{}", a.num().unwrap_or(0.0))).collect::<Vec<_>>().join(":")),
                            _ => None,
                        }
                    };
                    match value {
                        // One frame, or a list of frames.
                        V::Arr(items) if items.iter().all(|a| a.num().is_some()) => tokens.push(format!("{key}={}", frame(value).unwrap_or_default())),
                        V::Arr(items) => match items.iter().map(frame).collect::<Option<Vec<_>>>() {
                            Some(frames) => tokens.push(format!("{key}={}", frames.join("/"))),
                            None => self.err(node, Some(key), format!("`{key}` is [[partial amplitudes] ...], one list a frame")),
                        },
                        _ => self.err(node, Some(key), format!("`{key}` is [[partial amplitudes] ...], one list a frame")),
                    }
                }
                _ => match word(value) {
                    Some(w) => tokens.push(format!("{key}={w}")),
                    None if matches!(value, V::Nil) => {}
                    None => self.err(node, Some(key), format!("`{key}` is a number, an @choice or true/false")),
                },
            }
        }
        // The preset first, so the fields after it override it.
        tokens.sort_by_key(|t| !t.starts_with("preset="));
        let text = tokens.join(" ");
        match MorphPatch::parse(&text) {
            Ok(patch) => InstKind::Morph { patch: patch.to_text(), srcs },
            Err(why) => {
                let at = obj.fields.iter().map(|(k, _)| k.as_str()).find(|k| why.contains(&format!("`{k}`")));
                self.err(node, at, why);
                InstKind::Morph { patch: String::new(), srcs }
            }
        }
    }

    /// A Tandem: its patch by name (found in the installed library) and the
    /// parameters set over it, each checked against the catalogue.
    fn tandem(&mut self, node: &str, obj: &Obj) -> InstKind {
        use makepad_audio_synth::tandem::catalog::catalog;
        let mut patch = String::new();
        let mut set = Vec::new();
        for (key, value) in &obj.fields {
            match key.as_str() {
                "at" => {}
                "patch" => match value {
                    V::Str(s) | V::Id(s) => patch = s.trim_start_matches('@').to_string(),
                    V::Nil => {}
                    _ => self.err(node, Some("patch"), "`patch` is a library patch's name: \"Leads/Name\" (tandem_patches lists them)"),
                },
                k => {
                    let Some(info) = catalog().index(k).map(|i| &catalog().params[i]) else {
                        self.err(node, Some(k), format!("`{k}` is not a Tandem parameter (tandem_params lists them)"));
                        continue;
                    };
                    match value {
                        V::Num(n) => set.push((k.to_string(), *n as f32)),
                        V::Bool(b) => set.push((k.to_string(), *b as u8 as f32)),
                        _ => self.err(node, Some(k), format!("`{k}` is a number ({}..{} raw)", info.min, info.max)),
                    }
                }
            }
        }
        let path = if patch.is_empty() {
            String::new()
        } else {
            match crate::tandem_library::find(&patch) {
                Some(p) => p.to_string_lossy().into_owned(),
                None if crate::tandem_library::active().is_none() => String::new(),
                None => {
                    self.err(node, Some("patch"), format!("no library patch `{patch}` (tandem_patches lists them)"));
                    String::new()
                }
            }
        };
        InstKind::Tandem { patch, path, set }
    }

    fn flag(&mut self, node: &str, obj: &Obj, key: &str) -> bool {
        match obj.get(key) {
            None | Some(V::Nil) => false,
            Some(V::Bool(b)) => *b,
            Some(V::Num(n)) => *n != 0.0,
            Some(_) => {
                self.err(node, Some(key), format!("`{key}` is true or false"));
                false
            }
        }
    }

    fn asset(&mut self, node: &str, key: &str, value: Option<&V>) -> Option<AssetRef> {
        match value {
            Some(V::Obj(obj)) if obj.get("ref").and_then(V::text) == Some("asset") => {
                let id = obj.get("id").and_then(V::text).unwrap_or_default().to_string();
                let rev = obj.get("rev").and_then(V::text).unwrap_or_default().to_string();
                if id.is_empty() {
                    self.err(node, Some(key), "asset(id, rev) needs an asset id");
                    return None;
                }
                Some(AssetRef { id, rev })
            }
            _ => {
                self.err(node, Some(key), format!("`{key}` is a library asset: asset(\"<id>\", \"<rev>\")"));
                None
            }
        }
    }

    /// Recipe keys a Synth may set on its layers, and their recipe names.
    fn layer_keys(&mut self, node: &str, obj: &Obj, layer: &mut Layer, whole: bool) {
        const NUMS: [(&str, &str); 23] = SYNTH_KEYS;
        for (field, key) in NUMS {
            if let Some(value) = obj.get(field) {
                match value.num() {
                    Some(n) => {
                        if let Err(e) = layer.set_num(key, n as f32) {
                            self.err(node, Some(field), e.0);
                        }
                    }
                    None => self.err(node, Some(field), format!("`{field}` is a number")),
                }
            }
        }
        for (field, key) in [("osc", "osc"), ("wave", "osc"), ("noise", "noise"), ("filter", "filter")] {
            if let Some(value) = obj.get(field) {
                match value.text() {
                    Some(text) => {
                        let text = match text { "lp" => "lowpass", "hp" => "highpass", "bp" => "bandpass", t => t };
                        if let Err(e) = layer.set_str(key, text) {
                            self.err(node, Some(field), e.0);
                        }
                    }
                    None => self.err(node, Some(field), format!("`{field}` is a name like @saw")),
                }
            }
        }
        // A layer object also takes any recipe key the Synth fields do not
        // name (freq, to, glide, pan, repeat, lp/hp/bp ...), so a library
        // layer can be written in a score as it is.
        if !whole {
            const SYNTH_ONLY: [&str; 4] = ["octave", "unison", "spread", "mono"];
            for (key, value) in &obj.fields {
                if NUMS.iter().any(|(f, _)| f == key) || ["osc", "wave", "noise", "filter"].contains(&key.as_str()) || SYNTH_ONLY.contains(&key.as_str()) {
                    continue;
                }
                if let Err(e) = crate::palette::sounds::set_key(layer, key, value) {
                    self.err(node, Some(key), e);
                }
            }
        }
    }

    /// A `Shader` or `ShaderFx` node's program: `lib: "acid"` or
    /// `code: "…"`, every other field one of its params. Compiled here so
    /// a problem shows on its field.
    fn shader(&mut self, node: &str, obj: &Obj, kind: makepad_score_player::shader::Kind) -> Option<ShaderDef> {
        use makepad_score_player::shader;
        let (lib, code) = match (obj.get("lib"), obj.get("code")) {
            (Some(V::Str(name)) | Some(V::Id(name)), _) => match shader::library(name) {
                Some(code) => (Some(name.clone()), code.to_string()),
                None => {
                    let names: Vec<&str> = shader::LIBRARY.iter().map(|(n, _)| *n).collect();
                    self.err(node, Some("lib"), format!("no library shader `{name}` (there are {})", names.join(", ")));
                    return None;
                }
            },
            (_, Some(V::Str(code))) => (None, code.clone()),
            _ => {
                self.err(node, None, "a shader is `lib: \"acid\"` (a library shader) or `code: \"…\"` (its own)");
                return None;
            }
        };
        let field = if lib.is_some() { "lib" } else { "code" };
        let compiled = match shader::compile(&code) {
            Ok(compiled) => compiled,
            Err(errors) => {
                for e in errors.iter().take(3) {
                    let line = code[..e.start.min(code.len())].matches('\n').count() + 1;
                    self.err(node, Some(field), format!("shader line {line}: {}", e.message));
                }
                return None;
            }
        };
        if compiled.kind() != kind {
            let want = if kind == shader::Kind::Instrument { "an instrument (it has a voice()); effects go in ShaderFx" } else { "an effect; instruments go in Shader" };
            self.err(node, Some(field), format!("this shader is not {want}"));
            return None;
        }
        let mut params = Vec::new();
        for (name, value) in &obj.fields {
            if ["lib", "code", "mono", "at", "in"].contains(&name.as_str()) {
                continue;
            }
            let Some(info) = (0..compiled.param_count()).map(|index| compiled.param_view(index)).find(|p| p.name == name) else {
                let names: Vec<&str> = (0..compiled.param_count()).map(|index| compiled.param_view(index).name).collect();
                self.err(node, Some(name), format!("the shader has no param `{name}` (it has {})", names.join(", ")));
                continue;
            };
            match value.num() {
                Some(v) => params.push((name.clone(), (v as f32).clamp(info.min, info.max))),
                None => self.err(node, Some(name), format!("`{name}` is a number from {} to {}", info.min, info.max)),
            }
        }
        Some(ShaderDef { lib, code, params })
    }

    /// A note list: `[[beat note dur vel] ...]` (a note by number, name or
    /// drum name; dur and vel optional), `[beat @program n]` changes and
    /// `[beat @pedal v]` pedal moves. A list of such lists (the hands or
    /// voices of a piece: `notes: [right_hand left_hand pedal]`) plays as
    /// one, merged by beat.
    fn note_list(&mut self, node: &str, value: Option<&V>, keep: &dyn Fn(usize) -> bool) -> Option<(std::sync::Arc<[ListNote]>, std::sync::Arc<[(f32, makepad_score_player::NoteOpts)]>)> {
        let items = match value {
            None | Some(V::Nil) => return None,
            Some(V::Arr(items)) => items,
            Some(_) => {
                self.err(node, Some("notes"), "`notes` is a list of [beat note dur vel]");
                return None;
            }
        };
        let is_list = |v: &V| matches!(v, V::Arr(inner) if inner.iter().all(|e| matches!(e, V::Arr(_))));
        let mut entries: Vec<&[V]> = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            match item {
                // A voice muted (or not soloed) plays nothing.
                V::Arr(inner) if !inner.is_empty() && is_list(item) && !keep(i) => {}
                V::Arr(inner) if !inner.is_empty() && is_list(item) => entries.extend(inner.iter().filter_map(|e| if let V::Arr(e) = e { Some(e.as_slice()) } else { None })),
                V::Arr(inner) if inner.is_empty() => {}
                V::Arr(entry) => entries.push(entry),
                _ => {
                    self.err(node, Some("notes"), format!("note {i} is not [beat note dur vel]"));
                    return None;
                }
            }
        }
        let mut out = Vec::with_capacity(entries.len());
        let mut note_opts: Vec<(f32, makepad_score_player::NoteOpts)> = Vec::new();
        for (i, entry) in entries.into_iter().enumerate() {
            let beat = entry.first().and_then(V::num).unwrap_or(-1.0);
            if beat < 0.0 {
                self.err(node, Some("notes"), format!("note {i}: its beat is a number from 0"));
                return None;
            }
            match entry.get(1).and_then(V::text) {
                Some("program") => {
                    let program = entry.get(2).and_then(V::num).unwrap_or(0.0).clamp(0.0, 127.0) as u8;
                    out.push(ListNote { beat, note: 0.0, dur: 0.0, vel: 0.0, kind: ListKind::Program(program), opts: 0 });
                    continue;
                }
                Some("pedal") => {
                    let value = entry.get(2).and_then(V::num).unwrap_or(0.0).clamp(0.0, 1.0) as f32;
                    out.push(ListNote { beat, note: 0.0, dur: 0.0, vel: value, kind: ListKind::Pedal, opts: 0 });
                    continue;
                }
                _ => {}
            }
            let note = match entry.get(1) {
                Some(V::Num(n)) => Some(*n as f32),
                Some(v) => v.text().and_then(|t| theory::note_number(t).or_else(|| presets::gm_drum(t).map(|g| g as f32))),
                None => None,
            };
            let Some(note) = note else {
                self.err(node, Some("notes"), format!("note {i}: not a note (a number, @c4 or a drum)"));
                return None;
            };
            let dur = entry.get(2).and_then(V::num).unwrap_or(0.5).clamp(0.0, 4096.0) as f32;
            let vel = entry.get(3).and_then(V::num).unwrap_or(0.8).clamp(0.0, 1.0) as f32;
            let opts = match entry.get(4) {
                Some(V::Obj(map)) => {
                    note_opts.push(list_note_opts(map));
                    note_opts.len() as u32
                }
                _ => 0,
            };
            out.push(ListNote { beat, note, dur, vel, kind: ListKind::Note, opts });
        }
        out.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        Some((out.into(), note_opts.into()))
    }

    fn synth(&mut self, node: &str, obj: &Obj) -> InstKind {
        let sounds = crate::palette::sounds::sounds();
        let mut patch_mono = false;
        let mut recipe = match (obj.get("preset"), obj.get("recipe")) {
            // A library patch by name, or a patch object (`{layers: [...]}`).
            (Some(V::Obj(patch)), _) => match crate::palette::sounds::patch_recipe(patch) {
                Ok(recipe) => {
                    patch_mono = matches!(patch.get("mono"), Some(V::Bool(true)));
                    recipe
                }
                Err(e) => {
                    self.err(node, Some("preset"), e);
                    Recipe::parse("saw freq=440").unwrap_or_default()
                }
            },
            (Some(preset), _) => match preset.text().and_then(|name| sounds.patch(name)) {
                Some(patch) => {
                    patch_mono = patch.mono;
                    patch.recipe
                }
                None => {
                    self.err(node, Some("preset"), format!("no preset `{}` (presets: {})", preset.text().unwrap_or("?"), sounds.patch_names().join(", ")));
                    Recipe::parse("saw freq=440").unwrap_or_default()
                }
            },
            (None, Some(V::Str(text))) => match Recipe::parse(text) {
                Ok(recipe) => recipe,
                Err(e) => {
                    self.err(node, Some("recipe"), e.0);
                    Recipe::parse("saw freq=440").unwrap_or_default()
                }
            },
            _ => {
                // A plain oscillator voice built from the fields.
                let mut recipe = Recipe::default();
                let mut layer = Layer::default();
                let _ = layer.set_str("osc", "saw");
                layer.sustain = 0.7;
                layer.decay = 0.3;
                layer.release = 0.15;
                recipe.push(layer);
                recipe
            }
        };
        if let Some(V::Arr(layers)) = obj.get("layers") {
            recipe.count = 0;
            for layer_value in layers {
                let V::Obj(layer_obj) = layer_value else {
                    self.err(node, Some("layers"), "`layers` is a list of {osc: @saw cutoff: 900 …}");
                    continue;
                };
                let mut layer = Layer::default();
                let _ = layer.set_str("osc", "saw");
                if let Some(ratio) = layer_obj.get("octave").and_then(V::num) {
                    layer.freq = 440.0 * 2f32.powf(ratio as f32);
                }
                self.layer_keys(node, layer_obj, &mut layer, false);
                if recipe.push(layer).is_none() {
                    self.err(node, Some("layers"), "a synth has at most 6 layers");
                }
            }
        }
        let count = recipe.count;
        for layer in recipe.layers.iter_mut().take(count) {
            let mut l = *layer;
            // `gain` on the node is the instrument's level, not each layer's.
            let mut filtered = obj.clone();
            filtered.fields.retain(|(k, _)| k != "gain" && k != "layers");
            // A preset's band-pass layers are its formants (choir, strings):
            // the node's filter settings leave them alone.
            if l.filter == makepad_audio_synth::dsp::FilterMode::Bandpass && recipe.count > 1 {
                filtered.fields.retain(|(k, _)| !matches!(k.as_str(), "cutoff" | "res" | "q" | "cutoff_to" | "filter"));
            }
            self.layer_keys(node, &filtered, &mut l, true);
            *layer = l;
        }
        // Unison: detuned copies of the first layer.
        let unison = self.num(node, obj, "unison", 1.0, 1.0, 6.0) as usize;
        if unison > 1 && recipe.count > 0 {
            let spread = self.num(node, obj, "spread", 12.0, 0.0, 100.0) as f32;
            let base = recipe.layers[0];
            recipe.count = 0;
            for i in 0..unison {
                let mut layer = base;
                let t = if unison == 1 { 0.0 } else { i as f32 / (unison - 1) as f32 * 2.0 - 1.0 };
                layer.detune_cents = base.detune_cents + t * spread;
                layer.gain = base.gain / (unison as f32).sqrt();
                let _ = recipe.push(layer);
            }
        }
        // `gain` scales the patch's own level (its `volume`).
        recipe.volume = (recipe.volume * self.num(node, obj, "gain", 1.0, 0.0, 4.0) as f32).min(4.0);
        let mono = if obj.get("mono").is_some() { self.flag(node, obj, "mono") } else { patch_mono };
        InstKind::Synth { recipe, mono }
    }

    fn drums(&mut self, node: &str, obj: &Obj) -> InstKind {
        let sounds = crate::palette::sounds::sounds();
        // A library kit by name, or a kit object (`{kick: [layers] ...}`).
        let (kit, recipes) = match obj.get("kit") {
            Some(V::Obj(kit)) => match crate::palette::sounds::kit_slots(kit) {
                Ok(slots) => ("custom".to_string(), slots),
                Err(e) => {
                    self.err(node, Some("kit"), e);
                    return InstKind::Drums { kit: "custom".into(), slots: Vec::new() };
                }
            },
            value => {
                let name = value.and_then(V::text).unwrap_or(crate::palette::sounds::DEFAULT_KIT).to_string();
                match sounds.kit(&name) {
                    Some(kit) => (name, kit.slots.clone()),
                    None => {
                        self.err(node, Some("kit"), format!("no kit `{name}` (kits: {})", sounds.kit_names().join(", ")));
                        return InstKind::Drums { kit: name, slots: Vec::new() };
                    }
                }
            }
        };
        let mut slots: Vec<SlotDef> = recipes.into_iter().map(|recipe| SlotDef { recipe, sample: None, gain: 1.0, tune: 0.0 }).collect();
        for (field, what) in [("tune", 0), ("gain", 1), ("recipes", 2), ("samples", 3)] {
            let Some(value) = obj.get(field) else { continue };
            let V::Obj(map) = value else {
                self.err(node, Some(field), format!("`{field}` maps drums to values: {{kick: …}}"));
                continue;
            };
            for (drum, value) in &map.fields {
                let Some(slot) = presets::slot_of(drum) else {
                    self.err(node, Some(field), format!("no drum `{drum}` (drums: {})", presets::DRUM_SLOTS.join(", ")));
                    continue;
                };
                match what {
                    0 => slots[slot].tune = value.num().unwrap_or(0.0) as f32,
                    1 => slots[slot].gain = value.num().unwrap_or(1.0).clamp(0.0, 4.0) as f32,
                    2 => match value {
                        V::Arr(layers) => match crate::palette::sounds::recipe(layers) {
                            Ok(recipe) => slots[slot].recipe = Some(recipe),
                            Err(e) => self.err(node, Some(field), format!("{drum}: {e}")),
                        },
                        _ => match value.text().map(Recipe::parse) {
                            Some(Ok(recipe)) => slots[slot].recipe = Some(recipe),
                            Some(Err(e)) => self.err(node, Some(field), format!("{drum}: {}", e.0)),
                            None => self.err(node, Some(field), format!("{drum}: a list of layers ({{osc: @sine freq: 60 d: 0.4}}) or recipe text")),
                        },
                    },
                    _ => slots[slot].sample = self.asset(node, field, Some(value)),
                }
            }
        }
        InstKind::Drums { kit, slots }
    }
}

/// The fields each node type reads; anything else is a typo worth saying.
pub const CLOCK_FIELDS: [&str; 10] = ["bpm", "sig", "swing", "seed", "params", "tempo", "midi", "osc_in", "osc_out", "at"];
pub const SYNTH_FIELDS: [&str; 34] = ["at", 
    "preset", "recipe", "layers", "osc", "wave", "noise", "filter", "cutoff", "res", "q", "attack", "decay", "sustain", "release", "hold", "drive",
    "detune", "vib", "vib_depth", "fm", "index", "index_to", "cutoff_to", "sweep", "pluck", "width", "gain", "unison", "spread", "mono", "pwm", "pwm_depth", "delay",
];
pub const DRUMS_FIELDS: [&str; 6] = ["kit", "tune", "gain", "recipes", "samples", "at"];
pub const SAMPLER_FIELDS: [&str; 5] = ["src", "root", "attack", "release", "at"];
/// `label` and `color` (a palette name, `@amber`) are for the views: a
/// track's name and colour; the player does not read them.
pub const LOOP_FIELDS: [&str; 25] = ["play", "beats", "amp", "pan", "mute", "solo", "sync", "hits", "note", "step", "code", "when", "follow", "dur", "quant", "notes", "sends", "swing", "humanize", "midi_in", "mute_voices", "solo_voices", "at", "label", "color"];
pub const INSTRUMENT_FIELDS: [&str; 9] = ["kind", "preset", "src", "bank", "program", "gain", "channel", "port", "at"];
pub const IRON_FIELDS: [&str; 17] = [
    "program", "cutoff", "resonance", "env", "attack", "decay", "sustain", "release", "sub", "noise", "glide", "crush", "delay", "chorus", "reverb", "output", "at",
];
/// Morph's fields besides its parameters (`morph_fields` has them all).
pub const MORPH_EXTRA: [&str; 19] = [
    "preset", "mods", "fx", "macro_names", "a_frames", "b_frames", "c_frames", "a_src", "b_src", "c_src", "lfo1_points", "lfo2_points", "lfo3_points",
    "lfo4_points", "lfo5_points", "lfo6_points", "lfo7_points", "lfo8_points", "at",
];

/// Every field a Tandem reads: `patch`, `at` and every catalogue parameter.
pub fn tandem_fields() -> Vec<&'static str> {
    let c = makepad_audio_synth::tandem::catalog::catalog();
    ["patch", "at"].into_iter().chain(c.params.iter().map(|p| p.name.as_str())).collect()
}

/// Every field a Morph reads: its schema's parameters and [`MORPH_EXTRA`].
pub fn morph_fields() -> Vec<&'static str> {
    let schema = &makepad_audio_synth::morph::layout().schema;
    MORPH_EXTRA.iter().copied().chain(schema.params.iter().map(|p| p.key.as_str())).collect()
}

pub const VOICE_FIELDS: [&str; 13] = ["engine", "profile", "choir", "parts", "vibrato", "vibrato_rate", "vibrato_delay", "portamento", "scoop", "breath", "drift", "jitter", "at"];
pub const SECTION_FIELDS: [&str; 9] = ["bars", "loops", "next", "repeat", "bpm", "ramp", "tempo", "set", "at"];

fn param_list(value: Option<&V>) -> Vec<(String, f32)> {
    match value {
        Some(V::Obj(obj)) => obj.fields.iter().map(|(k, v)| (k.clone(), v.num().unwrap_or(0.0) as f32)).collect(),
        _ => Vec::new(),
    }
}

/// Load a score: evaluate it and read the typed document.
pub fn load(source: &str) -> Result<Loaded, Vec<Diag>> {
    let outline = source::outline(source);
    let evaluated = vm::evaluate(source);
    let mut r = Reader { src: source, outline: &outline, diags: Vec::new() };
    if !evaluated.errors.is_empty() {
        let diags = evaluated.errors.iter().map(|e| {
            let (line, col, message) = vm::split_error(e);
            Diag { line, col, node: None, message }
        }).collect();
        return Err(diags);
    }
    let V::Obj(root) = &evaluated.root else {
        return Err(vec![Diag { line: 1, col: 1, node: None, message: "a score ends with `Score{…}` listing its nodes".into() }]);
    };
    if root.proto.as_deref() != Some("Score") {
        return Err(vec![Diag { line: 1, col: 1, node: None, message: "a score ends with `Score{…}` listing its nodes".into() }]);
    }
    // The nodes: the root's members, named by their `let`.
    // `Score{clock kick}` names each member; its value is the node.
    let mut members = Vec::new();
    for (field, value) in root.fields.iter().map(|(k, v)| (Some(k.as_str()), v)).chain(root.items.iter().map(|v| (None, v))) {
        let V::Obj(obj) = value else { continue };
        if obj.proto.as_deref().is_none_or(|p| !vm::NODE_TYPES.contains(&p) || p == "Score") {
            continue;
        }
        match r.name_of(value) {
            Some(name) => members.push((name, obj.clone())),
            None => r.err("", field, "Score{…} lists nodes written `let name = Type{…}`"),
        }
    }
    let mut doc = ScoreDoc {
        source: source.to_string(),
        label: root.get("label").and_then(V::text).unwrap_or("").to_string(),
        clock: ClockDef::default(),
        insts: Vec::new(),
        loops: Vec::new(),
        fx: Vec::new(),
        sections: Vec::new(),
        arrange: Vec::new(),
        master: r.num("", root, "master", 1.0, 0.0, 2.0) as f32,
        ceiling: r.num("", root, "ceiling", -0.8, -24.0, 0.0) as f32,
        order: Vec::new(),
        positions: Vec::new(),
        outline: Outline::default(),
        rights: match root.get("rights") {
            None | Some(V::Nil) => None,
            Some(V::Obj(o)) => {
                let text = |key: &str| o.get(key).and_then(V::text).unwrap_or("").to_string();
                let rights = ScoreRights { license: text("license"), credits: text("credits"), source: text("source"), terms: text("terms") };
                if rights.license.is_empty() {
                    r.err("", Some("rights"), "`rights` names its `license`, e.g. {license: \"CC-BY-4.0\" credits: \"…\"}");
                }
                Some(rights)
            }
            Some(_) => {
                r.err("", Some("rights"), "`rights` is {license credits source terms}");
                None
            }
        },
    };
    let mut clocks = 0;
    for (name, obj) in &members {
        let ty = obj.proto.clone().unwrap_or_default();
        doc.order.push((name.clone(), ty.clone()));
        if let Some(V::Obj(at)) = obj.get("at") {
            if let (Some(x), Some(y)) = (at.get("x").and_then(V::num), at.get("y").and_then(V::num)) {
                doc.positions.push((name.clone(), (x, y)));
            }
        }
        let n = name.as_str();
        let allowed: Vec<&str> = match ty.as_str() {
            "Clock" => CLOCK_FIELDS.to_vec(),
            "Synth" => SYNTH_FIELDS.to_vec(),
            "Drums" => DRUMS_FIELDS.to_vec(),
            "Sampler" => SAMPLER_FIELDS.to_vec(),
            "Ironfish" => IRON_FIELDS.to_vec(),
            "Morph" => morph_fields(),
            "Tandem" => tandem_fields(),
            "Instrument" => INSTRUMENT_FIELDS.to_vec(),
            "Voice" => VOICE_FIELDS.to_vec(),
            "Loop" | "Stinger" => LOOP_FIELDS.to_vec(),
            "Section" => SECTION_FIELDS.to_vec(),
            other => match FxKind::parse(other) {
                Some(kind) => kind.params().iter().map(|(p, ..)| *p).chain(["in", "key", "at", "b1", "b2", "b3", "b4"]).collect(),
                None => Vec::new(),
            },
        };
        if !allowed.is_empty() {
            for (field, _) in &obj.fields {
                if !allowed.contains(&field.as_str()) {
                    r.err(n, Some(field), format!("`{field}` is not a {ty} field ({ty} fields: {})", allowed.join(", ")));
                }
            }
        }
        match ty.as_str() {
            "Clock" => {
                clocks += 1;
                let sig = match obj.get("sig") {
                    Some(V::Arr(v)) if v.len() == 2 => (v[0].num().unwrap_or(4.0).clamp(1.0, 32.0) as u32, v[1].num().unwrap_or(4.0) as u32),
                    None => (4, 4),
                    _ => {
                        r.err(n, Some("sig"), "`sig` is [beats unit], like [4 4] or [6 8]");
                        (4, 4)
                    }
                };
                if ![1, 2, 4, 8, 16].contains(&sig.1) {
                    r.err(n, Some("sig"), "the signature's unit is 1, 2, 4, 8 or 16");
                }
                let params = param_list(obj.get("params"));
                if params.len() > crate::model::MAX_PARAMS {
                    r.err(n, Some("params"), format!("at most {} params", crate::model::MAX_PARAMS));
                }
                doc.clock = ClockDef {
                    name: name.clone(),
                    bpm: r.num(n, obj, "bpm", 120.0, 20.0, 400.0),
                    sig,
                    swing: r.num(n, obj, "swing", 0.0, -0.5, 0.75),
                    seed: r.num(n, obj, "seed", 1.0, 0.0, 4.0e15) as u64,
                    tempo: match obj.get("tempo") {
                        None | Some(V::Nil) => None,
                        Some(V::Obj(t)) => {
                            let param = t.get("param").and_then(V::text).unwrap_or("").to_string();
                            if !params.iter().any(|(p, _)| *p == param) {
                                r.err(n, Some("tempo"), format!("`tempo` follows a Clock param: no param `{param}`"));
                            }
                            let num = |key: &str, default: f64| t.get(key).and_then(V::num).unwrap_or(default);
                            Some(TempoFollow { param, from: num("from", 60.0).clamp(20.0, 400.0), to: num("to", 180.0).clamp(20.0, 400.0), ramp: num("ramp", 0.0).clamp(0.0, 256.0) })
                        }
                        Some(_) => {
                            r.err(n, Some("tempo"), "`tempo` is {param: @mood from: 64 to: 104 ramp: 4}");
                            None
                        }
                    },
                    osc_in: match obj.get("osc_in") {
                        None | Some(V::Nil) => None,
                        Some(V::Num(p)) if (1024.0..65536.0).contains(p) => Some(*p as u16),
                        Some(_) => {
                            r.err(n, Some("osc_in"), "`osc_in` is the UDP port OSC arrives on, 1024..65535 (like 9000)");
                            None
                        }
                    },
                    osc_out: match obj.get("osc_out") {
                        None | Some(V::Nil) => None,
                        Some(v) => match v.text().filter(|t| t.parse::<std::net::SocketAddr>().is_ok()) {
                            Some(t) => Some(t.to_string()),
                            None => {
                                r.err(n, Some("osc_out"), "`osc_out` is where osc_send sends, \"host:port\" (like \"127.0.0.1:57120\")");
                                None
                            }
                        },
                    },
                    midi: match obj.get("midi") {
                        None | Some(V::Nil) => Vec::new(),
                        Some(V::Obj(map)) => map.fields.iter().filter_map(|(param, v)| match v.num() {
                            Some(cc) if (0.0..128.0).contains(&cc) && params.iter().any(|(p, _)| p == param) => Some((param.clone(), cc as u8)),
                            _ => {
                                r.err(n, Some("midi"), format!("`midi` maps Clock params to MIDI CC numbers 0..127, like {{{param}: 74}}"));
                                None
                            }
                        }).collect(),
                        Some(_) => {
                            r.err(n, Some("midi"), "`midi` is {param: cc}: a MIDI controller moves the param");
                            Vec::new()
                        }
                    },
                    params,
                };
            }
            "Synth" => {
                let kind = r.synth(n, obj);
                doc.insts.push(InstDef { name: name.clone(), kind });
            }
            "Drums" => {
                let kind = r.drums(n, obj);
                doc.insts.push(InstDef { name: name.clone(), kind });
            }
            "Ironfish" => {
                use makepad_score_player::presets::{IRON_PARAMS, IRON_PROGRAMS};
                let program = match obj.get("program") {
                    None | Some(V::Nil) => 0,
                    Some(V::Num(n)) if (0.0..IRON_PROGRAMS.len() as f64).contains(n) => *n as usize,
                    Some(v) => match v.text().and_then(|t| IRON_PROGRAMS.iter().position(|p| *p == t)) {
                        Some(i) => i,
                        None => {
                            r.err(n, Some("program"), format!("`program` is 0-7 or one of {}", IRON_PROGRAMS.join(", ")));
                            0
                        }
                    },
                };
                let values = IRON_PARAMS.iter().map(|(field, _)| match obj.get(field) {
                    None | Some(V::Nil) => f32::NAN,
                    Some(_) => r.num(n, obj, field, 0.5, 0.0, 1.0) as f32,
                }).collect();
                doc.insts.push(InstDef { name: name.clone(), kind: InstKind::Ironfish { program, values } });
            }
            "Morph" => {
                let kind = r.morph(n, obj);
                doc.insts.push(InstDef { name: name.clone(), kind });
            }
            "Tandem" => {
                let kind = r.tandem(n, obj);
                doc.insts.push(InstDef { name: name.clone(), kind });
            }
            "Sampler" => {
                let src = r.asset(n, "src", obj.get("src")).unwrap_or(AssetRef { id: String::new(), rev: String::new() });
                let root_note = match obj.get("root") {
                    Some(v) => v.num().or_else(|| v.text().and_then(theory::note_number).map(|n| n as f64)).unwrap_or(60.0) as f32,
                    None => 60.0,
                };
                let attack = r.num(n, obj, "attack", 0.002, 0.0, 10.0) as f32;
                let release = r.num(n, obj, "release", 0.05, 0.0, 10.0) as f32;
                doc.insts.push(InstDef { name: name.clone(), kind: InstKind::Sampler { src, root: root_note, attack, release } });
            }
            "Loop" | "Stinger" => {
                let play = r.refs(n, "play", obj.get("play")).into_iter().next();
                let hits = match obj.get("hits") {
                    Some(V::Str(text)) => {
                        let steps = theory::pattern(text);
                        if steps.is_empty() {
                            r.err(n, Some("hits"), "an empty pattern");
                        }
                        let (drum, note) = match obj.get("note") {
                            Some(V::Id(name)) | Some(V::Str(name)) => match presets::drum(name) {
                                Some((slot, gm)) => (Some((slot as u8, gm)), 60.0),
                                None => match theory::note_number(name) {
                                    Some(note) => (None, note),
                                    None => {
                                        r.err(n, Some("note"), format!("`{name}` is neither a drum nor a note"));
                                        (Some((0, 36)), 60.0)
                                    }
                                },
                            },
                            Some(V::Num(note)) => (None, *note as f32),
                            _ => (Some((0, 36)), 60.0),
                        };
                        let step = r.num(n, obj, "step", 0.25, 1.0 / 64.0, 16.0);
                        Some(Hits { steps, drum, note, step })
                    }
                    None | Some(V::Nil) => None,
                    Some(_) => {
                        r.err(n, Some("hits"), "`hits` is a pattern string like \"x..x..x.\"");
                        None
                    }
                };
                let code = match obj.get("code") {
                    Some(V::Fn(index, _)) => Some(*index),
                    None | Some(V::Nil) => None,
                    Some(_) => {
                        r.err(n, Some("code"), "`code` is a function: code: fn(i) { … }");
                        None
                    }
                };
                let plays_input = play.as_deref().and_then(|p| r.outline.node(p)).and_then(|c| c.field("kind")).is_some_and(|f| f.value.text(r.src).trim() == "@input");
                if code.is_none() && hits.is_none() && obj.get("notes").is_none() && !plays_input && obj.get("midi_in").is_none_or(|v| matches!(v, V::Nil | V::Bool(false))) {
                    r.err(n, None, "a loop plays `hits: \"x..x\"`, runs `code: fn(i) { … }`, plays `notes: [[beat note dur vel] …]` or a MIDI keyboard (`midi_in: true`)");
                }
                let when = match obj.get("when") {
                    Some(V::Fn(index, _)) => Some(*index),
                    None | Some(V::Nil) => None,
                    Some(_) => {
                        r.err(n, Some("when"), "`when` is a function returning true or false");
                        None
                    }
                };
                let sync = match obj.get("sync") {
                    None | Some(V::Nil) => Sync::Bar,
                    Some(V::Id(s)) if s == "bar" => Sync::Bar,
                    Some(V::Id(s)) if s == "now" => Sync::Now,
                    Some(value) => match r.name_of(value) {
                        Some(other) => Sync::Loop(other),
                        None => {
                            r.err(n, Some("sync"), "`sync` is @bar, @now or another loop");
                            Sync::Bar
                        }
                    },
                };
                let follow = match obj.get("follow") {
                    Some(V::Obj(f)) => {
                        let param = f.get("param").and_then(V::text).unwrap_or("").to_string();
                        Some((param, f.get("from").and_then(V::num).unwrap_or(0.0) as f32, f.get("to").and_then(V::num).unwrap_or(1.0) as f32))
                    }
                    None | Some(V::Nil) => None,
                    Some(_) => {
                        r.err(n, Some("follow"), "`follow` is {param: @intensity from: 0.2 to: 0.8}");
                        None
                    }
                };
                let beats = match obj.get("beats") {
                    None | Some(V::Nil) => None,
                    Some(_) => Some(r.num(n, obj, "beats", 4.0, 1.0 / 16.0, 8192.0)),
                };
                let stinger = (ty == "Stinger").then(|| match obj.get("quant") {
                    Some(V::Id(q)) if q == "bar" => -1.0,
                    Some(V::Id(q)) if q == "now" => 0.0,
                    Some(V::Num(q)) => *q,
                    _ => 1.0,
                });
                let (scopes, unknown) = r.outline.node(n).map_or((Vec::new(), Vec::new()), |c| fx_scopes(c.body.text(r.src)));
                for name in unknown {
                    r.err(n, Some("code"), format!("fx(@{name}): no such effect; fx() takes {SCOPE_FX}"));
                }
                let mut list_opts: std::sync::Arc<[(f32, makepad_score_player::NoteOpts)]> = std::sync::Arc::new([]);
                doc.loops.push(LoopDef {
                    name: name.clone(),
                    play,
                    beats,
                    amp: r.num(n, obj, "amp", 0.8, 0.0, 4.0) as f32,
                    pan: r.num(n, obj, "pan", 0.0, -1.0, 1.0) as f32,
                    mute: r.flag(n, obj, "mute"),
                    solo: r.flag(n, obj, "solo"),
                    sync,
                    hits,
                    code,
                    when,
                    follow,
                    dur: r.num(n, obj, "dur", 0.5, 0.0, 64.0) as f32,
                    stinger,
                    notes: {
                        // Voices of `notes: [right_hand left_hand pedal]`
                        // muted or soloed: `mute_voices: [@left_hand]`.
                        let names = |key: &str| -> Vec<String> {
                            match obj.get(key) {
                                Some(V::Arr(items)) => items.iter().filter_map(|v| v.text().map(str::to_string)).collect(),
                                _ => Vec::new(),
                            }
                        };
                        let (muted, soloed) = (names("mute_voices"), names("solo_voices"));
                        let voices: Vec<String> = r.outline.node(n).and_then(|c| c.field("notes")).map(|f| {
                            let text = f.value.text(r.src).trim();
                            let inner = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')).unwrap_or("");
                            if inner.contains('[') { Vec::new() } else { inner.split(|c: char| c.is_whitespace() || c == ',').filter(|w| !w.is_empty()).map(str::to_string).collect() }
                        }).unwrap_or_default();
                        let keep = |i: usize| voices.get(i).is_none_or(|name| !muted.contains(name) && (soloed.is_empty() || soloed.contains(name)));
                        match r.note_list(n, obj.get("notes"), &keep) {
                            Some((notes, opts)) => {
                                list_opts = opts;
                                Some(notes)
                            }
                            None => None,
                        }
                    },
                    note_opts: list_opts.clone(),
                    swing: obj.get("swing").is_some().then(|| r.num(n, obj, "swing", 0.0, -0.5, 0.75)),
                    humanize: match obj.get("humanize") {
                        None | Some(V::Nil) => None,
                        Some(V::Obj(h)) => Some((
                            h.get("time").and_then(V::num).unwrap_or(0.01).clamp(0.0, 0.25),
                            h.get("vel").and_then(V::num).unwrap_or(0.1).clamp(0.0, 1.0) as f32,
                        )),
                        Some(_) => {
                            r.err(n, Some("humanize"), "`humanize` is {time: 0.01 vel: 0.1} (beats, a fraction of the velocity)");
                            None
                        }
                    },
                    sends: match obj.get("sends") {
                        None | Some(V::Nil) => Vec::new(),
                        Some(V::Obj(map)) => map.fields.iter().filter_map(|(fx, v)| match v.num() {
                            Some(amount) => Some((fx.clone(), amount.clamp(0.0, 4.0) as f32)),
                            None => {
                                r.err(n, Some("sends"), format!("`{fx}` in sends is an amount, like {{{fx}: 0.3}}"));
                                None
                            }
                        }).collect(),
                        Some(_) => {
                            r.err(n, Some("sends"), "`sends` is {verb: 0.3 echo: 0.15}: an FX and how much goes to it");
                            Vec::new()
                        }
                    },
                    scopes,
                    midi_in: match obj.get("midi_in") {
                        None | Some(V::Nil) | Some(V::Bool(false)) => None,
                        Some(V::Bool(true)) => Some(0),
                        Some(V::Num(c)) if (1.0..=16.0).contains(c) => Some(*c as u8),
                        Some(_) => {
                            r.err(n, Some("midi_in"), "`midi_in` is true (any channel) or a channel 1..16");
                            None
                        }
                    },
                });
            }
            "Section" => {
                let loops = r.refs(n, "loops", obj.get("loops"));
                // `repeat: true` or `next: @self` plays it again until a
                // goto; `next: @name` names a section anywhere in the score.
                let next = match obj.get("next") {
                    _ if r.flag(n, obj, "repeat") => Some(name.clone()),
                    None | Some(V::Nil) => None,
                    Some(V::Id(s)) if s == "stop" => Some("stop".to_string()),
                    Some(V::Id(s)) if s == "self" => Some(name.clone()),
                    Some(V::Id(s)) | Some(V::Str(s)) => Some(s.clone()),
                    Some(value) => r.name_of(value),
                };
                use makepad_audio_engine::feed::{TEMPO_MAX, TEMPO_MIN};
                let bpm = obj.get("bpm").is_some().then(|| r.num(n, obj, "bpm", 120.0, TEMPO_MIN, TEMPO_MAX));
                let ramp = r.num(n, obj, "ramp", 0.0, 0.0, 256.0);
                // `tempo: [[beat bpm] ...]`: beats from the section's start.
                let mut tempo = Vec::new();
                match obj.get("tempo") {
                    None | Some(V::Nil) => {}
                    Some(V::Arr(points)) => {
                        for point in points {
                            match point {
                                V::Arr(p) if p.len() == 2 && p[0].num().is_some_and(|b| b >= 0.0) && p[1].num().is_some() => {
                                    tempo.push((p[0].num().unwrap_or(0.0), p[1].num().unwrap_or(120.0).clamp(TEMPO_MIN, TEMPO_MAX)));
                                }
                                _ => {
                                    r.err(n, Some("tempo"), "`tempo` is a list of [beat bpm] (beats from the section's start)");
                                    break;
                                }
                            }
                        }
                    }
                    Some(_) => r.err(n, Some("tempo"), "`tempo` is a list of [beat bpm] (beats from the section's start)"),
                }
                tempo.sort_by(|a, b| a.0.total_cmp(&b.0));
                let set = match obj.get("set") {
                    None | Some(V::Nil) => Vec::new(),
                    Some(V::Obj(map)) => map.fields.iter().filter_map(|(param, v)| v.num().map(|x| (param.clone(), x as f32))).collect(),
                    Some(_) => {
                        r.err(n, Some("set"), "`set` is {param: value}: Clock params the section sets as it starts");
                        Vec::new()
                    }
                };
                doc.sections.push(SectionDef { name: name.clone(), bars: r.num(n, obj, "bars", 8.0, 0.25, 4096.0), loops, next, bpm, ramp, tempo, set });
            }
            "Instrument" => {
                let native = obj.get("kind").and_then(V::text).unwrap_or("piano").to_string();
                let font = match obj.get("src") {
                    None | Some(V::Nil) => None,
                    value => r.asset(n, "src", value),
                };
                if native == "input" {
                    let gain = r.num(n, obj, "gain", 1.0, 0.0, 4.0) as f32;
                    doc.insts.push(InstDef { name: name.clone(), kind: InstKind::Input { gain } });
                    continue;
                }
                if native == "midi" {
                    let channel = r.num(n, obj, "channel", 1.0, 1.0, 16.0) as u8;
                    let port = obj.get("port").and_then(V::text).unwrap_or("").to_string();
                    doc.insts.push(InstDef { name: name.clone(), kind: InstKind::Midi { channel, port } });
                    continue;
                }
                match native.as_str() {
                    "piano" | "drums" => {}
                    // No `src`: the General MIDI SoundFont.
                    "soundfont" => {}
                    other => r.err(n, Some("kind"), format!("`{other}` is not a native instrument: @piano, @drums, @soundfont, @midi or @input")),
                }
                let preset = obj.get("preset").and_then(V::text).map(str::to_string);
                if let (Some(p), "piano") = (&preset, native.as_str()) {
                    if !makepad_audio_synth::instruments::PianoInstrument::has_preset(p) {
                        let names: Vec<&str> = makepad_audio_synth::instruments::PianoInstrument::preset_names().collect();
                        r.err(n, Some("preset"), format!("no piano preset `{p}` (presets: {})", names.join(", ")));
                    }
                }
                let bank = r.num(n, obj, "bank", 0.0, 0.0, 128.0) as u16;
                // A GM program by number, or by name (`@strings`, `@french_horn`).
                let program = match obj.get("program").and_then(V::text) {
                    Some(named) => crate::gm_font::program(named).unwrap_or_else(|| {
                        r.err(n, Some("program"), format!("no General MIDI program `{named}`: a number 0..127 or a name like @strings, @brass, @choir, @french_horn"));
                        0
                    }),
                    None => r.num(n, obj, "program", 0.0, 0.0, 127.0) as u8,
                };
                let gain = r.num(n, obj, "gain", 1.0, 0.0, 4.0) as f32;
                doc.insts.push(InstDef { name: name.clone(), kind: InstKind::Native { native, preset, font, bank, program, gain } });
            }
            "Voice" => {
                // `engine: @neural` sings with the neural voice (its phrases
                // rendered ahead); else the realtime singer.
                let neural = match obj.get("engine") {
                    None | Some(V::Nil) => false,
                    Some(V::Id(e)) | Some(V::Str(e)) if e == "neural" => true,
                    Some(V::Id(e)) | Some(V::Str(e)) if e == "dsp" => false,
                    Some(_) => {
                        r.err(n, Some("engine"), "a Voice's engine is @dsp (the realtime singer, the default) or @neural (the neural voice)".to_string());
                        false
                    }
                };
                // A built-in voice by name, or a library singer asset(id, rev).
                let (profile, asset) = match obj.get("profile") {
                    None | Some(V::Nil) => ("alto".to_string(), None),
                    Some(v @ V::Obj(_)) => match r.asset(n, "profile", Some(v)) {
                        Some(asset) => (crate::player::sample_name(&asset), Some(asset)),
                        None => ("alto".to_string(), None),
                    },
                    Some(v) => (v.text().map(str::to_string).unwrap_or_default(), None),
                };
                if asset.is_none() && makepad_score_player::sung::Builtin::from_name(&profile).is_none() {
                    r.err(n, Some("profile"), format!("no singer `{profile}` (soprano, alto, tenor, baritone, bass, or a library singer: asset(\"<id>\", \"<rev>\"))"));
                }
                let singers = r.num(n, obj, "choir", 1.0, 1.0, 8.0) as u8;
                let parts = r.num(n, obj, "parts", 1.0, 1.0, 8.0) as u8;
                let ranges = [(0.0, 2.0), (0.5, 12.0), (0.0, 2.0), (0.0, 1.0), (0.0, 2.0), (0.0, 1.0), (0.0, 50.0), (0.0, 50.0)];
                let mut style = [f32::NAN; 8];
                for (i, field) in VOICE_STYLE.iter().enumerate() {
                    if obj.get(field).is_some() {
                        style[i] = r.num(n, obj, field, 0.0, ranges[i].0, ranges[i].1) as f32;
                    }
                }
                doc.insts.push(InstDef { name: name.clone(), kind: InstKind::Voice { profile, asset, singers, parts, style, neural } });
            }
            "Shader" => {
                if let Some(shader) = r.shader(n, obj, makepad_score_player::shader::Kind::Instrument) {
                    doc.insts.push(InstDef { name: name.clone(), kind: InstKind::Shader { shader, mono: r.flag(n, obj, "mono") } });
                }
            }
            "ShaderFx" => {
                if let Some(shader) = r.shader(n, obj, makepad_score_player::shader::Kind::Effect) {
                    let inputs = r.refs(n, "in", obj.get("in"));
                    doc.fx.push(FxDef { name: name.clone(), kind: FxKind::Bus, params: FxKind::Bus.defaults(), inputs, side: None, shader: Some(shader), follow: Vec::new() });
                }
            }
            other => match FxKind::parse(other) {
                Some(kind) => {
                    let mut params = kind.defaults();
                    let mut follow = Vec::new();
                    for (i, (param, ..)) in kind.params().iter().enumerate() {
                        match obj.get(param) {
                            // A setting that follows a Clock param (automation).
                            Some(V::Obj(f)) => {
                                let name = f.get("param").and_then(V::text).unwrap_or("").to_string();
                                let num = |key: &str, default: f64| f.get(key).and_then(V::num).unwrap_or(default);
                                let t = TempoFollow { param: name, from: num("from", params[i] as f64), to: num("to", params[i] as f64), ramp: num("ramp", 0.0).clamp(0.0, 256.0) };
                                params[i] = kind.clamp(i, t.from as f32);
                                follow.push((i, t));
                            }
                            Some(_) => params[i] = kind.clamp(i, r.num(n, obj, param, params[i] as f64, -1e9, 1e9) as f32),
                            None => {}
                        }
                    }
                    if kind == FxKind::Delay {
                        if let Some(V::Num(time)) = obj.get("time") {
                            params[0] = *time as f32;
                        }
                    }
                    // An Eq's bands, each `bN: [@peak, 900, -3, 1.4]` (shape,
                    // Hz, dB, Q; gain and Q may be left out).
                    if kind == FxKind::Eq {
                        for b in 0..4 {
                            let key = format!("b{}", b + 1);
                            match obj.get(&key) {
                                None | Some(V::Nil) => {}
                                Some(V::Arr(items)) => {
                                    let shape = items.first().and_then(V::text).unwrap_or("");
                                    match makepad_score_player::fx::EQ_TYPES.iter().position(|t| *t == shape) {
                                        Some(t) => {
                                            params[b * 4] = t as f32;
                                            for (j, v) in items.iter().skip(1).take(3).enumerate() {
                                                if let Some(x) = v.num() {
                                                    params[b * 4 + 1 + j] = kind.clamp(b * 4 + 1 + j, x as f32);
                                                }
                                            }
                                        }
                                        None => r.err(n, Some(&key), format!("`{key}` is [@shape, Hz, dB, Q]: a shape of @{}", makepad_score_player::fx::EQ_TYPES.join(" @"))),
                                    }
                                }
                                Some(_) => r.err(n, Some(&key), format!("`{key}` is [@peak, 900, -3, 1.4]: shape, Hz, dB, Q")),
                            }
                        }
                    }
                    let inputs = r.refs(n, "in", obj.get("in"));
                    let side = r.refs(n, "key", obj.get("key")).into_iter().next();
                    if kind == FxKind::Duck && side.is_none() {
                        r.err(n, None, "a Duck names what pushes it down: key: kick");
                    }
                    doc.fx.push(FxDef { name: name.clone(), kind, params, inputs, side, shader: None, follow });
                }
                None => r.err(n, None, format!("`{other}` is not a score node")),
            },
        }
    }
    if clocks == 0 {
        r.err("", None, "a score has a Clock{bpm: …}: add `let clock = Clock{bpm: 120}` and list it in Score{…}");
    } else if clocks > 1 {
        r.err("", None, "a score has one Clock");
    }
    for section in &doc.sections {
        if let Some(next) = section.next.as_deref().filter(|next| *next != "stop") {
            if !doc.sections.iter().any(|s| s.name == next) {
                r.err(&section.name, Some("next"), format!("`next: @{next}` is not a section (`@self` or `repeat: true` repeats this one)"));
            }
        }
    }
    // References must be members of the right kind.
    let is = |name: &str, kinds: &[&str]| doc.order.iter().any(|(n, t)| n == name && kinds.contains(&t.as_str()));
    let fx_names: Vec<&str> = doc.fx.iter().map(|f| f.name.as_str()).collect();
    for l in &doc.loops {
        match &l.play {
            Some(play) if !is(play, &["Synth", "Drums", "Sampler", "Ironfish", "Morph", "Tandem", "Shader", "Voice", "Instrument"]) => r.err(&l.name, Some("play"), format!("`{play}` is not an instrument in Score{{…}}")),
            None if l.hits.as_ref().is_none_or(|h| h.drum.is_none()) && l.code.is_some() => r.err(&l.name, Some("play"), "a loop names the instrument it plays: play: synth"),
            _ => {}
        }
        if let Sync::Loop(other) = &l.sync {
            if !is(other, &["Loop"]) {
                r.err(&l.name, Some("sync"), format!("`{other}` is not a loop"));
            }
        }
        if let Some((param, ..)) = &l.follow {
            if !doc.clock.params.iter().any(|(p, _)| p == param) {
                r.err(&l.name, Some("follow"), format!("no param `{param}` in the Clock's params"));
            }
        }
    }
    let param_names: Vec<&str> = doc.clock.params.iter().map(|(p, _)| p.as_str()).collect();
    for fx in &doc.fx {
        for (_, t) in &fx.follow {
            if !param_names.contains(&t.param.as_str()) {
                r.err(&fx.name, None, format!("no param `{}` in the Clock's params{}", t.param, crate::assist::hint(&t.param, param_names.iter().copied())));
            }
        }
    }
    for section in &doc.sections {
        for (p, _) in &section.set {
            if !param_names.contains(&p.as_str()) {
                r.err(&section.name, Some("set"), format!("no param `{p}` in the Clock's params{}", crate::assist::hint(p, param_names.iter().copied())));
            }
        }
    }
    for l in &doc.loops {
        for (target, _) in &l.sends {
            if !fx_names.contains(&target.as_str()) {
                r.err(&l.name, Some("sends"), format!("`{target}` is not an FX in Score{{…}}{}", crate::assist::hint(target, fx_names.iter().copied())));
            }
        }
    }
    for (i, fx) in doc.fx.iter().enumerate() {
        for input in fx.inputs.iter().chain(fx.side.iter()) {
            let is_loop = is(input, &["Loop", "Stinger"]);
            let fx_at = fx_names.iter().position(|n| n == input);
            if !is_loop && fx_at.is_none() {
                r.err(&fx.name, Some("in"), format!("`{input}` is not a loop or an FX in Score{{…}}"));
            } else if fx_at.is_some_and(|at| at >= i) {
                r.err(&fx.name, Some("in"), format!("`{input}` comes after `{}` in Score{{…}}: list sources first", fx.name));
            }
        }
    }
    for section in &doc.sections {
        for l in &section.loops {
            if !is(l, &["Loop"]) {
                r.err(&section.name, Some("loops"), format!("`{l}` is not a loop"));
            }
        }
    }
    doc.arrange = r.refs("", "arrange", root.get("arrange"));
    for name in doc.arrange.clone() {
        if !doc.sections.iter().any(|s| s.name == name) {
            r.err("", Some("arrange"), format!("`{name}` is not a Section"));
        }
    }
    if !r.diags.is_empty() {
        let mut diags = r.diags;
        diags.sort_by_key(|d| (d.line, d.col));
        diags.dedup();
        return Err(diags);
    }
    doc.outline = outline;
    Ok(Loaded { doc, vm: evaluated })
}

/// Game parameters a score may declare (`Clock{params: {...}}`).
pub const MAX_PARAMS: usize = 16;

/// A note list entry's options (after a comma: `[0 @a4 1 0.8, {…}]`),
/// `{pan bend vib arp arp_step env cutoff_env
/// curve rate start finish looped cutoff res attack decay sustain release
/// drive detune}`, as `play` reads them: (pan, opts).
fn list_note_opts(map: &Obj) -> (f32, makepad_score_player::NoteOpts) {
    let mut n = makepad_score_player::NoteOpts::default();
    let get = |k: &str| map.get(k).and_then(V::num).map(|v| v as f32);
    for (name, slot) in [
        ("cutoff", &mut n.cutoff), ("res", &mut n.res), ("attack", &mut n.attack), ("decay", &mut n.decay),
        ("sustain", &mut n.sustain), ("release", &mut n.release), ("drive", &mut n.drive), ("detune", &mut n.detune),
        ("rate", &mut n.rate), ("start", &mut n.start), ("finish", &mut n.finish),
    ] {
        if let Some(value) = get(name) {
            *slot = value;
        }
    }
    n.looped = matches!(map.get("looped"), Some(V::Bool(true)));
    let curve = get("curve").unwrap_or(1.0).clamp(0.1, 10.0);
    let list = |k: &str| -> Vec<f32> {
        let mut flat = Vec::new();
        if let Some(V::Arr(items)) = map.get(k) {
            for item in items {
                match item {
                    V::Arr(pair) => flat.extend(pair.iter().filter_map(V::num).map(|v| v as f32)),
                    v => flat.extend(v.num().map(|v| v as f32)),
                }
            }
        }
        flat
    };
    let env = |k: &str| -> makepad_score_player::Env {
        let mut env = makepad_score_player::Env { curve, ..Default::default() };
        let mut last = f32::NEG_INFINITY;
        for pair in list(k).chunks_exact(2).take(makepad_score_player::ENV_POINTS) {
            let t = pair[0].max(last).max(0.0);
            env.pts[env.n as usize] = [t, pair[1]];
            env.n += 1;
            last = t;
        }
        env
    };
    n.env = env("env");
    n.cutoff_env = env("cutoff_env");
    n.bend = env("bend");
    if n.env.n > 0 {
        for (slot, value) in [(&mut n.attack, 0.002), (&mut n.decay, 0.0), (&mut n.sustain, 1.0)] {
            if slot.is_nan() {
                *slot = value;
            }
        }
    }
    if n.cutoff_env.n > 0 && n.cutoff.is_nan() {
        n.cutoff = n.cutoff_env.pts[0][1];
    }
    if let [rate, depth, rest @ ..] = list("vib").as_slice() {
        n.vib_rate = rate.clamp(0.0, 100.0);
        n.vib_depth = depth.clamp(0.0, 24.0);
        n.vib_delay = rest.first().copied().unwrap_or(0.0).max(0.0);
    }
    let arp = list("arp");
    if !arp.is_empty() {
        for (slot, v) in n.arp.iter_mut().zip(&arp) {
            *slot = v.clamp(-48.0, 48.0);
        }
        n.arp_n = arp.len().min(makepad_score_player::ARP_STEPS) as u8;
        n.arp_step = get("arp_step").unwrap_or(0.0625).max(1e-3);
    }
    (get("pan").unwrap_or(0.0).clamp(-1.0, 1.0), n)
}
