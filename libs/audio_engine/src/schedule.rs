//! A prepared audio revision.
//!
//! `compile` runs on the prepare worker and builds everything the audio
//! thread will touch: the ops in topological order, one stereo span per
//! audio port, every DSP state (loops their voice pools, FX their delay
//! lines and rooms),
//! the control bank and the tables that carry state over from the base
//! generation. The audio thread only reads and steps it.
//!
//! Continuity across an edit is state transfer, stated exactly: a node with
//! the same qualified id and type as in the base generation, at the same
//! device rate, keeps its DSP state (sounding voices, filter memory, the
//! sequencer's position); its authored settings are then the new revision's.
//! Nothing renders twice and nothing is cloned. A node that cannot carry its
//! state starts fresh, and if any sounding node is dropped or rewired the
//! swap is wrapped in a short output dip instead of a click.

use crate::nodes::{AUDIO, CLOCK, LEVEL};
use crate::CHUNK;
use makepad_flow::live::plan::{qualified, EndpointKind, LiveNode, OwnerSlice, PlanError};
use makepad_flow::Literal;
use makepad_audio_dsp::svf::Svf;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

/// Samples of one stereo span: left then right.
pub const SPAN: usize = CHUNK * 2;
/// What prepared schedules hold, shared by prepare workers: an account,
/// not a limit (memory is limited by the machine; a schedule that cannot
/// get it fails where it allocates). A permit stays with its allocation
/// through staging, playback and retirement; only the host/worker drops
/// it, so the account shows whether replaced revisions are released.
#[derive(Default)]
pub struct AudioBudget {
    bytes: AtomicUsize,
    /// Deck PCM, leased through `TrackLease`; its own limit.
    pub tracks: AtomicUsize,
}

pub struct BudgetPermit {
    budget: Arc<AudioBudget>,
    bytes: usize,
}

impl AudioBudget {
    fn reserve(self: &Arc<Self>, bytes: usize) -> BudgetPermit {
        self.bytes.fetch_add(bytes, Ordering::AcqRel);
        BudgetPermit { budget: self.clone(), bytes }
    }

    /// Bytes the schedules prepared and not yet dropped hold (an estimate
    /// from above).
    pub fn prepared_bytes(&self) -> usize {
        self.bytes.load(Ordering::Acquire)
    }
}

impl Drop for BudgetPermit {
    fn drop(&mut self) {
        self.budget.bytes.fetch_sub(self.bytes, Ordering::Release);
    }
}

/// The semantic control contract, also checked at the owner boundary.
pub fn control_range(kind: &str, name: &str) -> Option<(f64, f64, bool)> {
    Some(match (kind, name) {
        ("Transport", "bpm") => (40.0, 240.0, false),
        ("Transport", "playing") | ("AudioOutput" | "CueOutput", "enabled") | ("Gain", "mute") => (0.0, 1.0, true),
        ("Filter", "cutoff") => (40.0, 18000.0, false),
        ("Filter", "resonance") => (0.1, 8.0, false),
        ("Gain", "gain") | ("AudioMixer", "master") => (0.0, 2.0, false),
        ("Loop", "amp") => (0.0, 4.0, false),
        ("Loop", "pan") => (-1.0, 1.0, false),
        ("Loop", "mute" | "solo") => (0.0, 1.0, true),
        ("Pads", "amp") => (0.0, 4.0, false),
        ("Pads", "mute") => (0.0, 1.0, true),
        (kind, name) if makepad_score_player::FxKind::parse(kind).is_some_and(|fx| fx.name() == kind && fx.param_index(name).is_some()) => {
            let fx = makepad_score_player::FxKind::parse(kind)?;
            let (_, _, lo, hi) = fx.params()[fx.param_index(name)?];
            (lo as f64, hi as f64, fx.is_toggle(name))
        }
        ("Gain", "pan") => (-1.0, 1.0, false),
        ("StemMixer", "gain_vocals" | "gain_drums" | "gain_bass" | "gain_other") => (0.0, 1.5, false),
        ("StemMixer", "mute_vocals" | "mute_drums" | "mute_bass" | "mute_other" | "solo_vocals" | "solo_drums" | "solo_bass" | "solo_other") => (0.0, 1.0, true),
        _ => return None,
    })
}
/// Effective values of every control endpoint of one generation. The host
/// writes unwired controls; the audio thread writes driven controls before
/// their consumer runs. Both owners and host readback use these atomic slots.
/// Remembered manual values are host-owned and survive a wire taking over.
pub struct ControlBank {
    targets: Box<[AtomicU32]>,
    manual: Box<[AtomicU32]>,
}

impl ControlBank {
    fn new(values: &[f32]) -> Self {
        Self {
            targets: values.iter().map(|value| AtomicU32::new(value.to_bits())).collect(),
            manual: values.iter().map(|value| AtomicU32::new(value.to_bits())).collect(),
        }
    }

    pub fn get(&self, index: u16) -> f32 {
        f32::from_bits(self.targets[index as usize].load(Ordering::Relaxed))
    }

    pub fn manual(&self, index: u16) -> f32 {
        f32::from_bits(self.manual[index as usize].load(Ordering::Relaxed))
    }

    pub fn set_manual(&self, index: u16, value: f32) {
        self.manual[index as usize].store(value.to_bits(), Ordering::Relaxed);
        self.set_effective(index, value);
    }

    /// The effective values' bits, into `into` (as many as the bank holds).
    pub fn snapshot(&self, into: &mut [u32]) {
        for (slot, target) in into.iter_mut().zip(self.targets.iter()) {
            *slot = target.load(Ordering::Relaxed);
        }
    }

    /// Whether any effective value differs from `snapshot`'s.
    pub fn moved_since(&self, snapshot: &[u32]) -> bool {
        self.targets.iter().zip(snapshot).any(|(target, seen)| target.load(Ordering::Relaxed) != *seen)
    }

    pub fn set_effective(&self, index: u16, value: f32) {
        self.targets[index as usize].store(value.to_bits(), Ordering::Relaxed);
    }

    /// Exchange two slots, remembered and effective values alike: a deck
    /// swap moves each deck's transport with its source.
    pub fn exchange(&self, a: u16, b: u16) {
        let (manual_a, manual_b) = (self.manual(a), self.manual(b));
        let (target_a, target_b) = (self.get(a), self.get(b));
        self.manual[a as usize].store(manual_b.to_bits(), Ordering::Relaxed);
        self.manual[b as usize].store(manual_a.to_bits(), Ordering::Relaxed);
        self.set_effective(a, target_b);
        self.set_effective(b, target_a);
    }
}

/// A same-owner wire into a control or toggle: before op `op` runs, the
/// scalar its producer published this pass (`probes[probe]`) becomes the
/// control's effective value in bank slot `slot`. A non-finite sample holds
/// the value the slot had (the remembered manual value until a first good one).
#[derive(Clone)]
pub struct Route {
    pub op: u16,
    pub slot: u16,
    pub probe: u16,
    pub kind: EndpointKind,
}

impl Route {
    #[inline]
    pub fn apply(&self, probes: &[f32], controls: &ControlBank) {
        if let Some(value) = makepad_flow::live::plan::normalize_signal(&self.kind, probes[self.probe as usize] as f64) {
            controls.set_effective(self.slot, value as f32);
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct ClockChunk {
    /// Beat position at the first frame of the pass.
    pub start: f64,
    pub per_frame: f64,
    pub playing: bool,
    pub bpm: f32,
    /// The pass's beats exactly, tempo changes inside it included.
    pub tempo: makepad_score_player::Tempo,
}

/// `X` is an extension's node state (the decks of Stage's audio owner).
pub enum NodeState<X> {
    None,
    /// The beat is counted from an anchor in frames, not summed pass by
    /// pass, so it is the same however a buffer is cut into passes: `anchor`
    /// is the beat where the tempo or play state last changed, `frames` the
    /// frames played since, at `bpm` (NaN before the first pass).
    Transport(Box<TransportState>),
    Filter { left: Svf, right: Svf, cutoff: f32, q: f32 },
    Gain { left: f32, right: f32, peak: f32 },
    Mixer { master: f32 },
    StemMixer { gains: [f32; 4] },
    Output { peak_l: f32, peak_r: f32, gate: f32 },
    Loop(Box<LoopState>),
    Pads(Box<PadsState>),
    Fx(Box<FxNodeState>),
    ShaderFx(Box<ShaderFxState>),
    /// A player of its own ([`crate::nested`]).
    Nested(Box<dyn crate::nested::NestedNode>),
    Ext(X),
}

/// A `ShaderFx` node between passes: the shader's state, its input's
/// copy, the params last set (their list's address).
pub struct ShaderFxState {
    pub fx: makepad_score_player::shader::ShaderFx,
    pub input: Box<[f32]>,
    pub params: usize,
    pub peak: f32,
}

/// Move a node's state from the generation playing (`old`) into the next
/// (`new`): the next generation's freshly made state goes back with the
/// old one. A shader's new program takes over the sounding state (hot
/// swap, see `ShaderInst::swap_from`). No allocation: runs on the audio
/// thread.
pub fn migrate_state<X>(old: &mut NodeState<X>, new: &mut NodeState<X>) {
    std::mem::swap(old, new);
    match (old, new) {
        (NodeState::Loop(fresh), NodeState::Loop(kept)) => {
            // What the new plan decided about MIDI out and audio in holds.
            kept.midi_live = fresh.midi_live;
            std::mem::swap(&mut fresh.input, &mut kept.input);
            // The held settings go on the new revision's instrument afresh.
            std::mem::swap(&mut fresh.live, &mut kept.live);
            kept.live_seen = None;
            if fresh.inst_class != kept.inst_class {
                // Another instrument: the new engine plays from here, with
                // the feed, gains and the events already waiting.
                std::mem::swap(&mut fresh.engine, &mut kept.engine);
                kept.engine.take_pending(&mut fresh.engine);
                kept.inst_class = fresh.inst_class;
            } else {
                kept.engine.take_program(&mut fresh.engine);
            }
        }
        (NodeState::Pads(fresh), NodeState::Pads(kept)) => {
            // The voices sounding and the feed stay; the pads' samples and
            // levels are the new revision's (a voice holds its own sample).
            std::mem::swap(&mut fresh.samples, &mut kept.samples);
            std::mem::swap(&mut fresh.gains, &mut kept.gains);
        }
        // A nested player plays on unless it now plays something else.
        (NodeState::Nested(fresh), NodeState::Nested(kept)) => {
            if fresh.key() != kept.key() {
                std::mem::swap(fresh, kept);
            }
        }
        (NodeState::ShaderFx(fresh), NodeState::ShaderFx(kept)) => {
            if !fresh.fx.shader().ptr_eq(kept.fx.shader()) {
                fresh.fx.swap_from(&mut kept.fx);
                std::mem::swap(&mut fresh.fx, &mut kept.fx);
                kept.params = 0;
            }
        }
        _ => {}
    }
}

/// A transport between passes: where its beat is (anchored at a beat and a
/// fractional frame, so any pass slicing counts the same beats), the tempo
/// its bpm control and playing flag set, and the tempo map a score sends
/// it (changes at beats, ramps in sixteenth-beat steps).
pub struct TransportState {
    pub beat: f64,
    pub anchor: f64,
    pub anchor_frame: f64,
    pub frames: u64,
    /// The bpm control and playing flag last seen.
    pub bpm: f32,
    pub playing: bool,
    /// The tempo playing (the control's, or the map's since its last change).
    pub tempo: f64,
    pub feed: Option<makepad_flow::live::spsc::Consumer<crate::feed::TempoEvent>>,
    /// Changes waiting for their beat, in beat order (capacity reserved).
    pub pending: Vec<crate::feed::TempoEvent>,
    /// A ramp under way: (start beat, from bpm, to bpm, steps, next step).
    pub ramp: Option<(f64, f64, f64, u32, u32)>,
    /// The tempo playing came from the map (not the bpm control): the
    /// control is written back so a host reading it sees the tempo.
    pub mapped: bool,
}

impl LoopState {
    /// The loop's clock is not running it (a stopped transport).
    pub fn playing_clear(&self) -> bool {
        !self.playing
    }
}

/// A `Loop` node between passes: its voices and the events waiting for
/// their chunk, the reading end of its feed, and its smoothed gains.
pub struct LoopState {
    pub engine: makepad_score_player::loop_engine::LoopEngine,
    pub feed: Option<makepad_flow::live::spsc::Consumer<makepad_score_player::Event>>,
    pub gain_l: f32,
    pub gain_r: f32,
    pub peak: f32,
    pub playing: bool,
    /// What kind of instrument its engine was made for.
    pub inst_class: u64,
    /// A MIDI loop of the live score: its MIDI goes out.
    pub midi_live: bool,
    /// An input loop's ring from the audio input.
    pub input: Option<makepad_flow::live::spsc::Consumer<[f32; 2]>>,
    /// Its instrument's settings an editor holds live, and the version of
    /// them last set on the engine (none yet: set on the first pass).
    pub live: Option<Arc<crate::feed::LiveParams>>,
    pub live_seen: Option<u32>,
}

/// Voices a `Pads` node sounds at once; past it the oldest gives way.
pub const PAD_VOICES: usize = 32;
/// Frames a stopped or stolen voice fades over.
pub const PAD_FADE_FRAMES: u32 = 128;

/// One sounding pad sample.
pub struct PadVoice {
    pub sample: Arc<makepad_score_player::Sample>,
    pub pad: u32,
    /// Where it is in the sample (its frames) and how fast it moves.
    pub pos: f64,
    pub step: f64,
    pub gain: f32,
    /// Frames left of a fade out, when it was let go.
    pub fading: Option<u32>,
    /// Order of the hits, for the oldest to give way.
    pub serial: u64,
}

/// A `Pads` node between passes: each pad's sample and level, the voices
/// sounding (room reserved: no allocation on the audio thread) and the
/// reading end of its hits.
pub struct PadsState {
    pub samples: Arc<[Option<Arc<makepad_score_player::Sample>>]>,
    pub gains: Arc<[f32]>,
    pub voices: Vec<PadVoice>,
    pub feed: Option<makepad_flow::live::spsc::Consumer<crate::feed::PadHit>>,
    pub serial: u64,
    pub gain: f32,
    pub peak: f32,
}

/// An FX node between passes.
pub struct FxNodeState {
    pub fx: makepad_score_player::fx::FxState,
    pub peak: f32,
    /// The score's automation: its feed, what waits for its beat, and each
    /// setting's ramp (from, to, start beat, beats) once one began.
    pub feed: Option<makepad_flow::live::spsc::Consumer<crate::feed::AutomationEvent>>,
    pub waiting: Vec<crate::feed::AutomationEvent>,
    pub ramps: [Option<(f32, f32, f64, f64)>; makepad_score_player::fx::MAX_FX_PARAMS],
}

/// `Y` is an extension's op (the decks of Stage's audio owner).
pub enum Op<Y> {
    Transport { state: u16, clock: u16, bpm: u16, playing: u16, probe: u16, mapped: u16 },
    Filter { state: u16, input: Option<u16>, out: u16, cutoff: u16, resonance: u16 },
    Gain { state: u16, input: Option<u16>, out: u16, gain: u16, pan: u16, mute: u16, probe: u16 },
    Mixer { state: u16, inputs: Box<[u16]>, out: u16, master: u16 },
    StemMixer { state: u16, input: Option<u16>, stems: [Option<u16>; 4], available: Option<u16>, out: u16, gains: [u16; 4], mutes: [u16; 4], solos: [u16; 4] },
    /// `cue`: the headphone cue's sink (CueOutput), else the device's.
    Output { state: u16, input: Option<u16>, out: u16, enabled: u16, probes: (u16, u16), cue: bool },
    Loop { state: u16, clock: Option<u16>, out: u16, amp: u16, pan: u16, mute: u16, solo: u16, probe: u16, inst: makepad_score_player::Inst },
    Pads { state: u16, out: u16, amp: u16, mute: u16, probe: u16 },
    Fx { state: u16, kind: makepad_score_player::FxKind, inputs: Box<[u16]>, key: Option<u16>, out: u16, params: [u16; makepad_score_player::fx::MAX_FX_PARAMS], count: u8, probe: u16, gr: u16 },
    ShaderFx { state: u16, inputs: Box<[u16]>, out: u16, params: std::sync::Arc<[(u16, f32)]>, probe: u16 },
    Nested { state: u16, clock: Option<u16>, out: u16, probe: u16 },
    Ext(Y),
}

/// `B`: what an extension publishes from the schedule (its telemetry
/// board, sized when the schedule is prepared); nothing by default.
pub struct Schedule<X, Y, B = ()> {
    pub generation: u64,
    pub incarnation: u64,
    pub format: u64,
    /// The generation `migrate` indexes into; any other base is not touched.
    pub base: Option<u64>,
    pub rate: u32,
    /// Wrap the swap in a short output dip.
    pub dip: bool,
    pub ops: Box<[Op<Y>]>,
    pub states: Box<[NodeState<X>]>,
    pub arena: Box<[f32]>,
    /// The valid portion of every span from the last actual render pass.
    pub last_frames: u16,
    /// Dedicated post-gate spans, multiplied by the session dip after the pass.
    pub sinks: Box<[u16]>,
    pub clocks: Box<[ClockChunk]>,
    pub probes: Box<[f32]>,
    pub controls: Arc<ControlBank>,
    /// Wired controls in op order; see [`Route`].
    pub routes: Box<[Route]>,
    /// `(base state, this state)` of every node that carries its state over.
    pub migrate: Box<[(u16, u16)]>,
    /// Deck states carried over across a device-rate change (their
    /// position is in source frames); at an equal rate they are in `migrate`.
    pub migrate_decks: Box<[(u16, u16)]>,
    /// `(stable target id, state, continuity)` sorted by id: decks for
    /// notes, decks for cues and tracks.
    pub triggers: Box<[(u32, u16, u64)]>,
    /// Deck state indices in observation order.
    pub decks: Box<[u16]>,
    pub board: B,
    /// Whether the schedule rests while it is silent ([`crate::rest`]).
    pub rest: crate::rest::Rest,
    pub _permit: BudgetPermit,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LayoutNode {
    pub id: String,
    pub type_name: String,
    pub state: u16,
    pub clock_source: Option<String>,
    /// A loop's kind of instrument (its state only carries over to the
    /// same kind: an Ironfish loop's synth, a choir, live in its state).
    pub inst_class: u64,
    pub sounding: bool,
}

/// What the host keeps about a generation: enough to address its controls
/// and probes by name and to prepare the next generation against it.
#[derive(Clone, Debug)]
pub struct Layout {
    pub generation: u64,
    pub rate: u32,
    pub nodes: Vec<LayoutNode>,
    /// `node.param` to its bank index and authored value.
    pub controls: HashMap<String, (u16, f64)>,
    /// Driven `node.param` to the `node.port` that drives it. A driven
    /// control refuses writes; its bank slot holds the effective value.
    pub drivers: HashMap<String, String>,
    /// `node.port` to its probe index.
    pub probes: HashMap<String, u16>,
    /// Output stream (or owner-defined sink observation) to its arena span.
    pub audio_probes: HashMap<String, u16>,
    /// Qualified deck id to its stable target id and continuity.
    pub triggers: HashMap<String, (u32, u64)>,
    /// Qualified Transport id to its clock index.
    pub transports: HashMap<String, u16>,
    /// Qualified AudioDeck id to its observation index.
    pub decks: HashMap<String, u16>,
    pub audio_edges: Vec<(String, String, Option<u64>)>,
}

pub struct Compiled<X, Y, B = ()> {
    pub schedule: Box<Schedule<X, Y, B>>,
    pub layout: Arc<Layout>,
}

pub fn number(node: &LiveNode, name: &str) -> Result<f64, PlanError> {
    match node.param(name) {
        Some(Literal::Num(value)) if value.is_finite() => Ok(*value),
        _ => Err(PlanError::at(&node.id, Some(name), "this parameter is a finite number")),
    }
}

fn text<'a>(node: &'a LiveNode, name: &str) -> Result<&'a str, PlanError> {
    match node.param(name) {
        Some(Literal::Str(value) | Literal::Id(value)) => Ok(value),
        _ => Err(PlanError::at(&node.id, Some(name), "this parameter is text")),
    }
}

/// A stable 32-bit id for a trigger target: the same qualified id gives the
/// same id in every generation, so a command finds a deck that moved.
pub fn trigger_id(node: &str) -> u32 {
    node.bytes().fold(0x811c_9dc5u32, |hash, byte| (hash ^ byte as u32).wrapping_mul(0x0100_0193))
}

/// An owner's own node types beyond the engine's (Stage's decks): what
/// they need, how one is prepared, and whether its state survives a change
/// of device rate.
pub trait Ext {
    type State;
    type Op;
    /// `(controls, probes, audio spans)` a node of this type takes, or
    /// `None` when the type is not this extension's.
    fn demand(type_name: &str) -> Option<(usize, usize, usize)>;
    /// Prepare one node; `None` when the type is not this extension's.
    fn compile(cx: &mut ExtCx) -> Result<Option<(Self::State, Self::Op)>, PlanError>;
    /// Whether a node of this type carries its state over a rate change.
    fn keeps_across_rates(type_name: &str) -> bool;
    /// The range of an extension node's control: `(min, max, toggle)`.
    fn control_range(kind: &str, name: &str) -> Option<(f64, f64, bool)>;
}

/// What an extension's node sees while it is prepared.
pub struct ExtCx<'a, 'b> {
    pub node: &'a LiveNode,
    pub state: u16,
    pub rate: u32,
    pub generation: u64,
    pub base: Option<&'a Layout>,
    pub layout: &'a mut Layout,
    pub spans: &'a mut HashMap<(String, String), u16>,
    /// Observed state indices, in order (the schedule's `decks`).
    pub observed: &'a mut Vec<u16>,
    pub sounding: &'a mut bool,
    control: &'a mut (dyn FnMut(&mut Layout, &str) -> Result<u16, PlanError> + 'b),
    probe: &'a mut (dyn FnMut(&mut Layout, &str) -> u16 + 'b),
}

impl ExtCx<'_, '_> {
    /// A control endpoint of the node, with its bank slot.
    pub fn control(&mut self, name: &str) -> Result<u16, PlanError> {
        (self.control)(self.layout, name)
    }

    /// A scalar probe of the node's `port`.
    pub fn probe(&mut self, port: &str) -> u16 {
        (self.probe)(self.layout, port)
    }
}

/// A clock slot driven from outside: no op runs it, the host writes the
/// slot ([`Schedule::drive_clock`]) before each pass. What a player that
/// follows its host's clock (a nested score) has instead of a Transport.
pub const CLOCK_IN: &str = "ClockIn";

impl<X, Y, B> Schedule<X, Y, B> {
    /// Set clock slot `clock` (a [`CLOCK_IN`] node's) for the next pass.
    pub fn drive_clock(&mut self, clock: u16, chunk: ClockChunk) {
        self.clocks[clock as usize] = chunk;
    }
}

pub fn compile<E: Ext, B: Default>(slice: &OwnerSlice, generation: u64, incarnation: u64, format: u64, base: Option<&Layout>, budget: &Arc<AudioBudget>) -> Result<Compiled<E::State, E::Op, B>, PlanError> {
    let rate = format as u32;
    if !(8_000..=384_000).contains(&rate) {
        return Err(PlanError::general(format!("the device rate {rate} Hz is outside what the DSP was built for")));
    }
    // Refuse resource demand before constructing any delay/reverb state.
    let (mut control_count, mut probes, mut audio_spans, mut heavy) = (0usize, 0usize, 0usize, 0usize);
    let mut ids = HashSet::new();
    for node in &slice.nodes {
        if !ids.insert(&node.id) {
            return Err(PlanError::at(&node.id, None, "duplicate audio node id"));
        }
        let (controls, count) = match node.type_name.as_str() {
            "Transport" => (2, 2),
            CLOCK_IN => (0, 0),
            "Filter" => (2, 0), "Gain" => (3, 1), "AudioMixer" => (1, 0), "AudioOutput" | "CueOutput" => (1, 2),
            "StemMixer" => (12, 0),
            "Loop" => {
                heavy += 1;
                (4, 1)
            }
            "Pads" => (2, 1),
            "ShaderFx" => {
                heavy += 1;
                (0, 1)
            }
            kind if makepad_score_player::FxKind::parse(kind).is_some_and(|fx| fx.name() == kind) => {
                heavy += 1;
                (makepad_score_player::FxKind::parse(kind).map_or(0, |fx| fx.params().len()), 2)
            }
            other => match E::demand(other) {
                Some((controls, probes, _)) => (controls, probes),
                None if crate::nested::factory(other).is_some() => {
                    heavy += 1;
                    (0, 1)
                }
                None => return Err(PlanError::at(&node.id, None, "unsupported audio node type")),
            },
        };
        control_count += controls;
        probes += count;
        audio_spans += match node.type_name.as_str() {
            "Filter" | "Gain" | "AudioMixer" | "StemMixer" | "AudioOutput" | "CueOutput" | "Loop" | "Pads" | "ShaderFx" => 1,
            kind if makepad_score_player::FxKind::parse(kind).is_some_and(|fx| fx.name() == kind) => 1,
            other => E::demand(other).map_or(usize::from(crate::nested::factory(other).is_some()), |(_, _, spans)| spans),
        };
        // A mixer's and an effect's audio input take any number of
        // connections; every other input one.
        for input in &node.inputs {
            let fx = node.type_name == "ShaderFx" || makepad_score_player::FxKind::parse(&node.type_name).is_some_and(|fx| fx.name() == node.type_name);
            let many = (node.type_name == "AudioMixer" && input.port == "inputs") || (fx && input.port == "audio");
            if !many && input.sources.len() > 1 {
                return Err(PlanError::at(&node.id, Some(&input.port), "this audio input takes one connection"));
            }
        }
    }
    // Controls, probes and spans are indexed by u16 in the realtime ops.
    let indices = u16::MAX as usize;
    if control_count > indices || probes > indices || audio_spans > indices || slice.nodes.len() > indices {
        return Err(PlanError::general(format!("the audio graph has more than {indices} controls, probes or nodes")));
    }
    // Upper bound includes the exact variable delay/chorus lengths, the
    // fixed reverb, inline DSP state, arenas and conservative metadata room.
    // A loop's voice pool and queue, or an FX's delay lines and rooms, at
    // most: the four-second delay's two lines are the largest.
    let heavy_bytes = 2 * (4 * rate as usize + 8) * 4 + 262_144;
    let bytes = heavy * heavy_bytes + audio_spans * SPAN * 4 + slice.nodes.len() * 4096 + 16_384
        + control_count * (std::mem::size_of::<Route>() + 2 * std::mem::size_of::<AtomicU32>());
    // The machine is the limit: a schedule that cannot get its memory says
    // so here instead of aborting halfway through building its nodes.
    if Vec::<u8>::new().try_reserve_exact(bytes).is_err() {
        return Err(PlanError::general(format!("not enough memory to prepare the audio graph ({} MiB)", bytes >> 20)));
    }
    let permit = budget.reserve(bytes);
    let mut ops = Vec::with_capacity(slice.nodes.len());
    let mut deck_states: Vec<u16> = Vec::new();
    let mut states: Vec<NodeState<E::State>> = Vec::with_capacity(slice.nodes.len());
    let mut bank: Vec<f32> = Vec::new();
    let mut layout = Layout {
        generation, rate, nodes: Vec::new(), controls: HashMap::new(), probes: HashMap::new(), audio_probes: HashMap::new(),
        triggers: HashMap::new(), audio_edges: Vec::new(), drivers: HashMap::new(), transports: HashMap::new(), decks: HashMap::new(),
    };
    // One slot of its kind per output port, in node order: an output is
    // always allocated after every input it could read.
    let mut spans: HashMap<(String, String), u16> = HashMap::new();
    let mut clocks: HashMap<(String, String), u16> = HashMap::new();
    let mut probe_count = 0u16;
    let mut sinks = Vec::new();

    let mut routes: Vec<Route> = Vec::with_capacity(control_count);
    for node in &slice.nodes {
        let op_index = ops.len() as u16;
        let mut control = |layout: &mut Layout, name: &str| -> Result<u16, PlanError> {
            let value = number(node, name)?;
            let (min, max, toggle) = control_range(&node.type_name, name).or_else(|| E::control_range(&node.type_name, name))
                .ok_or_else(|| PlanError::at(&node.id, Some(name), "unknown audio control"))?;
            let (range_min, range_max) = (min, max);
            if !(min..=max).contains(&value) || (toggle && value != 0.0 && value != 1.0) {
                return Err(PlanError::at(&node.id, Some(name), "audio control is outside its range or is not an exact 0/1 toggle"));
            }
            let index = bank.len() as u16;
            bank.push(value as f32);
            layout.controls.insert(format!("{}.{name}", node.id), (index, value));
            if let Some(input) = node.input(name).filter(|input| !input.sources.is_empty()) {
                let source = &input.sources[0];
                let from = qualified(&source.from.node);
                let key = format!("{from}.{}", source.from.port);
                let scalar = slice.nodes.iter().find(|other| other.id == from)
                    .and_then(|other| other.outputs.iter().find(|(port, _)| *port == source.from.port))
                    .is_some_and(|(_, ty)| ty.as_str() == LEVEL);
                let probe = layout.probes.get(&key).copied().filter(|_| scalar)
                    .ok_or_else(|| PlanError::at(&node.id, Some(name), format!("`{key}` publishes no audio scalar this control can follow")))?;
                let kind = match &input.control {
                    Some(kind @ EndpointKind::Toggle { .. }) => kind.clone(),
                    Some(EndpointKind::Control { min, max, step, default }) => EndpointKind::Control {
                        min: min.max(range_min), max: max.min(range_max), step: *step, default: *default,
                    },
                    _ => return Err(PlanError::at(&node.id, Some(name), "this parameter takes no wire")),
                };
                routes.push(Route { op: op_index, slot: index, probe, kind });
                layout.drivers.insert(format!("{}.{name}", node.id), key);
            }
            Ok(index)
        };
        let mut probe = |layout: &mut Layout, port: &str| -> u16 {
            let index = probe_count;
            probe_count += 1;
            layout.probes.insert(format!("{}.{port}", node.id), index);
            index
        };
        let source = |port: &str, key: &str, table: &HashMap<(String, String), u16>| -> Result<Option<u16>, PlanError> {
            let Some(input) = node.input(port) else { return Ok(None) };
            if input.ty.as_str() != key || input.sources.len() > 1 {
                return Err(PlanError::at(&node.id, Some(port), "audio input has the wrong type or shape"));
            }
            let Some(source) = input.sources.first() else { return Ok(None) };
            table
                .get(&(qualified(&source.from.node), source.from.port.clone()))
                .copied()
                .map(Some)
                .ok_or_else(|| PlanError::at(&node.id, Some(port), "its source is not an audio node's port"))
        };
        let state = states.len() as u16;
        let mut clock_source = None;
        let mut inst_class = 0u64;
        let mut sounding = false;
        for input in &node.inputs {
            if input.ty.as_str() == AUDIO || (node.type_name == "StemMixer" && input.port == "available") {
                for source in &input.sources {
                    layout.audio_edges.push((format!("{}.{}", qualified(&source.from.node), source.from.port), format!("{}.{}", node.id, input.port), source.conn));
                }
            }
        }
        match node.type_name.as_str() {
            "Transport" => {
                let clock = clocks.len() as u16;
                clocks.insert((node.id.clone(), "clock".into()), clock);
                layout.transports.insert(node.id.clone(), clock);
                let (bpm, playing) = (control(&mut layout, "bpm")?, control(&mut layout, "playing")?);
                let mapped = probe(&mut layout, "mapped");
                let probe = probe(&mut layout, "clock");
                states.push(NodeState::Transport(Box::new(TransportState {
                    beat: 0.0, anchor: 0.0, anchor_frame: 0.0, frames: 0, bpm: f32::NAN, playing: false, tempo: 120.0,
                    // A transport that carries its state over keeps its feed.
                    feed: (!base.filter(|base| base.rate == rate).is_some_and(|base| base.nodes.iter().any(|old| old.id == node.id && old.type_name == "Transport")))
                        .then(|| crate::feed::tempo_feed(&node.id).fresh()),
                    pending: Vec::with_capacity(crate::feed::TEMPO_EVENTS), ramp: None, mapped: false,
                })));
                ops.push(Op::Transport { state, clock, bpm, playing, probe, mapped });
            }
            CLOCK_IN => {
                let clock = clocks.len() as u16;
                clocks.insert((node.id.clone(), "clock".into()), clock);
                states.push(NodeState::None);
            }
            "Filter" => {
                let input = source("audio", AUDIO, &spans)?;
                let out = spans.len() as u16;
                spans.insert((node.id.clone(), "audio".into()), out);
                let (cutoff, resonance) = (control(&mut layout, "cutoff")?, control(&mut layout, "resonance")?);
                sounding = true;
                states.push(NodeState::Filter { left: Svf::default(), right: Svf::default(), cutoff: bank[cutoff as usize], q: bank[resonance as usize] });
                ops.push(Op::Filter { state, input, out, cutoff, resonance });
            }
            "Gain" => {
                let input = source("audio", AUDIO, &spans)?;
                let out = spans.len() as u16;
                spans.insert((node.id.clone(), "audio".into()), out);
                let (gain, pan) = (control(&mut layout, "gain")?, control(&mut layout, "pan")?);
                let mute = control(&mut layout, "mute")?;
                let probe = probe(&mut layout, "peak");
                states.push(NodeState::Gain { left: 0.0, right: 0.0, peak: 0.0 });
                ops.push(Op::Gain { state, input, out, gain, pan, mute, probe });
            }
            "AudioMixer" => {
                let mut inputs = Vec::new();
                for source in node.input("inputs").map(|input| input.sources.as_slice()).unwrap_or_default() {
                    let span = spans
                        .get(&(qualified(&source.from.node), source.from.port.clone()))
                        .ok_or_else(|| PlanError::at(&node.id, Some("inputs"), "an entry is not an audio node's port"))?;
                    inputs.push(*span);
                }
                let out = spans.len() as u16;
                spans.insert((node.id.clone(), "audio".into()), out);
                let master = control(&mut layout, "master")?;
                states.push(NodeState::Mixer { master: bank[master as usize] });
                ops.push(Op::Mixer { state, inputs: inputs.into_boxed_slice(), out, master });
            }
            "StemMixer" => {
                let input = source("audio", AUDIO, &spans)?;
                let names = ["vocals", "drums", "bass", "other"];
                let mut stems = [None; 4];
                let (mut gains, mut mutes, mut solos) = ([0; 4], [0; 4], [0; 4]);
                for (lane, name) in names.iter().enumerate() {
                    stems[lane] = source(name, AUDIO, &spans)?;
                    gains[lane] = control(&mut layout, &format!("gain_{name}"))?;
                    mutes[lane] = control(&mut layout, &format!("mute_{name}"))?;
                    solos[lane] = control(&mut layout, &format!("solo_{name}"))?;
                }
                let available = match node.input("available") {
                    Some(input) => {
                        if input.ty.as_str() != LEVEL || input.sources.len() > 1 {
                            return Err(PlanError::at(&node.id, Some("available"), "stem availability takes one level source"));
                        }
                        input.sources.first().map(|source| {
                            let key = format!("{}.{}", qualified(&source.from.node), source.from.port);
                            layout.probes.get(&key).copied().ok_or_else(|| PlanError::at(&node.id, Some("available"), "stem availability source has no scalar probe"))
                        }).transpose()?
                    }
                    None => None,
                };
                let out = spans.len() as u16;
                spans.insert((node.id.clone(), "audio".into()), out);
                states.push(NodeState::StemMixer { gains: [0.0; 4] });
                ops.push(Op::StemMixer { state, input, stems, available, out, gains, mutes, solos });
            }
            "Loop" => {
                let clock = source("clock", CLOCK, &clocks)?;
                clock_source = node.input("clock").and_then(|input| input.sources.first()).map(|source| format!("{}.{}", qualified(&source.from.node), source.from.port));
                let out = spans.len() as u16;
                spans.insert((node.id.clone(), "audio".into()), out);
                let (amp, pan) = (control(&mut layout, "amp")?, control(&mut layout, "pan")?);
                let (mute, solo) = (control(&mut layout, "mute")?, control(&mut layout, "solo")?);
                let probe = probe(&mut layout, "peak");
                sounding = true;
                let inst = loop_inst(node)?;
                inst_class = match inst {
                    makepad_score_player::Inst::Ironfish { .. } => 1,
                    makepad_score_player::Inst::Shader { .. } => 2,
                    makepad_score_player::Inst::Morph { .. } => 8,
                    makepad_score_player::Inst::Tandem { .. } => 9,
                    // A choir is made for its singers and style, and for a
                    // library singer that has (or has not yet) arrived.
                    makepad_score_player::Inst::Voice { .. } => {
                        let arrived = text(node, "profile").is_ok_and(|name| crate::feed::voices::get(name).is_some());
                        let text = format!("{:?}{:?}{:?}{:?}{arrived}", node.param("profile"), node.param("singers"), node.param("parts"), node.param("style"));
                        text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)) | 3
                    }
                    // Native instruments are made for their kinds and programs.
                    makepad_score_player::Inst::Native { .. } => {
                        let text = format!("{:?}{:?}{:?}{:?}", node.param("native"), node.param("preset"), node.param("font"), node.param("programs"));
                        text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)) | 4
                    }
                    makepad_score_player::Inst::Midi { .. } => {
                        crate::feed::midi_out::init();
                        5
                    }
                    makepad_score_player::Inst::Input { .. } => 6,
                    #[cfg(not(target_arch = "wasm32"))]
                    makepad_score_player::Inst::Neural { .. } => 7,
                    _ => 0,
                };
                // A loop that carries its state over (the same instrument on
                // the same clock) keeps its feed and its voices, and its slow
                // instruments are not built again; a new one takes a fresh
                // ring from the composer's side.
                // A loop on the same clock keeps its feed and the events
                // waiting in it whatever it plays now (an instrument switch
                // keeps the notes); a new one takes a fresh ring.
                let keeps = |same_class: bool| base.filter(|base| base.rate == rate).is_some_and(|base| base.nodes.iter().any(|old| {
                    old.id == node.id && old.type_name == "Loop" && old.clock_source == clock_source && (!same_class || old.inst_class == inst_class)
                }));
                let carries = keeps(true);
                let feed = (!keeps(false)).then(|| crate::feed::loop_feed(&node.id).fresh());
                let engine = makepad_score_player::loop_engine::LoopEngine::for_inst(crate::feed::key_of(&node.id), &inst, rate as f32, carries)
                    .map_err(|error| PlanError::at(&node.id, Some("inst"), error))?;
                let midi_live = matches!(inst, makepad_score_player::Inst::Midi { .. }) && crate::feed::midi_out::is_live(&node.id);
                let input = matches!(inst, makepad_score_player::Inst::Input { .. }).then(crate::feed::audio_in::fresh);
                // A recipe synth's or a shader's settings can be held live.
                let live = matches!(inst, makepad_score_player::Inst::Synth { .. } | makepad_score_player::Inst::Shader { .. }).then(|| crate::feed::live_params(&node.id));
                states.push(NodeState::Loop(Box::new(LoopState { engine, feed, gain_l: 0.0, gain_r: 0.0, peak: 0.0, playing: false, inst_class, midi_live, input, live, live_seen: None })));
                ops.push(Op::Loop { state, clock, out, amp, pan, mute, solo, probe, inst });
            }
            "Pads" => {
                let out = spans.len() as u16;
                spans.insert((node.id.clone(), "audio".into()), out);
                let (amp, mute) = (control(&mut layout, "amp")?, control(&mut layout, "mute")?);
                let probe = probe(&mut layout, "peak");
                sounding = true;
                let list = |name: &str| match node.param(name) {
                    Some(Literal::Arr(items)) => items.clone(),
                    _ => Vec::new(),
                };
                let samples: Arc<[Option<Arc<makepad_score_player::Sample>>]> = list("samples").iter().map(|item| match item {
                    Literal::Str(name) if !name.is_empty() => crate::feed::samples::get(name),
                    _ => None,
                }).collect();
                let gains: Arc<[f32]> = (0..samples.len()).map(|pad| match list("gains").get(pad) {
                    Some(Literal::Num(gain)) if gain.is_finite() => gain.clamp(0.0, 4.0) as f32,
                    _ => 1.0,
                }).collect();
                // Pads that carry their state over keep their feed and voices.
                let carries = base.filter(|base| base.rate == rate).is_some_and(|base| base.nodes.iter().any(|old| old.id == node.id && old.type_name == "Pads"));
                states.push(NodeState::Pads(Box::new(PadsState {
                    samples, gains, voices: Vec::with_capacity(PAD_VOICES),
                    feed: (!carries).then(|| crate::feed::pad_feed(&node.id).fresh()),
                    serial: 0, gain: 0.0, peak: 0.0,
                })));
                ops.push(Op::Pads { state, out, amp, mute, probe });
            }
            kind if makepad_score_player::FxKind::parse(kind).is_some_and(|fx| fx.name() == kind) => {
                let fx = makepad_score_player::FxKind::parse(kind).unwrap_or(makepad_score_player::FxKind::Bus);
                let mut inputs = Vec::new();
                for source in node.input("audio").map(|input| input.sources.as_slice()).unwrap_or_default() {
                    let span = spans
                        .get(&(qualified(&source.from.node), source.from.port.clone()))
                        .ok_or_else(|| PlanError::at(&node.id, Some("audio"), "an entry is not an audio node's port"))?;
                    inputs.push(*span);
                }
                let key = source("key", AUDIO, &spans)?;
                let out = spans.len() as u16;
                spans.insert((node.id.clone(), "audio".into()), out);
                let mut params = [0u16; makepad_score_player::fx::MAX_FX_PARAMS];
                for (slot, (name, ..)) in params.iter_mut().zip(fx.params()) {
                    *slot = control(&mut layout, name)?;
                }
                // A Comp's gain reduction (dB, ≤ 0) for its meter.
                let gr = probe(&mut layout, "gr");
                let probe = probe(&mut layout, "peak");
                sounding = true;
                // An FX that carries its state over keeps its automation feed.
                let carries = base.filter(|base| base.rate == rate).is_some_and(|base| base.nodes.iter().any(|old| old.id == node.id && old.type_name == node.type_name));
                states.push(NodeState::Fx(Box::new(FxNodeState {
                    fx: makepad_score_player::fx::FxState::new(fx, rate as f32), peak: 0.0,
                    feed: (!carries).then(|| crate::feed::automation_feed(&node.id).fresh()),
                    waiting: Vec::with_capacity(crate::feed::AUTOMATION_EVENTS), ramps: [None; makepad_score_player::fx::MAX_FX_PARAMS],
                })));
                ops.push(Op::Fx { state, kind: fx, inputs: inputs.into_boxed_slice(), key, out, params, count: fx.params().len() as u8, probe, gr });
            }
            "ShaderFx" => {
                let mut inputs = Vec::new();
                for source in node.input("audio").map(|input| input.sources.as_slice()).unwrap_or_default() {
                    let span = spans
                        .get(&(qualified(&source.from.node), source.from.port.clone()))
                        .ok_or_else(|| PlanError::at(&node.id, Some("audio"), "an entry is not an audio node's port"))?;
                    inputs.push(*span);
                }
                let out = spans.len() as u16;
                spans.insert((node.id.clone(), "audio".into()), out);
                let probe = probe(&mut layout, "peak");
                sounding = true;
                let (shader, params) = shader_of(node, makepad_score_player::shader::Kind::Effect)?;
                let mut fx = makepad_score_player::shader::ShaderFx::new(shader, rate as f32);
                for (index, value) in params.iter() {
                    fx.set_param(*index as usize, *value);
                }
                let applied = std::sync::Arc::as_ptr(&params) as *const u8 as usize;
                states.push(NodeState::ShaderFx(Box::new(ShaderFxState { fx, input: vec![0.0; 2 * CHUNK].into_boxed_slice(), params: applied, peak: 0.0 })));
                ops.push(Op::ShaderFx { state, inputs: inputs.into_boxed_slice(), out, params, probe });
            }
            "AudioOutput" | "CueOutput" => {
                let cue = node.type_name == "CueOutput";
                let input = source("audio", AUDIO, &spans)?;
                let out = spans.len() as u16;
                spans.insert((node.id.clone(), "waveform".into()), out);
                sinks.push(out);
                let enabled = control(&mut layout, "enabled")?;
                let probes = (probe(&mut layout, "peak_l"), probe(&mut layout, "peak_r"));
                states.push(NodeState::Output { peak_l: 0.0, peak_r: 0.0, gate: 0.0 });
                ops.push(Op::Output { state, input, out, enabled, probes, cue });
            }
            other => {
                let mut cx = ExtCx {
                    node, state, rate, generation, base, layout: &mut layout, spans: &mut spans, observed: &mut deck_states,
                    sounding: &mut sounding, control: &mut control, probe: &mut probe,
                };
                match E::compile(&mut cx)? {
                    Some((ext_state, ext_op)) => {
                        states.push(NodeState::Ext(ext_state));
                        ops.push(Op::Ext(ext_op));
                    }
                    None => match crate::nested::factory(other) {
                        Some(factory) => {
                            let clock = source("clock", CLOCK, &clocks)?;
                            clock_source = node.input("clock").and_then(|input| input.sources.first()).map(|source| format!("{}.{}", qualified(&source.from.node), source.from.port));
                            let out = spans.len() as u16;
                            spans.insert((node.id.clone(), "audio".into()), out);
                            let probe = probe(&mut layout, "peak");
                            sounding = true;
                            states.push(NodeState::Nested(factory(node, rate, generation, base)?));
                            ops.push(Op::Nested { state, clock, out, probe });
                        }
                        None => return Err(PlanError::at(&node.id, None, format!("the audio owner has no node type `{other}`"))),
                    },
                }
            }
        }
        layout.nodes.push(LayoutNode { id: node.id.clone(), type_name: node.type_name.clone(), state, clock_source, inst_class, sounding });
    }
    for ((node, port), index) in &spans {
        layout.audio_probes.insert(format!("{node}.{port}"), *index);
    }

    // State transfer is only defined against the exact base this was
    // prepared for, at the same device rate.
    let mut migrate = Vec::new();
    let mut migrate_decks = Vec::new();
    let mut dip = false;
    if let Some(base) = base {
        let compatible = base.rate == rate;
        for old in base.nodes.iter().filter(|old| E::keeps_across_rates(&old.type_name)) {
            if let Some(new) = layout.nodes.iter().find(|new| new.id == old.id && new.type_name == old.type_name) {
                migrate_decks.push((old.state, new.state));
            }
        }
        for old in &base.nodes {
            let new = layout.nodes.iter().find(|new| new.id == old.id && new.type_name == old.type_name);
            match new {
                // A loop keeps its voices and its feed only on the same
                // clock.
                Some(new) if compatible && (new.type_name != "Loop" || new.clock_source == old.clock_source) => migrate.push((old.state, new.state)),
                _ => dip |= old.sounding,
            }
        }
        dip |= base.audio_edges != layout.audio_edges;
    }
    // Every wire into a control must have become a route: a driven input a
    // node does not read as a control is refused, never silently ignored.
    let driven = slice.nodes.iter().flat_map(|node| node.inputs.iter())
        .filter(|input| input.control.is_some() && !input.sources.is_empty()).count();
    if driven != routes.len() {
        return Err(PlanError::general("a wired control input is not one this audio node reads"));
    }
    let mut triggers: Vec<(u32, u16, u64)> = layout
        .nodes
        .iter()
        .filter_map(|node| layout.triggers.get(&node.id).map(|(id, continuity)| (*id, node.state, *continuity)))
        .collect();
    triggers.sort_unstable();
    if triggers.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(PlanError::general("two nodes hash to the same trigger id; rename one of them"));
    }

    let schedule = Box::new(Schedule {
        generation,
        incarnation,
        format,
        base: base.map(|base| base.generation),
        rate,
        dip,
        ops: ops.into_boxed_slice(),
        states: states.into_boxed_slice(),
        arena: vec![0.0; spans.len() * SPAN].into_boxed_slice(),
        last_frames: 0,
        sinks: sinks.into_boxed_slice(),
        clocks: vec![ClockChunk::default(); clocks.len()].into_boxed_slice(),
        probes: vec![0.0; probe_count as usize].into_boxed_slice(),
        controls: Arc::new(ControlBank::new(&bank)),
        routes: routes.into_boxed_slice(),
        migrate: migrate.into_boxed_slice(),
        migrate_decks: migrate_decks.into_boxed_slice(),
        triggers: triggers.into_boxed_slice(),
        decks: deck_states.into_boxed_slice(),
        board: B::default(),
        rest: crate::rest::Rest::new(bank.len()),
        _permit: permit,
    });
    Ok(Compiled { schedule, layout: Arc::new(layout) })
}

/// The continuity a target keeps from the base generation when the same
/// node of the same type ran there at `rate`; else this generation.
pub fn continuity_of(base: Option<&Layout>, rate: u32, node: &LiveNode, generation: u64) -> u64 {
    base.filter(|base| base.rate == rate)
        .and_then(|base| base.nodes.iter().find(|old| old.id == node.id && old.type_name == node.type_name).and_then(|_| base.triggers.get(&node.id)))
        .map(|(_, continuity)| *continuity)
        .unwrap_or(generation)
}

/// A `Loop` node's instrument, built from its authored settings.
fn loop_inst(node: &LiveNode) -> Result<makepad_score_player::Inst, PlanError> {
    use makepad_score_player::{Inst, KitSlot};
    use makepad_audio_synth::recipe::Recipe;
    let key = crate::feed::key_of(&node.id);
    let recipe = |text: &str, param: &str| Recipe::parse(text).map_err(|error| PlanError::at(&node.id, Some(param), error.0));
    Ok(match text(node, "inst")? {
        "recipe" => Inst::Synth { key, recipe: recipe(text(node, "recipe")?, "recipe")?, mono: number(node, "mono").unwrap_or(0.0) >= 0.5 },
        "kit" => {
            let list = |name: &str| match node.param(name) {
                Some(Literal::Arr(items)) => items.clone(),
                _ => Vec::new(),
            };
            let (texts, gains, pitches, samples) = (list("kit"), list("kit_gain"), list("kit_pitch"), list("kit_sample"));
            let num = |items: &[Literal], i: usize, default: f32| match items.get(i) {
                Some(Literal::Num(n)) => *n as f32,
                _ => default,
            };
            let mut slots = Vec::with_capacity(texts.len());
            for (i, item) in texts.iter().enumerate() {
                let recipe = match item {
                    Literal::Str(text) if !text.is_empty() => Some(recipe(text, "kit")?),
                    _ => None,
                };
                let sample = match samples.get(i) {
                    Some(Literal::Str(name)) if !name.is_empty() => crate::feed::samples::get(name),
                    _ => None,
                };
                slots.push(KitSlot { recipe, sample, gain: num(&gains, i, 1.0), pitch: num(&pitches, i, 1.0) });
            }
            Inst::Drums { key, slots: slots.into() }
        }
        "sampler" => match crate::feed::samples::get(text(node, "sample").unwrap_or("")) {
            Some(sample) => Inst::Sampler {
                key,
                sample,
                root: number(node, "root").unwrap_or(60.0) as f32,
                attack: number(node, "attack").unwrap_or(0.002) as f32,
                release: number(node, "release").unwrap_or(0.05) as f32,
            },
            // Not loaded (yet): silent, never an error.
            None => Inst::Silent,
        },
        "ironfish" => {
            let values: Vec<f32> = match node.param("iron") {
                Some(Literal::Arr(items)) => items.iter().map(|item| match item {
                    Literal::Num(n) => *n as f32,
                    _ => f32::NAN,
                }).collect(),
                _ => Vec::new(),
            };
            let program = number(node, "program").unwrap_or(0.0).max(0.0) as usize;
            Inst::Ironfish { key, patch: makepad_score_player::presets::iron_patch(program, &values) }
        }
        // Morph: its patch text (checked by the score), the samples its
        // oscillators read, and the tables built here, off the audio thread.
        "morph" => {
            let patch = makepad_audio_synth::morph::MorphPatch::parse(text(node, "morph").unwrap_or("")).map_err(|error| PlanError::at(&node.id, Some("morph"), error))?;
            let names: Vec<String> = match node.param("morph_src") {
                Some(Literal::Arr(items)) => items.iter().map(|item| match item {
                    Literal::Str(name) => name.clone(),
                    _ => String::new(),
                }).collect(),
                _ => Vec::new(),
            };
            let sample = |o: usize| names.get(o).filter(|n| !n.is_empty()).and_then(|n| crate::feed::samples::get(n)).map(|s| makepad_audio_synth::kit::osc::SampleBuf { data: s.left.clone(), rate: s.rate });
            let src = patch.sources([sample(0), sample(1), sample(2)]);
            Inst::Morph { key, patch: Arc::new(patch), src }
        }
        // Tandem: the library patch read and compiled here, off the audio
        // thread, with the score's parameters set over it.
        "tandem" => {
            use makepad_audio_synth::tandem::{program::Program, LoadMode, Patch};
            let path = text(node, "tandem_path").unwrap_or("");
            let mut patch = if path.is_empty() {
                Patch::init()
            } else {
                let bytes = std::fs::read(path).map_err(|e| PlanError::at(&node.id, Some("tandem_path"), format!("{path}: {e}")))?;
                let stem = std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                Patch::from_fxp(&bytes, &stem, LoadMode::Patch).map_err(|e| PlanError::at(&node.id, Some("tandem_path"), e))?
            };
            for word in text(node, "tandem_set").unwrap_or("").split_whitespace() {
                if let Some((k, v)) = word.split_once('=') {
                    if let Ok(v) = v.parse::<f32>() {
                        patch.set(k, v);
                    }
                }
            }
            Inst::Tandem { key, prog: Arc::new(Program::compile(&patch)) }
        }
        "shader" => {
            let (shader, params) = shader_of(node, makepad_score_player::shader::Kind::Instrument)?;
            Inst::Shader { key, shader, params, mono: number(node, "mono").unwrap_or(0.0) >= 0.5 }
        }
        // The neural voice: its singer is loaded here (off the audio thread)
        // from the installed voice; none installed is silence, never an error
        // (a browser build has no singer: the voice sings as a choir voice).
        #[cfg(not(target_arch = "wasm32"))]
        "voice" if text(node, "engine").is_ok_and(|e| e == "neural") => match makepad_score_player::sung::neural::shared() {
            Some(singer) => Inst::Neural { key, singer },
            None => Inst::Silent,
        },
        "voice" => {
            use makepad_score_player::sung::{Builtin, ChoirStyle, VoiceProfile};
            let name = text(node, "profile").unwrap_or("alto");
            // A built-in voice, or a library singer by `id@rev` (the built-in
            // alto sings until the host registers it).
            let profile = match (Builtin::from_name(name), crate::feed::voices::get(name)) {
                (Some(builtin), _) => std::sync::Arc::new(VoiceProfile::builtin(builtin)),
                (None, Some(profile)) => profile,
                (None, None) if name.contains('@') => std::sync::Arc::new(VoiceProfile::builtin(Builtin::Alto)),
                (None, None) => return Err(PlanError::at(&node.id, Some("profile"), format!("no singer `{name}`: soprano, alto, tenor, baritone, bass or a library singer"))),
            };
            let mut style = ChoirStyle { parts: number(node, "parts").unwrap_or(1.0).clamp(1.0, 8.0) as u8, ..ChoirStyle::default() };
            if let Some(Literal::Arr(items)) = node.param("style") {
                let b = &mut style.base;
                let slots: [&mut f32; 8] = [&mut b.vibrato_depth, &mut b.vibrato_rate, &mut b.vibrato_delay, &mut b.portamento, &mut b.scoop, &mut b.breath, &mut b.drift_cents, &mut b.jitter_cents];
                for (slot, item) in slots.into_iter().zip(items) {
                    if let Literal::Num(n) = item {
                        if *n >= 0.0 {
                            *slot = *n as f32;
                        }
                    }
                }
            }
            let singers = number(node, "singers").unwrap_or(1.0).clamp(1.0, 8.0) as u8;
            Inst::Voice { key, profiles: vec![profile].into(), singers, style }
        }
        "native" => {
            use makepad_audio_synth::instruments::InstrumentKind;
            let programs: Vec<u8> = match node.param("programs") {
                Some(Literal::Arr(items)) => items.iter().filter_map(|i| match i { Literal::Num(n) => Some(n.clamp(0.0, 127.0) as u8), _ => None }).collect(),
                _ => Vec::new(),
            };
            let programs = if programs.is_empty() { vec![0] } else { programs };
            let preset = text(node, "preset").ok().filter(|p| !p.is_empty()).map(str::to_string);
            let kinds: Vec<(u8, InstrumentKind)> = match text(node, "native")? {
                "piano" => vec![(programs[0], InstrumentKind::Piano { preset })],
                "drums" => vec![(programs[0], InstrumentKind::Drums)],
                "soundfont" => match crate::feed::files::get(text(node, "font").unwrap_or("")) {
                    Some(bytes) => {
                        let bank = number(node, "bank").unwrap_or(0.0).clamp(0.0, 128.0) as u16;
                        programs.iter().map(|p| (*p, InstrumentKind::SoundFont { bytes: bytes.clone(), bank, program: *p })).collect()
                    }
                    // Not loaded (yet): silent, never an error.
                    None => return Ok(Inst::Silent),
                },
                other => return Err(PlanError::at(&node.id, Some("native"), format!("`{other}` is not a native instrument: piano, drums or soundfont"))),
            };
            Inst::Native { key, kinds: kinds.into(), gain: number(node, "gain").unwrap_or(1.0).clamp(0.0, 4.0) as f32 }
        }
        "input" => Inst::Input { key, gain: number(node, "gain").unwrap_or(1.0).clamp(0.0, 4.0) as f32 },
        "midi" => {
            let channel = (number(node, "channel").unwrap_or(1.0).clamp(1.0, 16.0) as u8) - 1;
            Inst::Midi { key, channel, port: crate::feed::midi_out::port_hash(text(node, "midi_port").unwrap_or("")) }
        }
        "" | "silent" => Inst::Silent,
        other => return Err(PlanError::at(&node.id, Some("inst"), format!("`{other}` is not an instrument: recipe, kit, sampler, ironfish, shader, voice, native, midi or input"))),
    })
}

/// A shader node's program (compiled here, off the audio thread) and its
/// settings as (param index, value).
#[allow(clippy::type_complexity)]
fn shader_of(node: &LiveNode, kind: makepad_score_player::shader::Kind) -> Result<(makepad_score_player::shader::AudioShaderRef, std::sync::Arc<[(u16, f32)]>), PlanError> {
    let code = text(node, "shader")?;
    let shader = makepad_score_player::shader::compile(code).map_err(|errors| {
        let first = errors.first().map(|e| e.message.clone()).unwrap_or_default();
        PlanError::at(&node.id, Some("shader"), format!("the shader does not compile: {first}"))
    })?;
    if shader.kind() != kind {
        let want = if kind == makepad_score_player::shader::Kind::Instrument { "an instrument (with voice())" } else { "an effect" };
        return Err(PlanError::at(&node.id, Some("shader"), format!("the shader is not {want}")));
    }
    let mut params = Vec::new();
    if let Some(Literal::Arr(items)) = node.param("shader_params") {
        for pair in items.chunks(2) {
            if let [Literal::Str(name), Literal::Num(value)] = pair {
                let index = makepad_score_player::shader::param_index(&shader, name).ok_or_else(|| PlanError::at(&node.id, Some("shader_params"), format!("the shader has no param `{name}`")))?;
                params.push((index as u16, *value as f32));
            }
        }
    }
    Ok((shader, params.into()))
}


#[cfg(test)]
mod budget_tests {
    use super::*;

    /// The account follows the schedules: a dropped one gives its bytes
    /// back, and nothing refuses for size.
    #[test]
    fn prepared_bytes_are_an_account_not_a_limit() {
        let budget = Arc::new(AudioBudget::default());
        let permits: Vec<BudgetPermit> = (0..64).map(|_| budget.reserve(4 << 20)).collect();
        assert_eq!(budget.prepared_bytes(), 64 * (4 << 20));
        drop(permits);
        assert_eq!(budget.prepared_bytes(), 0);
    }
}
