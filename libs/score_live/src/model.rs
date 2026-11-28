//! The typed score: what a document declares, read from its evaluated
//! value, with every problem reported at its line and column. Loading
//! never plays anything; the composer and the graph builder read this.

use crate::source::{self, Outline};
use crate::theory;
use crate::vm::{self, Evaluated, Obj, V};
use makepad_core_util::ui_thread::WorkerOnly;
use makepad_audio_synth::recipe::{Layer, Recipe};
use makepad_flow::Literal;
use makepad_flow::live::owner::PrepareControl;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;
use makepad_score_player::{presets, shader::Steal, FxKind};

static COMPILE: WorkerOnly = WorkerOnly::new("a score document compile");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diag {
    pub line: u32,
    pub col: u32,
    pub node: Option<String>,
    pub message: String,
}

/// A warning at a score field and its byte range in the resolved shader source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShaderWarning {
    pub diag: Diag,
    pub start: usize,
    pub end: usize,
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
    Shader { shader: ShaderDef, mono: Option<bool>, legato: bool, glide: f32, steal: Steal },
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
    /// Side and parameter wires authored on an instrument definition.
    pub wires: Vec<(String, AudioSources)>,
    pub lib: Option<String>,
    pub code: String,
    pub overrides: Vec<(String, Override)>,
    /// Authored factor and conversion modes, empty when the shader selects its rate.
    pub oversample: Vec<Literal>,
    pub samples: Vec<(String, AssetRef)>,
    /// The node's random stream selector, zero by default.
    pub seed: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Override {
    Num(f64),
    Choice(String),
}

#[derive(Clone, Debug)]
pub struct CheckedShader {
    pub shader: makepad_score_player::shader::AudioShaderRef,
    pub params: Vec<(String, f32)>,
}

/// A choice as written in a score, including names that need quotes.
pub fn shader_choice(name: &str) -> String {
    if name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
        format!("@{name}")
    } else {
        format!("{name:?}")
    }
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
    /// The note stream it plays (`in: keys` or `in: arp`): a `MidiIn` or a
    /// stream node fed from one. A loop with a stream plays only it.
    pub input: Option<String>,
}

/// A note stream node: a keyboard (`MidiIn`) or what shapes what one plays
/// (`Arp`, `Chord`, `Transpose`, `Velocity`), each fed from another
/// (`in:`); loops play them (`Loop{play: lead in: arp}`).
#[derive(Clone, Debug, PartialEq)]
pub struct StreamNode {
    pub name: String,
    pub kind: StreamKind,
    /// The stream node it takes (none for a `MidiIn`).
    pub input: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StreamKind {
    /// MIDI in: the device (part of a port's name; "any" for every input),
    /// the channel (1..16; 0 for every one), the bend range (semitones),
    /// MPE (a channel per key) and the delay a message plays at (ms).
    MidiIn { device: String, channel: u8, bend: f32, mpe: bool, master: Option<u8>, latency: f32 },
    Arp { settings: makepad_audio_synth::kit::arp::ArpSettings, latch: bool, sync: bool },
    Chord { tones: Vec<i8> },
    Transpose { semis: i8 },
    Velocity { curve: makepad_score_player::stream::VelCurve, min: f32, max: f32 },
}

/// The stream node types.
pub const STREAM_TYPES: [&str; 5] = ["MidiIn", "Arp", "Chord", "Transpose", "Velocity"];

/// A loop's stream resolved: the `MidiIn` it starts from, the stages from
/// there in order (as the player runs them), and the nodes on the way
/// (the loop's input first).
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedStream {
    pub source: String,
    pub device: String,
    pub channel: u8,
    pub latency: f32,
    pub def: makepad_score_player::stream::StreamDef,
    pub path: Vec<String>,
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

/// Ordered audio sources. The node-name view is also used by instrument pickers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioSources {
    nodes: Vec<String>,
    named: std::collections::BTreeMap<usize, String>,
}

impl std::ops::Deref for AudioSources {
    type Target = [String];
    fn deref(&self) -> &Self::Target { &self.nodes }
}

impl AudioSources {
    pub fn ports(&self) -> impl Iterator<Item = (&str, &str)> {
        self.nodes.iter().enumerate().map(|(i, node)| {
            (node.as_str(), self.named.get(&i).map_or("audio", String::as_str))
        })
    }

    // Bare loop audio and Port{node: loop name: @audio} are distinct when
    // the instrument declares an output called audio.
    pub(crate) fn graph_ports<'a>(&'a self, doc: &'a ScoreDoc) -> impl Iterator<Item = (&'a str, &'a str)> {
        self.ports().enumerate().map(move |(i, (node, port))| {
            if matches!(port, "audio" | "peak") && self.named.contains_key(&i) {
                let shader = doc.loops.iter().find(|l| l.name == node).and_then(|l| l.play.as_deref()).and_then(|n| doc.shader(n));
                if shader.is_some_and(|s| (0..s.shader.port_count()).map(|i| s.shader.port_view(i)).any(|p| p.output && p.name == port)) {
                    return (node, makepad_audio_engine::nodes::instrument_output(port));
                }
            }
            (node, port)
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FxDef {
    pub name: String,
    pub kind: FxKind,
    pub params: [f32; makepad_score_player::fx::MAX_FX_PARAMS],
    pub inputs: AudioSources,
    /// Named side inputs, including a native effect's key, and parameter wires.
    pub sides: Vec<(String, AudioSources)>,
    /// A shader FX (`ShaderFx{…}`): its program; `kind` is then Bus.
    pub shader: Option<ShaderDef>,
    /// Settings that follow a Clock param: (setting index, how).
    pub follow: Vec<(usize, TempoFollow)>,
}

pub(crate) fn audio_source_type(ty: &str) -> bool {
    matches!(ty, "Loop" | "Stinger" | "ShaderFx" | "ShaderControl") || FxKind::parse(ty).is_some()
}

pub(crate) fn native_param(ty: &str, field: &str) -> bool {
    FxKind::parse(ty).and_then(|kind| kind.param_index(field)).is_some()
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
pub struct AuthoredDoc {
    pub source: String,
    /// Host modules available to every audio shader in the score.
    pub shader_modules: std::collections::BTreeMap<String, String>,
    pub label: String,
    pub clock: ClockDef,
    pub insts: Vec<InstDef>,
    pub loops: Vec<LoopDef>,
    pub fx: Vec<FxDef>,
    /// Note streams: MIDI in and what shapes it.
    pub streams: Vec<StreamNode>,
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

impl AuthoredDoc {
    pub fn inst(&self, name: &str) -> Option<&InstDef> {
        self.insts.iter().find(|inst| inst.name == name)
    }

    pub fn shader_def(&self, node: &str) -> Option<&ShaderDef> {
        if let Some(inst) = self.inst(node) {
            return match &inst.kind {
                InstKind::Shader { shader, .. } => Some(shader),
                _ => None,
            };
        }
        self.fx.iter().find(|fx| fx.name == node)?.shader.as_ref()
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

    pub fn stream(&self, name: &str) -> Option<&StreamNode> {
        self.streams.iter().find(|s| s.name == name)
    }

    /// The stream loop `l` plays, back to its `MidiIn`; an error says why
    /// it does not resolve (a loop, a missing source, too many stages).
    pub fn resolve_stream(&self, l: &LoopDef) -> Option<Result<ResolvedStream, String>> {
        use makepad_score_player::stream::{StageDef, StreamDef, MAX_STAGES, MAX_TONES};
        let start = l.input.as_deref()?;
        let mut path: Vec<String> = Vec::new();
        let mut at = start.to_string();
        let mut stages: Vec<StageDef> = Vec::new();
        loop {
            if path.contains(&at) {
                return Some(Err(format!("the stream goes round in a circle through `{at}`")));
            }
            let Some(node) = self.stream(&at) else { return Some(Err(format!("`{at}` is not a stream node (MidiIn, Arp, Chord, Transpose, Velocity)"))) };
            path.push(at.clone());
            match &node.kind {
                StreamKind::MidiIn { device, channel, bend, mpe, master, latency } => {
                    if stages.len() > MAX_STAGES {
                        return Some(Err(format!("a stream passes at most {MAX_STAGES} nodes on its way")));
                    }
                    stages.reverse();
                    let mut def = StreamDef { bend_range: *bend, mpe: *mpe, master: *master, ..Default::default() };
                    for (slot, stage) in def.stages.iter_mut().zip(stages) {
                        *slot = Some(stage);
                    }
                    return Some(Ok(ResolvedStream { source: at, device: device.clone(), channel: *channel, latency: *latency, def, path }));
                }
                StreamKind::Arp { settings, latch, sync } => stages.push(StageDef::Arp { settings: *settings, latch: *latch, sync: *sync }),
                StreamKind::Chord { tones } => {
                    let mut t = [0i8; MAX_TONES];
                    let tones: Vec<i8> = tones.iter().copied().filter(|t| *t != 0).take(MAX_TONES).collect();
                    t[..tones.len()].copy_from_slice(&tones);
                    stages.push(StageDef::Chord { tones: t, n: tones.len() as u8 });
                }
                StreamKind::Transpose { semis } => stages.push(StageDef::Transpose(*semis)),
                StreamKind::Velocity { curve, min, max } => stages.push(StageDef::Velocity { curve: *curve, min: *min, max: *max }),
            }
            match &node.input {
                Some(next) => at = next.clone(),
                None => return Some(Err(format!("`{at}` takes a stream: in: keys (a MidiIn)"))),
            }
        }
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

    /// Recordings played by instruments, kit slots and shader tables.
    pub fn samples(&self) -> Vec<AssetRef> {
        let mut out: Vec<AssetRef> = Vec::new();
        for inst in &self.insts {
            match &inst.kind {
                InstKind::Sampler { src, .. } => out.push(src.clone()),
                InstKind::Morph { srcs, .. } => out.extend(srcs.iter().flatten().cloned()),
                InstKind::Drums { slots, .. } => out.extend(slots.iter().filter_map(|s| s.sample.clone())),
                InstKind::Shader { shader, .. } => out.extend(shader.samples.iter().map(|(_, asset)| asset.clone())),
                InstKind::Synth { .. } | InstKind::Tandem { .. } | InstKind::Ironfish { .. } | InstKind::Voice { .. } | InstKind::Native { .. } | InstKind::Midi { .. } | InstKind::Input { .. } => {}
            }
        }
        for shader in self.fx.iter().filter_map(|fx| fx.shader.as_ref()) {
            out.extend(shader.samples.iter().map(|(_, asset)| asset.clone()));
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

    /// Whether it names patches of the optional patch library that this
    /// machine does not have: they play the plain starting sound until the
    /// library is installed (Resources).
    pub fn lacks_patch_library(&self) -> bool {
        self.missing_library_patches().next().is_some()
    }

    /// The library patches it names that this machine does not have, as
    /// (instrument, patch): each plays the plain starting sound.
    pub fn missing_library_patches(&self) -> impl Iterator<Item = (&str, &str)> {
        self.insts.iter().filter_map(|inst| match &inst.kind {
            InstKind::Tandem { patch, path, .. } if !patch.is_empty() && path.is_empty() => Some((inst.name.as_str(), patch.as_str())),
            _ => None,
        })
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

/// A successful load adds resolved programs and diagnostics to the authored revision.
#[derive(Clone, Debug)]
pub struct ScoreDoc {
    authored: Arc<AuthoredDoc>,
    checked: Arc<[(String, CheckedShader)]>,
    warnings: Arc<[ShaderWarning]>,
}

impl std::ops::Deref for ScoreDoc {
    type Target = AuthoredDoc;
    fn deref(&self) -> &AuthoredDoc { &self.authored }
}

impl PartialEq for ScoreDoc {
    fn eq(&self, other: &Self) -> bool {
        self.authored == other.authored && self.checked.iter().map(|(_, s)| &s.params)
            .eq(other.checked.iter().map(|(_, s)| &s.params))
    }
}

impl ScoreDoc {
    pub fn shader(&self, node: &str) -> Option<&CheckedShader> {
        self.checked.iter().find(|(name, _)| name == node).map(|(_, shader)| shader)
    }

    pub fn warnings(&self) -> &[ShaderWarning] { &self.warnings }

    pub fn is_control(&self, fx: &FxDef) -> bool {
        fx.shader.is_some() && self.shader(&fx.name).is_some_and(|s| s.shader.kind() == makepad_score_player::shader::Kind::Control)
    }
}

pub struct EvaluatedDocument {
    pub doc: Arc<AuthoredDoc>,
    pub vm: Evaluated,
}

/// A loaded score: the typed document and the live VM its functions run in.
pub struct Loaded {
    pub doc: ScoreDoc,
    pub vm: Evaluated,
}

impl Loaded {
    pub fn into_evaluated(self) -> EvaluatedDocument {
        EvaluatedDocument { doc: self.doc.authored, vm: self.vm }
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        Arc::make_mut(&mut self.doc.authored).clock.seed = seed;
        self
    }
}

/// A cancelled request publishes neither a document nor diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    Problems(Vec<Diag>),
    /// The caller's instruction budget ended this evaluation.
    Unfinished { line: Option<u32> },
    /// The script heap could not grow during document evaluation.
    Memory { line: Option<u32> },
    Cancelled,
}

impl LoadError {
    /// Surface a refusal in hosts whose status only carries diagnostics.
    pub(crate) fn into_diagnostics(self) -> Vec<Diag> {
        match self {
            Self::Problems(diags) => diags,
            Self::Unfinished { line } | Self::Memory { line } => vec![Diag { line: line.unwrap_or(0), col: 0, node: None, message: self.to_string() }],
            Self::Cancelled => Vec::new(),
        }
    }

    /// Format problems for the caller's diagnostic view, preserving cancellation as an answer.
    pub fn describe(self, problems: impl FnOnce(Vec<Diag>) -> String) -> String {
        match self {
            Self::Problems(diags) => problems(diags),
            Self::Unfinished { .. } | Self::Memory { .. } => self.to_string(),
            Self::Cancelled => "the request was cancelled".into(),
        }
    }
}

impl From<Vec<Diag>> for LoadError {
    fn from(problems: Vec<Diag>) -> Self { Self::Problems(problems) }
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Unfinished { line } | Self::Memory { line } => {
                f.write_str(if matches!(self, Self::Memory { .. }) {
                    "Evaluation ran out of memory"
                } else {
                    "evaluation unfinished (instruction budget reached)"
                })?;
                if let Some(line) = line { write!(f, " at line {line}")?; }
                Ok(())
            }
            Self::Cancelled => f.write_str("the request was cancelled"),
            Self::Problems(problems) => {
                for (i, problem) in problems.iter().enumerate() {
                    if i != 0 { writeln!(f)?; }
                    write!(f, "{problem}")?;
                }
                Ok(())
            }
        }
    }
}

impl From<Loaded> for EvaluatedDocument {
    fn from(loaded: Loaded) -> Self { loaded.into_evaluated() }
}

/// One slice of a checked score load.
pub enum LoadStep {
    Done(Result<Loaded, LoadError>),
    Pending(PendingLoad),
}

/// The suspended VM and the source its document will be read against.
pub struct PendingLoad {
    evaluation: vm::PendingEvaluation,
    source: String,
    control: PrepareControl,
}

impl PendingLoad {
    pub fn step(self, slice: Duration) -> LoadStep {
        match self.evaluation.step(slice) {
            vm::EvalStep::Pending(evaluation) => LoadStep::Pending(Self { evaluation, ..self }),
            vm::EvalStep::Done(result) => LoadStep::Done(finish_load(&self.source, &self.control, result)),
        }
    }
}

/// Start the document's evaluation. Read and check it in the slice that finishes it.
pub fn load_begin(source: &str, control: &PrepareControl, slice: Duration) -> LoadStep {
    match vm::evaluate_begin(source, control, slice) {
        vm::EvalStep::Done(result) => LoadStep::Done(finish_load(source, control, result)),
        vm::EvalStep::Pending(evaluation) => LoadStep::Pending(PendingLoad {
            evaluation, source: source.to_owned(), control: control.clone(),
        }),
    }
}

/// A frame job's load: start on the first poll, resume once per poll, and
/// consume the completed document on the thread that evaluated it.
pub fn load_steps<T>(
    source: impl AsRef<str>, control: PrepareControl, slice: Duration,
    mut finish: impl FnMut(&str, Result<Loaded, LoadError>, &PrepareControl) -> T,
) -> impl FnMut() -> Poll<T> {
    let mut pending: Option<PendingLoad> = None;
    move || {
        let step = match pending.take() {
            Some(load) => load.step(slice),
            None => load_begin(source.as_ref(), &control, slice),
        };
        match step {
            LoadStep::Done(result) => Poll::Ready(finish(source.as_ref(), result, &control)),
            LoadStep::Pending(load) => { pending = Some(load); Poll::Pending }
        }
    }
}

fn finish_load(source: &str, control: &PrepareControl, result: Result<Evaluated, LoadError>) -> Result<Loaded, LoadError> {
    read_evaluated(source, result?, control, None, &mut false, true).map(checked_document)
}

pub fn load_controlled(source: &str, control: &PrepareControl) -> Result<Loaded, LoadError> {
    vm::drive(load(source, control))
}

/// Evaluate the document in slices, then read it and check its programs.
pub async fn load(source: &str, control: &PrepareControl) -> Result<Loaded, LoadError> {
    load_with_slice(source, control, vm::EVAL_SLICE).await
}

pub(crate) async fn load_with_slice(source: &str, control: &PrepareControl, slice: Duration) -> Result<Loaded, LoadError> {
    let evaluated = vm::evaluate_with_slice(source, control, slice).await;
    finish_load(source, control, evaluated)
}

impl ScoreDoc {
    pub fn load_controlled(source: &str, control: &PrepareControl) -> Result<Self, LoadError> {
        vm::drive(Self::load(source, control))
    }

    pub async fn load(source: &str, control: &PrepareControl) -> Result<Self, LoadError> {
        load(source, control).await.map(|loaded| loaded.doc)
    }
}

/// Programs produced while reading the shader definitions, in document order.
#[derive(Debug, Default)]
pub struct CompiledShaders {
    modules: std::collections::BTreeMap<String, String>,
    entries: Vec<(String, ShaderDef, CheckedShader)>,
}

fn score_root(evaluated: &Evaluated) -> Result<&Obj, Vec<Diag>> {
    if !evaluated.errors.is_empty() {
        let diags = evaluated.errors.iter().map(|e| {
            let (line, col, message) = vm::split_error(e);
            Diag { line, col, node: None, message }
        }).collect();
        return Err(diags);
    }
    match &evaluated.root {
        V::Obj(root) if root.proto.as_deref() == Some("Score") => Ok(root),
        _ => Err(vec![Diag { line: 1, col: 1, node: None, message: "a score ends with `Score{…}` listing its nodes".into() }]),
    }
}

fn score_label(root: &Obj) -> Option<String> {
    root.get("label").and_then(V::text).filter(|s| !s.trim().is_empty()).map(str::to_owned)
}

/// Evaluate the score's label without compiling its instruments.
pub fn evaluated_label_controlled(source: &str, control: &PrepareControl) -> Result<Option<String>, LoadError> {
    vm::drive(evaluated_label(source, control))
}

pub async fn evaluated_label(source: &str, control: &PrepareControl) -> Result<Option<String>, LoadError> {
    let evaluated = vm::evaluate(source, control).await?;
    Ok(score_label(score_root(&evaluated)?))
}

fn object_name(source: &str, outline: &Outline, obj: &Obj) -> Option<String> {
    let (line, col) = obj.loc?;
    let at = source::offset_of(source, line as usize, col as usize);
    outline.nodes.iter().filter(|n| n.span.start <= at && at < n.span.end)
        .min_by_key(|n| n.span.end - n.span.start).map(|n| n.name.clone())
}

/// Read the tempo and instrument descriptions of an already-written score.
/// Its VM resolves computed field values. No program or player is needed.
pub fn evaluated_palette_controlled(source: &str, control: &PrepareControl) -> Result<Option<(f64, Vec<String>)>, LoadError> {
    vm::drive(evaluated_palette(source, control))
}

pub async fn evaluated_palette(source: &str, control: &PrepareControl) -> Result<Option<(f64, Vec<String>)>, LoadError> {
    let evaluated = vm::evaluate(source, control).await?;
    Ok(score_palette(source, score_root(&evaluated)?))
}

fn score_palette(source: &str, root: &Obj) -> Option<(f64, Vec<String>)> {
    let outline = source::outline(source);
    let mut bpm = 120.0;
    let mut palette = Vec::new();
    for value in root.fields.iter().map(|(_, v)| v).chain(&root.items) {
        let V::Obj(obj) = value else { continue };
        let Some(kind) = obj.proto.as_deref() else { continue };
        if kind == "Clock" {
            bpm = obj.get("bpm").and_then(V::num).unwrap_or(120.0).clamp(20.0, 400.0);
        }
        if !crate::instruments::INSTRUMENT_TYPES.contains(&kind) { continue; }
        let name = object_name(source, &outline, obj)?;
        let text = |field| obj.get(field).and_then(V::text);
        palette.push(match kind {
            "Synth" | "Ironfish" | "Voice" => format!("{name} ({})", kind.to_ascii_lowercase()),
            "Drums" => {
                let kit = match obj.get("kit") {
                    Some(V::Obj(_)) => "custom",
                    value => value.and_then(V::text).unwrap_or(crate::palette::sounds::DEFAULT_KIT),
                };
                format!("drums {kit}")
            }
            "Instrument" => match text("kind").unwrap_or("piano") {
                "soundfont" => {
                    let program = text("program").and_then(crate::gm_font::program)
                        .unwrap_or_else(|| obj.get("program").and_then(V::num).unwrap_or(0.0).clamp(0.0, 127.0) as u8);
                    format!("{name} (GM {program})")
                }
                "midi" | "input" => name,
                native => format!("{name} ({native})"),
            },
            "Shader" => format!("{name} (shader {})", text("lib").unwrap_or("own")),
            _ => name,
        });
    }
    Some((bpm, palette))
}

impl CompiledShaders {
    pub(crate) fn from_doc(doc: &ScoreDoc) -> Arc<Self> {
        let entries = doc.checked.iter().map(|(name, shader)|
            (name.clone(), doc.shader_def(name).unwrap().clone(), shader.clone())).collect();
        Arc::new(Self { modules: doc.shader_modules.clone(), entries })
    }

    /// Re-evaluate functions and captures in the calling VM, using this revision's programs.
    pub fn evaluate_controlled(&self, source: &str, control: &PrepareControl) -> Result<Loaded, LoadError> {
        vm::drive(self.evaluate(source, control))
    }

    pub async fn evaluate(&self, source: &str, control: &PrepareControl) -> Result<Loaded, LoadError> {
        read_parts(source, control, Some(self), &mut false, true, vm::EVAL_SLICE).await.map(checked_document)
    }

    /// Read a rewritten score without compiling. A changed shader definition
    /// needs a new program. Ordinary score diagnostics keep their meaning.
    #[cfg(test)]
    pub(crate) fn evaluate_rewrite(&self, source: &str, control: &PrepareControl) -> Result<Option<Loaded>, LoadError> {
        vm::drive(self.evaluate_rewrite_async(source, control))
    }

    pub(crate) async fn evaluate_rewrite_async(&self, source: &str, control: &PrepareControl) -> Result<Option<Loaded>, LoadError> {
        self.evaluate_rewrite_with_slice(source, control, vm::EVAL_SLICE).await
    }

    pub(crate) async fn evaluate_rewrite_with_slice(&self, source: &str, control: &PrepareControl, slice: Duration) -> Result<Option<Loaded>, LoadError> {
        let mut changed = false;
        match read_parts(source, control, Some(self), &mut changed, true, slice).await {
            Err(LoadError::Problems(_)) if changed => Ok(None),
            result => result.map(|parts| Some(checked_document(parts))),
        }
    }
}

/// The worker owns the evaluation and drops the VM before returning to the editor.
#[derive(Clone)]
pub struct RevisionLoad {
    pub revision: u64,
    pub source: Arc<str>,
    control: PrepareControl,
}

pub enum LoadResult {
    Ready(ScoreDoc, Arc<CompiledShaders>),
    Invalid(Vec<Diag>),
    Cancelled,
}

impl RevisionLoad {
    /// True while this revision is running its document code.
    pub fn evaluating(&self) -> bool { self.control.script_running() }

    /// True while this revision is executing shader constants or table initialization.
    pub fn executing(&self) -> bool { self.control.executing() }

    /// Start and finish a revision on the thread driving its slices.
    pub fn steps(self, slice: Duration) -> impl FnMut() -> Poll<LoadResult> {
        load_steps(self.source, self.control, slice, |_, result, _| Self::finish(result))
    }

    fn finish(result: Result<Loaded, LoadError>) -> LoadResult {
        match result {
            Ok(loaded) => {
                let shaders = CompiledShaders::from_doc(&loaded.doc);
                LoadResult::Ready(loaded.doc, shaders)
            }
            Err(LoadError::Cancelled) => LoadResult::Cancelled,
            Err(error) => LoadResult::Invalid(error.into_diagnostics()),
        }
    }

    pub fn run(&self) -> LoadResult {
        Self::finish(load_controlled(&self.source, &self.control))
    }
}

/// Work an editor has observed for long enough to display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadActivity {
    Evaluating,
    Initialising,
}

#[derive(Default)]
pub struct LoadProgress {
    evaluating_since: Option<(u64, f64)>,
    initialising_since: Option<f64>,
}

impl LoadProgress {
    /// Sample the pending document and the player's independent device-rate prepare.
    pub fn sample(&mut self, load: Option<&RevisionLoad>, slot_initialising: bool, now: f64, delay: f64) -> Option<LoadActivity> {
        let initialising = slot_initialising || load.is_some_and(RevisionLoad::executing);
        if initialising {
            self.initialising_since.get_or_insert(now);
        } else {
            self.initialising_since = None;
        }

        if let Some(load) = load.filter(|load| load.evaluating()) {
            let (revision, since) = self.evaluating_since.get_or_insert((load.revision, now));
            if *revision != load.revision {
                (*revision, *since) = (load.revision, now);
            }
            if now - *since >= delay { return Some(LoadActivity::Evaluating); }
        } else {
            self.evaluating_since = None;
        }
        self.initialising_since.filter(|since| now - since >= delay).map(|_| LoadActivity::Initialising)
    }
}

/// One editor's outstanding load and its most recently applied result.
pub struct RevisionLoads<J> {
    wanted: Option<RevisionLoad>,
    job: Option<J>,
    stopped: Option<u64>,
    pub applied: Option<RevisionLoad>,
    pub diags: Vec<Diag>,
    pub warnings: Arc<[ShaderWarning]>,
    pub shaders: Option<Arc<CompiledShaders>>,
}

impl<J> Default for RevisionLoads<J> {
    fn default() -> Self {
        Self { wanted: None, job: None, stopped: None, applied: None, diags: Vec::new(), warnings: Arc::default(), shaders: None }
    }
}

impl<J> RevisionLoads<J> {
    pub fn submit(&mut self, revision: u64, source: &str, cancel: impl FnOnce(J)) -> bool {
        if self.stopped(revision) || self.wanted.as_ref().or(self.applied.as_ref()).is_some_and(|r| r.revision == revision) { return false; }
        self.stop(cancel);
        self.stopped = None;
        self.wanted = Some(RevisionLoad { revision, source: Arc::from(source), control: PrepareControl::default() });
        true
    }

    /// Stop the wanted revision, even while it is waiting for a submission slot.
    pub fn stop(&mut self, cancel: impl FnOnce(J)) {
        if let Some(request) = self.wanted.take() {
            request.control.cancel();
            self.stopped = Some(request.revision);
        }
        if let Some(job) = self.job.take() { cancel(job); }
    }

    pub fn stopped(&self, revision: u64) -> bool { self.stopped == Some(revision) }

    /// A refused pool submission stays pending for the next frame.
    pub fn start(&mut self, start: impl FnOnce(RevisionLoad) -> Option<J>) {
        if self.job.is_none() {
            if let Some(wanted) = &self.wanted { self.job = start(wanted.clone()); }
        }
    }

    pub fn take(&mut self, take: impl FnOnce(&mut J) -> Option<LoadResult>) -> Option<ScoreDoc> {
        let result = take(self.job.as_mut()?)?;
        self.job = None;
        let request = self.wanted.take()?;
        if request.control.is_cancelled() { return None; }
        match result {
            LoadResult::Ready(doc, shaders) => {
                self.applied = Some(request);
                self.diags.clear();
                self.warnings = doc.warnings.clone();
                self.shaders = Some(shaders);
                Some(doc)
            }
            LoadResult::Invalid(diags) => {
                self.applied = Some(request);
                self.diags = diags;
                self.warnings = Arc::default();
                None
            }
            LoadResult::Cancelled => None,
        }
    }

    pub fn pending(&self) -> bool { self.wanted.is_some() }
}

impl<J> Drop for RevisionLoads<J> {
    fn drop(&mut self) {
        if let Some(request) = &self.wanted { request.control.cancel(); }
    }
}

// Per test thread. The one-shot hook synchronizes a worker already reading a shader.
#[cfg(any(test, feature = "test-compile-hook"))]
thread_local! {
    #[cfg(test)]
    pub(crate) static SHADER_COMPILES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static ON_SHADER_COMPILE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(feature = "test-compile-hook")]
#[doc(hidden)]
pub fn on_next_shader_compile(hook: impl FnOnce() + 'static) {
    ON_SHADER_COMPILE.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

fn compile_shader(code: &str, bindings: &makepad_score_player::shader::AudioBindings<'_>)
    -> Result<makepad_score_player::shader::AudioShaderRef, Vec<makepad_score_player::shader::CompileError>>
{
    #[cfg(test)]
    SHADER_COMPILES.with(|count| count.set(count.get() + 1));
    #[cfg(any(test, feature = "test-compile-hook"))]
    ON_SHADER_COMPILE.with(|hook| { if let Some(f) = hook.take() { f(); } });
    makepad_score_player::shader::compile_with(code, bindings)
}

use makepad_score_player::shader;

struct ShaderFields<'a> {
    def: ShaderDef,
    bindings: shader::AudioBindings<'a>,
    ports: Vec<(String, bool)>,
    param_wires: Vec<String>,
    numeric_errors: Vec<(usize, String)>,
    bad_seed: bool,
    invalid_inventory: bool,
    diags: Vec<Diag>,
}

struct Reader<'a> {
    control: &'a PrepareControl,
    reuse: Option<&'a CompiledShaders>,
    changed_shader: &'a mut bool,
    shader_index: usize,
    src: &'a str,
    outline: &'a Outline,
    diags: Vec<Diag>,
    warnings: Vec<ShaderWarning>,
}

impl Reader<'_> {
    fn cancelled(&self) -> bool { self.control.is_cancelled() }

    fn shader_mismatch(&mut self, node: &str, why: &str) {
        *self.changed_shader = true;
        self.err(node, None, why);
    }

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
        object_name(self.src, self.outline, obj)
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

    fn audio_sources(&mut self, node: &str, field: &str, value: Option<&V>) -> AudioSources {
        let values = match value {
            None | Some(V::Nil) => &[][..],
            Some(V::Arr(items)) => items.as_slice(),
            Some(item) => std::slice::from_ref(item),
        };
        let mut sources = AudioSources::default();
        for item in values {
            let mut target = item;
            let mut port = None;
            if let V::Obj(obj) = item {
                // A plain Clock-follow object or a Port extended with its
                // fields cannot supply automation and an audio wire together.
                if matches!(obj.proto.as_deref(), None | Some("Port")) && obj.get("param").is_some()
                    && self.outline.node(node).is_some_and(|n| native_param(&n.type_name, field)) {
                    self.err(node, Some(field), format!("choose either automation or source wires for `{field}`"));
                    continue;
                }
                if obj.proto.as_deref() == Some("Port") {
                    let (Some(source), Some(V::Id(name))) = (obj.get("node"), obj.get("name")) else {
                        self.err(node, Some(field), format!("use Port{{node: source name: @port}} on `{field}`"));
                        continue;
                    };
                    target = source;
                    port = Some(name);
                }
            }
            if let Some(id) = self.name_of(target) {
                if let Some(port) = port { sources.named.insert(sources.nodes.len(), port.to_string()); }
                sources.nodes.push(id);
            } else {
                self.err(node, Some(field), format!("each source on `{field}` names a node or its output port"));
            }
        }
        sources
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

    /// A stream node's settings.
    fn stream(&mut self, n: &str, ty: &str, obj: &Obj) -> StreamKind {
        use makepad_audio_synth::kit::arp::{self, ArpSettings};
        use makepad_score_player::stream::VelCurve;
        let choice = |r: &mut Self, key: &str, names: &[&str], default: usize| -> usize {
            match obj.get(key) {
                None | Some(V::Nil) => default,
                Some(v) => match v.text().and_then(|t| names.iter().position(|p| *p == t)) {
                    Some(i) => i,
                    None => {
                        r.err(n, Some(key), format!("`{key}` is one of @{}", names.join(" @")));
                        default
                    }
                },
            }
        };
        match ty {
            "MidiIn" => StreamKind::MidiIn {
                device: match obj.get("device") {
                    None | Some(V::Nil) => "any".to_string(),
                    Some(v) => match v.text() {
                        Some(t) => t.to_string(),
                        None => {
                            self.err(n, Some("device"), "`device` names an input: \"any\", or part of its name (\"Keystation\")");
                            "any".to_string()
                        }
                    },
                },
                channel: self.num(n, obj, "channel", 0.0, 0.0, 16.0) as u8,
                bend: self.num(n, obj, "bend", 2.0, 0.0, 48.0) as f32,
                mpe: self.flag(n, obj, "mpe"),
                master: match obj.get("master") {
                    None => Some(0),
                    Some(V::Nil) => None,
                    Some(v) if v.text() == Some("none") => None,
                    Some(V::Num(ch)) if ch.fract() == 0.0 && (0.0..=15.0).contains(ch) => Some(*ch as u8),
                    _ => {
                        self.err(n, Some("master"), "`master` takes a channel from 0 to 15 or @none");
                        None
                    }
                },
                latency: self.num(n, obj, "latency", 8.0, 0.0, 100.0) as f32,
            },
            "Arp" => {
                let pattern = choice(self, "pattern", arp::PATTERNS, 0);
                // `rate: 1/16` is a sixteenth note: a quarter of a beat.
                let rate = self.num(n, obj, "rate", 1.0 / 16.0, 1.0 / 256.0, 4.0);
                let settings = ArpSettings {
                    pattern,
                    step: rate * 4.0,
                    octaves: self.num(n, obj, "octaves", 1.0, 1.0, 4.0) as usize,
                    gate: self.num(n, obj, "gate", 0.6, 0.01, 1.0) as f32,
                    swing: self.num(n, obj, "swing", 0.0, 0.0, 0.5) as f32,
                    repeats: self.num(n, obj, "repeats", 1.0, 1.0, 4.0) as usize,
                    chance: self.num(n, obj, "chance", 1.0, 0.0, 1.0) as f32,
                    transpose: self.num(n, obj, "interval", 12.0, -24.0, 24.0) as i32,
                };
                let sync = !matches!(obj.get("sync"), Some(V::Bool(false)));
                StreamKind::Arp { settings, latch: self.flag(n, obj, "latch"), sync }
            }
            "Chord" => {
                let tones: Vec<i8> = match (obj.get("chord"), obj.get("tones")) {
                    (Some(v), None) => match v.text().and_then(theory::chord_kind) {
                        Some(kind) => kind.iter().map(|t| *t as i8).collect(),
                        None => {
                            self.err(n, Some("chord"), format!("`chord` is a chord kind: @{}", theory::CHORD_KINDS.join(" @")));
                            vec![0, 4, 7]
                        }
                    },
                    (None, Some(V::Arr(items))) => items.iter().filter_map(V::num).map(|t| t.clamp(-48.0, 48.0) as i8).collect(),
                    (None, None) => {
                        self.err(n, None, "a Chord names its `chord: @minor` or its `tones: [0 3 7]` (semitones from each key)");
                        vec![0]
                    }
                    _ => {
                        self.err(n, Some("tones"), "a Chord takes `chord: @minor` or `tones: [0 3 7]`, not both");
                        vec![0]
                    }
                };
                if tones.iter().filter(|t| **t != 0).count() > makepad_score_player::stream::MAX_TONES {
                    self.err(n, None, format!("a Chord adds at most {} tones to a key", makepad_score_player::stream::MAX_TONES));
                }
                StreamKind::Chord { tones }
            }
            "Transpose" => StreamKind::Transpose { semis: self.num(n, obj, "semis", 0.0, -48.0, 48.0) as i8 },
            _ => {
                let curve = match choice(self, "curve", &VelCurve::NAMES, 0) {
                    1 => VelCurve::Soft,
                    2 => VelCurve::Hard,
                    3 => VelCurve::Fixed(self.num(n, obj, "level", 0.8, 0.0, 1.0) as f32),
                    _ => VelCurve::Linear,
                };
                StreamKind::Velocity { curve, min: self.num(n, obj, "min", 0.0, 0.0, 1.0) as f32, max: self.num(n, obj, "max", 1.0, 0.0, 1.0) as f32 }
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

    fn read_shader<'b>(&mut self, node: &str, obj: &Obj, kind: shader::Kind, base: shader::AudioBindings<'b>) -> Option<ShaderFields<'b>> {
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
        let values = match obj.get("oversample") {
            None => &[][..],
            Some(v @ V::Num(_)) => std::slice::from_ref(v),
            Some(V::Arr(v)) if matches!(v.len(), 2 | 3) => v.as_slice(),
            _ => {
                self.err(node, Some("oversample"), "use a factor, [factor @mode] or [factor @up @down]");
                return None;
            }
        };
        let oversample: Vec<_> = values.iter().map(|v| match v {
            V::Num(n) => Literal::Num(*n),
            V::Id(mode) | V::Str(mode) => Literal::Id(mode.clone()),
            _ => Literal::Null,
        }).collect();
        let bindings = match makepad_audio_engine::schedule::with_shader_oversample(base, &oversample, kind) {
            Ok(bindings) => bindings,
            Err(why) => { self.err(node, Some("oversample"), why); return None; }
        };
        let decls = base.declarations(&code);
        let mut fields = ShaderFields {
            def: ShaderDef { lib, code, oversample, seed: 0, samples: Vec::new(), overrides: Vec::new(), wires: Vec::new() },
            bindings, ports: Vec::new(), param_wires: Vec::new(), numeric_errors: Vec::new(),
            bad_seed: false, invalid_inventory: false, diags: Vec::new(),
        };
        if decls.has_error() {
            let error = decls.error();
            let line = fields.def.code[..error.start.min(fields.def.code.len())].matches('\n').count() + 1;
            self.err(node, Some(field), format!("shader line {line}: {}", error.message));
            fields.invalid_inventory = true;
            return Some(fields);
        }
        fields.ports = (0..decls.port_count()).map(|i| {
            let port = decls.port(i);
            (port.name.to_owned(), port.output)
        }).collect();
        let seed = match obj.get("seed") {
            None => 0,
            Some(V::Num(n)) if n.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(n) => *n as u32,
            _ => {
                self.err(node, Some("seed"), "a shader seed is an integer from 0 to 4294967295");
                fields.bad_seed = true;
                0
            }
        };
        fields.def.seed = seed;
        if fields.bad_seed { return Some(fields); }
        let first_diag = self.diags.len();
        for (name, value) in &obj.fields {
            if ["lib", "code", "mono", "at", "in", "seed", "oversample"].contains(&name.as_str())
                || kind == shader::Kind::Instrument && ["legato", "glide", "steal"].contains(&name.as_str()) {
                continue;
            }
            if (0..decls.sample_count()).any(|i| decls.sample(i) == name) {
                if let Some(asset) = self.asset(node, name, Some(value)) {
                    fields.def.samples.push((name.clone(), asset));
                }
                continue;
            }
            if let Some(port) = (0..decls.port_count()).map(|i| decls.port(i)).find(|p| p.name == name) {
                if port.output {
                    self.err(node, Some(name), format!("`{name}` is an output; this field needs an input port"));
                } else if kind == shader::Kind::Instrument {
                    fields.def.wires.push((name.clone(), self.audio_sources(node, name, Some(value))));
                }
                continue;
            }
            if matches!(value, V::Obj(_) | V::Arr(_)) {
                if (0..decls.param_count()).position(|i| decls.param(i) == name).is_none() {
                    self.err(node, Some(name), format!("unknown input port `{name}`"));
                } else if kind == shader::Kind::Instrument {
                    fields.def.wires.push((name.clone(), self.audio_sources(node, name, Some(value))));
                } else {
                    fields.param_wires.push(name.clone());
                }
                continue;
            }
            let Some(index) = (0..decls.param_count()).position(|i| decls.param(i) == name) else {
                let names: Vec<&str> = (0..decls.param_count()).map(|i| decls.param(i)).collect();
                self.err(node, Some(name), format!("the shader has no param `{name}` (it has {})", names.join(", ")));
                continue;
            };
            let authored = if let Some(number) = value.num() {
                Override::Num(number)
            } else if decls.choice_count(index) > 0 {
                let choices = || (0..decls.choice_count(index)).map(|i| decls.choice(index, i));
                let Some(word) = value.text().filter(|word| choices().any(|choice| choice == *word)) else {
                    let names = choices().map(shader_choice).collect::<Vec<_>>();
                    self.err(node, Some(name), format!("`{name}` is one of {}", names.join(" ")));
                    continue;
                };
                Override::Choice(word.to_owned())
            } else {
                fields.numeric_errors.push((self.diags.len() - first_diag, name.clone()));
                self.err(node, Some(name), format!("`{name}` is a number"));
                continue;
            };
            fields.def.overrides.push((name.clone(), authored));
        }
        Some(fields)
    }

    fn finish_shader<'b>(&mut self, node: &str, kind: shader::Kind, read: (Option<ShaderFields<'b>>, Vec<Diag>),
        resolve: bool, checked: &mut Vec<(String, CheckedShader)>) -> Option<ShaderFields<'b>> {
        let (fields, diags) = read;
        let Some(mut fields) = fields else { self.diags.extend(diags); return None };
        fields.diags = diags;
        if resolve {
            let program = self.resolve_shader(node, kind, &mut fields)?;
            checked.push((node.to_owned(), program));
        } else {
            self.diags.append(&mut fields.diags);
        }
        Some(fields)
    }

    fn resolve_shader(&mut self, node: &str, kind: shader::Kind, fields: &mut ShaderFields<'_>) -> Option<CheckedShader> {
        let def = &fields.def;
        let code = def.code.as_str();
        let field = if def.lib.is_some() { "lib" } else { "code" };
        let bindings = fields.bindings;
        let entry = self.shader_index;
        self.shader_index += 1;
        let compiled = if let Some(set) = self.reuse {
            let Some((name, previous, checked)) = set.entries.get(entry) else {
                self.shader_mismatch(node, "internal error: missing compiled shader definition");
                return None;
            };
            if name != node || previous.code != def.code || previous.lib != def.lib || previous.oversample != def.oversample || checked.shader.kind() != kind {
                self.shader_mismatch(node, "internal error: shader definition differs from its compiled revision");
                return None;
            }
            checked.shader.clone()
        } else {
            if self.cancelled() { return None; }
            match compile_shader(code, &bindings) {
                Ok(compiled) => compiled,
                Err(errors) => {
                    // Attribute errors introduced by the node to its field, even when a
                    // different error (such as missing region latency) exists at base rate.
                    if self.cancelled() { return None; }
                    let other_errors = (!def.oversample.is_empty()).then(|| {
                        let mut reference = shader::AudioBindings { oversample: Default::default(), ..bindings };
                        if bindings.oversample.factor > 1 { reference.oversample.factor = 1; }
                        compile_shader(code, &reference).err().unwrap_or_default()
                    });
                    if self.cancelled() { return None; }
                    for e in errors.iter().take(3) {
                        let introduced = other_errors.as_ref().is_some_and(|other| !other.iter().any(|d| d.start == e.start && d.message == e.message));
                        let field = if introduced { "oversample" } else { field };
                        let line = code[..e.start.min(code.len())].matches('\n').count() + 1;
                        self.err(node, Some(field), format!("shader line {line}: {}", e.message));
                    }
                    return None;
                }
            }
        };
        if compiled.kind() != kind {
            let want = match kind {
                shader::Kind::Instrument => "an instrument (with voice()); use ShaderFx for effects or ShaderControl for controls",
                shader::Kind::Effect => "an effect (with effect()); use Shader for instruments or ShaderControl for controls",
                shader::Kind::Control => "a control (with block() and named outputs); use ShaderFx for effects or Shader for instruments",
            };
            self.err(node, Some(field), format!("this shader is not {want}"));
            return None;
        }
        for warning in compiled.warnings() {
            let (line, col) = self.at(node, Some(field));
            let shader_line = code.as_bytes()[..warning.start.min(code.len())].iter().filter(|&&b| b == b'\n').count() + 1;
            let diag = Diag { line, col, node: Some(node.to_string()), message: format!("shader line {shader_line}: {}", warning.message) };
            self.warnings.push(ShaderWarning { diag, start: warning.start, end: warning.end });
        }
        for i in 0..compiled.port_count() {
            let port = compiled.port_view(i);
            if kind != shader::Kind::Instrument && makepad_audio_engine::nodes::reserved_shader_port(port.name) {
                self.err(node, Some(field), format!("rename port `{}`: a ShaderFx already has that field", port.name));
            }
        }
        for (at, name) in &fields.numeric_errors {
            let Some(index) = compiled.param_index(name) else {
                self.err(node, Some(name), format!("internal error: compiled shader is missing declared param `{name}`"));
                return None;
            };
            let info = compiled.param_view(index);
            let unit = compiled.param_meta(index).unit;
            fields.diags[*at].message = format!("`{name}` is a number from {} to {}", unit.show(info.min), unit.show(info.max));
        }
        self.diags.append(&mut fields.diags);
        if fields.bad_seed || fields.invalid_inventory { return None; }
        let mut params = Vec::with_capacity(def.overrides.len());
        for (name, value) in &def.overrides {
            let Some(index) = compiled.param_index(name) else {
                self.err(node, Some(name), format!("internal error: compiled shader is missing declared param `{name}`"));
                return None;
            };
            let value = match value {
                Override::Num(number) => {
                    let info = compiled.param_view(index);
                    let number = if number.is_finite() { number.clamp(info.min.into(), info.max.into()) } else { *number };
                    compiled.param_value(index, number as f32)
                }
                Override::Choice(word) => {
                    let Some(choice) = compiled.param_meta(index).choices.iter().position(|&choice| choice == word) else {
                        self.err(node, Some(name), format!("internal error: compiled param `{name}` is missing declared choice `{word}`"));
                        return None;
                    };
                    choice as f32
                }
            };
            params.push((name.clone(), value));
        }
        if let Some(set) = self.reuse {
            let (_, old, checked) = &set.entries[entry];
            if old.wires != def.wires || old.samples != def.samples || old.seed != def.seed || checked.params != params {
                self.shader_mismatch(node, "internal error: shader bindings differ from their compiled revision");
                return None;
            }
        }
        Some(CheckedShader { shader: compiled, params })
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
pub const LOOP_FIELDS: [&str; 25] = ["play", "beats", "amp", "pan", "mute", "solo", "sync", "hits", "note", "step", "code", "when", "follow", "dur", "quant", "notes", "sends", "swing", "humanize", "in", "mute_voices", "solo_voices", "at", "label", "color"];
pub const MIDI_IN_FIELDS: [&str; 7] = ["device", "channel", "bend", "mpe", "master", "latency", "at"];
pub const ARP_FIELDS: [&str; 12] = ["in", "pattern", "rate", "octaves", "gate", "swing", "repeats", "chance", "interval", "latch", "sync", "at"];
pub const CHORD_FIELDS: [&str; 4] = ["in", "chord", "tones", "at"];
pub const TRANSPOSE_FIELDS: [&str; 3] = ["in", "semis", "at"];
pub const VELOCITY_FIELDS: [&str; 6] = ["in", "curve", "level", "min", "max", "at"];
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

pub fn read_document_controlled(source: &str, control: &PrepareControl) -> Result<EvaluatedDocument, LoadError> {
    vm::drive(read_document(source, control))
}

pub async fn read_document(source: &str, control: &PrepareControl) -> Result<EvaluatedDocument, LoadError> {
    read_parts(source, control, None, &mut false, false, vm::EVAL_SLICE).await.map(|(document, _, _)| document)
}

type ReadParts = (EvaluatedDocument, Vec<(String, CheckedShader)>, Vec<ShaderWarning>);

fn checked_document((EvaluatedDocument { doc: authored, vm }, checked, warnings): ReadParts) -> Loaded {
    Loaded { doc: ScoreDoc { authored, checked: checked.into(), warnings: warnings.into() }, vm }
}

async fn read_parts(source: &str, control: &PrepareControl, reuse: Option<&CompiledShaders>, changed: &mut bool, resolve: bool, slice: Duration) -> Result<ReadParts, LoadError> {
    let evaluated = vm::evaluate_with_slice(source, control, slice).await?;
    read_evaluated(source, evaluated, control, reuse, changed, resolve)
}

fn read_evaluated(source: &str, evaluated: Evaluated, control: &PrepareControl, reuse: Option<&CompiledShaders>, changed: &mut bool, resolve: bool) -> Result<ReadParts, LoadError> {
    let result = read(source, evaluated, control, reuse, changed, resolve);
    if control.is_cancelled() { return Err(LoadError::Cancelled); }
    result.map_err(LoadError::Problems)
}

fn read(source: &str, evaluated: Evaluated, control: &PrepareControl, reuse: Option<&CompiledShaders>, changed_shader: &mut bool, resolve: bool) -> Result<ReadParts, Vec<Diag>> {
    if resolve && reuse.is_none() { COMPILE.check(); }
    let outline = source::outline(source);
    let mut r = Reader { src: source, outline: &outline, diags: Vec::new(), warnings: Vec::new(), control, reuse, changed_shader, shader_index: 0 };
    let root = score_root(&evaluated)?;
    // The nodes: the root's members, named by their `let`.
    // `Score{clock kick}` names each member; its value is the node.
    let mut members = Vec::new();
    for (field, value) in root.fields.iter().map(|(k, v)| (Some(k.as_str()), v)).chain(root.items.iter().map(|v| (None, v))) {
        let V::Obj(obj) = value else { continue };
        if obj.proto.as_deref().is_none_or(|p| !vm::NODE_TYPES.contains(&p) || p == "Score") {
            continue;
        }
        // A module can also be a named member directly inside Score{…}.
        let module_name = field.filter(|_| obj.proto.as_deref() == Some("ShaderModule"));
        match r.name_of(value).or_else(|| module_name.map(str::to_owned)) {
            Some(name) => members.push((name, obj.clone())),
            None => r.err("", field, "Score{…} lists nodes written `let name = Type{…}`"),
        }
    }
    // Collect modules first so a shader can import one declared later.
    // Only playable and control nodes belong in the graph's ordering.
    let mut shader_modules = std::collections::BTreeMap::new();
    members.retain(|(name, obj)| {
        if obj.proto.as_deref() != Some("ShaderModule") { return true; }
        for (field, _) in &obj.fields {
            if field != "code" {
                r.err(name, Some(field), format!("unknown ShaderModule field `{field}`"));
            }
        }
        match obj.get("code") {
            Some(V::Str(code)) => {
                if shader_modules.insert(name.clone(), code.clone()).is_some() {
                    r.err(name, None, format!("two shader modules are named `{name}`"));
                }
            }
            _ => r.err(name, Some("code"), "a ShaderModule needs a code string"),
        }
        false
    });
    let mut doc = AuthoredDoc {
        source: source.to_string(),
        shader_modules,
        label: root.get("label").and_then(V::text).unwrap_or("").to_string(),
        clock: ClockDef::default(),
        insts: Vec::new(),
        loops: Vec::new(),
        fx: Vec::new(),
        streams: Vec::new(),
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
    if reuse.is_some_and(|set| set.modules != doc.shader_modules) {
        r.shader_mismatch("", "internal error: shader modules differ from their compiled revision");
    }
    let mut clocks = 0;
    let modules: Vec<_> = doc.shader_modules.iter().map(|(path, source)| makepad_score_player::shader::Module { path, source }).collect();
    // Classify every shader against its declarations before resolving any program.
    // Keep each node's diagnostics with it until its turn in source order.
    let mut shaders = Vec::new();
    let base = shader::AudioBindings {
        modules: &modules,
        control: Some(control.word()),
        executions: Some(control.execution_counter()),
        ..Default::default()
    };
    for (name, obj) in &members {
        let kind = match obj.proto.as_deref() {
            Some("Shader") => shader::Kind::Instrument,
            Some("ShaderFx") => shader::Kind::Effect,
            Some("ShaderControl") => shader::Kind::Control,
            _ => continue,
        };
        if r.cancelled() { return Err(Vec::new()); }
        let before = r.diags.len();
        let fields = r.read_shader(name, obj, kind, base);
        shaders.push((fields, r.diags.split_off(before)));
    }
    let mut shaders = shaders.into_iter();
    let mut checked = Vec::new();
    for (name, obj) in &members {
        if r.cancelled() { return Err(Vec::new()); }
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
            "MidiIn" => MIDI_IN_FIELDS.to_vec(),
            "Arp" => ARP_FIELDS.to_vec(),
            "Chord" => CHORD_FIELDS.to_vec(),
            "Transpose" => TRANSPOSE_FIELDS.to_vec(),
            "Velocity" => VELOCITY_FIELDS.to_vec(),
            other => match FxKind::parse(other) {
                Some(kind) => kind.params().iter().map(|(p, ..)| *p).chain(["in", "key", "at", "b1", "b2", "b3", "b4"]).collect(),
                None => Vec::new(),
            },
        };
        if !allowed.is_empty() {
            for (field, value) in &obj.fields {
                if crate::instruments::INSTRUMENT_TYPES.contains(&ty.as_str()) && ty != "Shader" {
                    let entry = |v: &V| matches!(v, V::Obj(o) if o.proto.as_deref().is_some_and(|ty| ty == "Port" || audio_source_type(ty)));
                    if entry(value) || matches!(value, V::Arr(items) if items.iter().any(entry)) {
                        r.err(n, Some(field), format!("wiring a built-in instrument's `{field}` is not supported yet; wire the port or param on a Shader instrument instead"));
                        continue;
                    }
                }
                if !allowed.contains(&field.as_str()) {
                    if matches!(ty.as_str(), "Loop" | "Stinger") && matches!(value, V::Obj(_) | V::Arr(_)) {
                        r.err(n, Some(field), format!("wire `{field}` on the Shader definition played by this loop"));
                        continue;
                    }
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
                let input = r.refs(n, "in", obj.get("in")).into_iter().next();
                if input.is_some() && (code.is_some() || hits.is_some() || obj.get("notes").is_some()) {
                    r.err(n, Some("in"), "a loop plays its stream (`in:`) or its own notes (`hits`, `code`, `notes`), not both: make a second loop");
                }
                if code.is_none() && hits.is_none() && obj.get("notes").is_none() && !plays_input && input.is_none() {
                    r.err(n, None, "a loop plays `hits: \"x..x\"`, runs `code: fn(i) { … }`, plays `notes: [[beat note dur vel] …]` or a stream (`in: keys`, a MidiIn or an Arp)");
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
                    input,
                });
            }
            "MidiIn" | "Arp" | "Chord" | "Transpose" | "Velocity" => {
                let input = if ty == "MidiIn" { None } else {
                    let input = r.refs(n, "in", obj.get("in")).into_iter().next();
                    if input.is_none() {
                        r.err(n, Some("in"), format!("a {ty} takes a stream: in: keys (a MidiIn, or another stream node)"));
                    }
                    input
                };
                let kind = r.stream(n, &ty, obj);
                doc.streams.push(StreamNode { name: name.clone(), kind, input });
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
                    Some(V::Id(e)) | Some(V::Str(e)) if e == "neural" => {
                        // (a build without it says how to get it: nothing sings in its place)
                        #[cfg(all(not(feature = "neural"), not(target_arch = "wasm32")))]
                        r.err(n, Some("engine"), makepad_score_player::NEURAL_NEEDS_LOCALAI);
                        true
                    }
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
                if let Some(fields) = r.finish_shader(n, shader::Kind::Instrument, shaders.next().unwrap(), resolve, &mut checked) {
                    let shader = fields.def;
                    let mono = obj.get("mono").map(|_| r.flag(n, obj, "mono"));
                    let legato = match obj.get("legato") {
                        None | Some(V::Nil) => true,
                        Some(v) if v.num() == Some(0.0) => false,
                        Some(v) if v.num() == Some(1.0) => true,
                        _ => {
                            r.err(n, Some("legato"), "`legato` takes 0 or 1");
                            true
                        }
                    };
                    let glide = r.num(n, obj, "glide", 0.0, 0.0, f32::MAX as f64) as f32;
                    let steal = match obj.get("steal") {
                        None | Some(V::Nil) => Steal::Oldest,
                        Some(v) => v.text().and_then(Steal::from_name).unwrap_or_else(|| {
                            r.err(n, Some("steal"), "`steal` takes @oldest or @quietest");
                            Steal::Oldest
                        }),
                    };
                    doc.insts.push(InstDef { name: name.clone(), kind: InstKind::Shader { shader, mono, legato, glide, steal } });
                }
            }
            "ShaderFx" | "ShaderControl" => {
                use makepad_score_player::shader::Kind;
                let kind = if ty == "ShaderControl" { Kind::Control } else { Kind::Effect };
                if kind == Kind::Control && obj.get("in").is_some() {
                    r.err(n, Some("in"), "a ShaderControl has no main audio input for `in`; wire a param instead");
                }
                if let Some(fields) = r.finish_shader(n, kind, shaders.next().unwrap(), resolve, &mut checked) {
                    let inputs = r.audio_sources(n, "in", obj.get("in"));
                    let mut sides = Vec::new();
                    for (name, output) in &fields.ports {
                        if !output && obj.get(name).is_some() {
                            sides.push((name.clone(), r.audio_sources(n, name, obj.get(name))));
                        }
                    }
                    for field in fields.param_wires {
                        sides.push((field.clone(), r.audio_sources(n, &field, obj.get(&field))));
                    }
                    let shader = fields.def;
                    doc.fx.push(FxDef { name: name.clone(), kind: FxKind::Bus, params: FxKind::Bus.defaults(), inputs, sides, shader: Some(shader), follow: Vec::new() });
                }
            }
            other => match FxKind::parse(other) {
                Some(kind) => {
                    let mut params = kind.defaults();
                    let mut follow = Vec::new();
                    let mut sides = Vec::new();
                    for (i, (param, ..)) in kind.params().iter().enumerate() {
                        match obj.get(param) {
                            Some(v @ V::Obj(f)) if f.proto.is_some() || r.name_of(v).is_some_and(|id| id != n) => {
                                sides.push(((*param).to_owned(), r.audio_sources(n, param, Some(v))));
                            }
                            Some(v @ V::Arr(_)) => {
                                sides.push(((*param).to_owned(), r.audio_sources(n, param, Some(v))));
                            }
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
                    let inputs = r.audio_sources(n, "in", obj.get("in"));
                    let mut key = r.audio_sources(n, "key", obj.get("key"));
                    // Native keys have always taken the first source in a list.
                    key.nodes.truncate(1);
                    key.named.retain(|&i, _| i == 0);
                    if kind == FxKind::Duck && key.is_empty() {
                        r.err(n, None, "a Duck names what pushes it down: key: kick");
                    }
                    if !key.is_empty() { sides.push(("key".into(), key)); }
                    doc.fx.push(FxDef { name: name.clone(), kind, params, inputs, sides, shader: None, follow });
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
    let program = |node: &str| checked.iter().find(|(name, _)| name == node).map(|(_, s)| s);
    let is_control = |node: &str| program(node).is_some_and(|s| s.shader.kind() == shader::Kind::Control);
    let fx_names: Vec<&str> = doc.fx.iter().map(|f| f.name.as_str()).collect();
    for l in &doc.loops {
        match &l.play {
            Some(play) if !is(play, &crate::instruments::INSTRUMENT_TYPES) => r.err(&l.name, Some("play"), format!("`{play}` is not an instrument in Score{{…}}")),
            None if l.hits.as_ref().is_none_or(|h| h.drum.is_none()) && l.code.is_some() => r.err(&l.name, Some("play"), "a loop names the instrument it plays: play: synth"),
            _ => {}
        }
        if let Some(Err(why)) = doc.resolve_stream(l) {
            r.err(&l.name, Some("in"), why);
        }
        if l.input.is_some() && l.play.is_none() {
            r.err(&l.name, Some("play"), "a loop names the instrument its stream plays: play: synth");
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
            if doc.fx.iter().any(|f| f.name == *target && is_control(&f.name)) {
                r.err(&l.name, Some("sends"), format!("control node `{target}` has no main audio input; wire a param instead"));
            } else if !fx_names.contains(&target.as_str()) {
                r.err(&l.name, Some("sends"), format!("`{target}` is not an FX in Score{{…}}{}", crate::assist::hint(target, fx_names.iter().copied())));
            }
        }
    }
    let graph_names: Vec<_> = fx_names.iter().copied().chain(doc.loops.iter().map(|l| l.name.as_str()))
        .chain(doc.insts.iter().map(|i| i.name.as_str())).collect();
    let index: std::collections::HashMap<_, _> = graph_names.iter().copied().enumerate().map(|(i, n)| (n, i)).collect();
    let mut dependencies = vec![Vec::new(); graph_names.len()];
    let mut forward = Vec::new();
    let mut validate = |node: &str, field: &str, sources: &AudioSources| {
        let i = index[node];
        for (input, port) in sources.ports() {
            let playing = doc.loops.iter().find(|l| l.name == input);
            let fx_at = fx_names.iter().position(|&n| n == input);
            if let Some(&at) = index.get(input) { dependencies[i].push(at); }
            if doc.inst(input).is_some() {
                r.err(node, Some(field), format!("instrument definition `{input}` has no rendered output; read the loop that plays it with Port{{node: loop_name name: @{port}}}"));
            } else if playing.is_none() && fx_at.is_none() {
                r.err(node, Some(field), format!("`{input}` is not a loop or an FX in Score{{…}}"));
            } else if port == "audio" && fx_at.is_some_and(|at| is_control(&doc.fx[at].name)) {
                r.err(node, Some(field), format!("wire `{field}` to a named output of control node `{input}`: use Port{{node: {input} name: @out}}"));
            } else if resolve && port != "audio" {
                let shader = if let Some(l) = playing { l.play.as_deref().and_then(program) } else { program(input) };
                if playing.is_some() && shader.is_none() {
                    r.err(node, Some(field), format!("`{input}.{port}` plays a built-in instrument; read its main audio or play a Shader instrument for named outputs"));
                } else if !shader.is_some_and(|s| (0..s.shader.port_count()).map(|p| s.shader.port_view(p)).any(|p| p.output && p.name == port)) {
                    r.err(node, Some(field), format!("unknown output port `{input}.{port}`"));
                }
            }
            if i < fx_names.len() && fx_at.is_some_and(|at| at >= i) {
                forward.push((node.to_owned(), field.to_owned(), input.to_owned()));
            }
        }
    };
    for fx in &doc.fx {
        validate(&fx.name, "in", &fx.inputs);
        for (field, inputs) in &fx.sides { validate(&fx.name, field, inputs); }
    }
    for inst in &doc.insts {
        if let InstKind::Shader { shader, .. } = &inst.kind {
            for (field, inputs) in &shader.wires { validate(&inst.name, field, inputs); }
        }
    }
    for l in &doc.loops {
        if let Some(i) = l.play.as_deref().and_then(|n| index.get(n)) { dependencies[index[l.name.as_str()]].push(*i); }
        for (fx, _) in &l.sends {
            if let Some(i) = index.get(fx.as_str()) { dependencies[*i].push(index[l.name.as_str()]); }
        }
    }
    if let Some(cycle) = makepad_audio_engine::schedule::wire_cycle(&dependencies) {
        let names = cycle.iter().map(|&i| graph_names[i]).collect::<Vec<_>>().join(" -> ");
        r.err(graph_names[cycle[0]], None, format!("audio wires make a cycle: {names}"));
    } else {
        for (node, field, input) in forward {
            r.err(&node, Some(&field), format!("`{input}` comes after `{node}` in Score{{…}}: list sources first"));
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
    if reuse.is_some_and(|set| set.entries.len() != r.shader_index) {
        r.shader_mismatch("", "internal error: shader count differs from its compiled revision");
    }
    if !r.diags.is_empty() {
        let mut diags = r.diags;
        diags.sort_by_key(|d| (d.line, d.col));
        diags.dedup();
        return Err(diags);
    }
    let warnings = r.warnings;
    doc.outline = outline;
    Ok((EvaluatedDocument { doc: Arc::new(doc), vm: evaluated }, checked, warnings))
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

#[cfg(test)]
pub(crate) fn test_load(source: &str) -> Result<Loaded, Vec<Diag>> {
    match load_controlled(source, &crate::PrepareControl::default()) {
        Ok(loaded) => Ok(loaded),
        Err(LoadError::Problems(diags)) => Err(diags),
        Err(error) => unreachable!("a fresh unbudgeted word cannot stop the load: {error}"),
    }
}

#[cfg(test)]
impl ScoreDoc {
    pub(crate) fn test_load(source: &str) -> Result<ScoreDoc, Vec<Diag>> {
        match ScoreDoc::load_controlled(source, &crate::PrepareControl::default()) {
            Ok(doc) => Ok(doc),
            Err(LoadError::Problems(diags)) => Err(diags),
            Err(error) => unreachable!("a fresh unbudgeted word cannot stop the load: {error}"),
        }
    }
}
