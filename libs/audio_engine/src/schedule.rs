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
use makepad_flow::document::ident::DPort;
use makepad_flow::live::owner::PrepareControl;
use makepad_flow::live::plan::{qualified, EndpointKind, LiveNode, OwnerSlice, PlanError};
use makepad_flow::Literal;
use makepad_audio_dsp::svf::Svf;
use makepad_score_player::shader::{align, AlignEdge, AlignNode, AudioBindings, AudioShader, AudioShaderRef, ProbeRecord, ProbeTap, SongTimeDelay};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

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
        ("Transport", "beats_per_bar") => (1.0, 32.0, false),
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
    /// Locates the transport has done (it changes on the pass of one).
    pub located: u32,
    /// The song's time over the pass, for shaders.
    pub time: makepad_score_player::shader::SongTime,
}

/// A node level, sampled on the 64-frame grid and advanced before each sample.
#[derive(Clone, Copy)]
pub struct GainRamp {
    value: f32,
    target: f32,
    step: f32,
    left: u16,
}

impl GainRamp {
    pub(crate) fn new(value: f32) -> Self {
        Self { value, target: value, step: 0.0, left: 0 }
    }

    pub(crate) fn target(&mut self, value: f32, frame: u32) {
        // The old aligned pass also corrected its accumulated rounding error
        // at the next 256-frame boundary. Preserve the exact addition order.
        if value != self.target || (self.left == 0 && self.value != value && frame % 256 == 0) {
            self.target = value;
            self.step = (value - self.value) / 256.0;
            self.left = 256;
        }
    }

    pub(crate) fn advance(&mut self, snap: bool) -> f32 {
        if self.left == 0 { return self.value; }
        self.value += self.step;
        self.left -= 1;
        let sample = self.value;
        if snap && self.left == 0 { self.value = self.target; }
        sample
    }
}

/// `X` is an extension's node state (the decks of Stage's audio owner).
pub enum NodeState<X> {
    None,
    /// The beat is counted from an anchor in frames, not summed pass by
    /// pass, so it is the same however a buffer is cut into passes: `anchor`
    /// is the beat where the tempo or play state last changed, `frames` the
    /// frames played since, at `bpm` (NaN before the first pass).
    Transport(Box<TransportState>),
    Delay(Box<DelayState>),
    Filter { left: Svf, right: Svf, cutoff: f32, q: f32, coeffs: makepad_audio_dsp::svf::SvfCoeffs },
    Gain { left: GainRamp, right: GainRamp, peak: f32 },
    Mixer { master: GainRamp },
    StemMixer { gains: [GainRamp; 4] },
    Output { peak_l: f32, peak_r: f32, gate: f32, hold: u64 },
    Loop(Box<LoopState>),
    Pads(Box<PadsState>),
    Fx(Box<FxNodeState>),
    ShaderFx(Box<ShaderFxState>),
    /// A player of its own ([`crate::nested`]).
    Nested(Box<dyn crate::nested::NestedNode>),
    Ext(X),
}

#[derive(Default)]
pub(crate) struct DelayTap {
    pub(crate) ring: Box<[[f32; 2]]>,
    pub(crate) write: usize,
    pub(crate) frames: usize,
}

pub struct DelayState {
    pub(crate) current: DelayTap,
    // Retain the old allocation after the fade. The host retires it.
    pub(crate) previous: DelayTap,
    pub(crate) warmup: usize,
    pub(crate) fade_left: usize,
    // Prepare can stop reserving the older ring (and omit a direct op)
    // once the playing generation reports that it has finished with it.
    fade_pending: Option<Arc<AtomicBool>>,
    report_completion: bool,
}

impl DelayState {
    fn start_fade(&mut self) {
        self.warmup = self.current.frames;
        self.fade_left = makepad_score_player::shader::FADE as usize;
    }

    pub(crate) fn report_if_finished(&mut self) {
        if self.report_completion && self.fade_left == 0 {
            if let Some(pending) = &self.fade_pending { pending.store(false, Ordering::Release); }
            self.report_completion = false;
        }
    }
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
    if let (NodeState::Delay(playing), NodeState::Delay(prepared)) = (&mut *old, &mut *new) {
        if playing.current.frames == prepared.current.frames {
            std::mem::swap(&mut playing.current, &mut prepared.current);
            if playing.fade_left != 0 {
                std::mem::swap(&mut playing.previous, &mut prepared.previous);
                prepared.warmup = playing.warmup;
                prepared.fade_left = playing.fade_left;
            }
        } else if playing.fade_left != 0 && playing.previous.frames == prepared.current.frames {
            // An edit back to the old tap can reverse a fade in place. Both
            // histories are already live; during warmup the old tap is at unity.
            std::mem::swap(&mut playing.previous, &mut prepared.current);
            if playing.warmup == 0 {
                std::mem::swap(&mut playing.current, &mut prepared.previous);
                prepared.fade_left = makepad_score_player::shader::FADE as usize - playing.fade_left;
            }
        } else {
            // If the previous edit is still filling its ring, the audience
            // is hearing its previous tap. Continue from that actual history.
            let audible = if playing.warmup != 0 { &mut playing.previous } else { &mut playing.current };
            std::mem::swap(audible, &mut prepared.previous);
            prepared.start_fade();
        }
        return;
    }
    std::mem::swap(old, new);
    match (old, new) {
        (NodeState::Loop(fresh), NodeState::Loop(kept)) => {
            // What the new plan decided about MIDI out and audio in holds.
            kept.midi_live = fresh.midi_live;
            std::mem::swap(&mut fresh.input, &mut kept.input);
            // The held settings go on the new revision's instrument afresh.
            std::mem::swap(&mut fresh.live, &mut kept.live);
            kept.live_seen = None;
            // MIDI in: the new revision's stream and ring, the keys held
            // and the arps' chords carried on.
            std::mem::swap(&mut fresh.stream, &mut kept.stream);
            if let (Some(new), Some(old)) = (kept.stream.as_deref_mut(), fresh.stream.as_deref_mut()) {
                new.take_state(old);
            }
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
        (NodeState::Fx(fresh), NodeState::Fx(kept)) => {
            // Keep the sounding DSP and its last values through a partial block.
            // An automation event from the old plan cannot outlive a wire.
            std::mem::swap(&mut fresh.wired, &mut kept.wired);
            std::mem::swap(&mut fresh.automated, &mut kept.automated);
            let retired = |i: usize| fresh.automated[i] && !kept.automated[i];
            let discard = std::array::from_fn(|i| kept.wired[i] || retired(i));
            for (ramp, &drop_event) in kept.ramps.iter_mut().zip(&discard) {
                if drop_event { *ramp = None; }
            }
            kept.waiting.retain(|event| discard.get(usize::from(event.param)) != Some(&true));
            if (0..discard.len()).any(retired) {
                // Also filter events already sent but not picked up by the last pass.
                kept.collect_events(Some(&discard));
            }
        }
        // A nested player plays on unless it now plays something else.
        (NodeState::Nested(fresh), NodeState::Nested(kept)) => {
            if fresh.key() != kept.key() {
                std::mem::swap(fresh, kept);
            }
        }
        (NodeState::ShaderFx(fresh), NodeState::ShaderFx(kept)) => {
            if !fresh.fx.same_program_and_seed(&kept.fx) {
                fresh.fx.swap_from(&mut kept.fx);
                std::mem::swap(&mut fresh.fx, &mut kept.fx);
                kept.params = 0;
            } else {
                kept.fx.take_prepared_song_time(&mut fresh.fx);
                kept.fx.take_prepared_wires(&mut fresh.fx);
                kept.fx.swap_probes(&mut fresh.fx);
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
    pub feed: Option<makepad_lock_free::Consumer<crate::feed::TempoEvent>>,
    /// Changes waiting for their beat, in beat order (capacity reserved).
    pub pending: Vec<crate::feed::TempoEvent>,
    /// A ramp under way: (start beat, from bpm, to bpm, steps, next step).
    pub ramp: Option<(f64, f64, f64, u32, u32)>,
    /// The tempo playing came from the map (not the bpm control): the
    /// control is written back so a host reading it sees the tempo.
    pub mapped: bool,
    /// Locates done (`TempoEvent::locate`).
    pub located: u32,
    /// A beat at a whole frame of `frames`' count, and that frame: the
    /// anchor a shader's `beat` counts from. Set once each time the
    /// transport re-anchors, so every pass reads the same numbers whatever
    /// its slicing.
    pub shader_anchor: (f64, u64),
}

impl LoopState {
    /// The loop's clock is not running it (a stopped transport).
    pub fn playing_clear(&self) -> bool {
        !self.playing && !self.stream.as_ref().is_some_and(|s| s.busy())
    }

    /// Something waits to be played: events from the composer, or MIDI in.
    pub fn has_input(&self) -> bool {
        self.feed.as_ref().is_some_and(|feed| !feed.is_empty()) || self.stream.as_ref().is_some_and(|s| s.has_input())
    }
}

/// A `Loop` node between passes: its voices and the events waiting for
/// their chunk, the reading end of its feed, and its smoothed gains.
pub struct LoopState {
    pub engine: makepad_score_player::loop_engine::LoopEngine,
    pub feed: Option<makepad_lock_free::Consumer<makepad_score_player::Event>>,
    pub gain_l: GainRamp,
    pub gain_r: GainRamp,
    pub peak: f32,
    pub playing: bool,
    /// What kind of instrument its engine was made for.
    pub inst_class: u64,
    /// A MIDI loop of the live score: its MIDI goes out.
    pub midi_live: bool,
    /// An input loop's ring from the audio input.
    pub input: Option<makepad_lock_free::Consumer<[f32; 2]>>,
    /// Its instrument's settings an editor holds live, and the version of
    /// them last set on the engine (none yet: set on the first pass).
    pub live: Option<Arc<crate::feed::LiveParams>>,
    pub live_seen: Option<u32>,
    /// Played from MIDI in: its ring and note stream.
    pub stream: Option<Box<crate::midi_stream::StreamRun>>,
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
    pub feed: Option<makepad_lock_free::Consumer<crate::feed::PadHit>>,
    pub serial: u64,
    pub gain: GainRamp,
    pub peak: f32,
}

/// An FX node between passes.
pub struct FxNodeState {
    pub fx: makepad_score_player::fx::FxState,
    pub peak: f32,
    /// A Duck inserted inside a block starts collecting at the next block start.
    pub(crate) key_peak: Option<f32>,
    pub(crate) previous_key_level: f32,
    /// Last applied targets, including the value held between block starts.
    pub values: [f32; makepad_score_player::fx::MAX_FX_PARAMS],
    pub wired: [bool; makepad_score_player::fx::MAX_FX_PARAMS],
    /// Params bound to automation in this plan; a binding that ends releases its target.
    pub automated: [bool; makepad_score_player::fx::MAX_FX_PARAMS],
    /// The score's automation: its feed, what waits for its beat, and each
    /// setting's ramp (from, to, start beat, beats) once one began.
    pub feed: Option<makepad_lock_free::Consumer<crate::feed::AutomationEvent>>,
    pub waiting: Vec<crate::feed::AutomationEvent>,
    pub ramps: [Option<(f32, f32, f64, f64)>; makepad_score_player::fx::MAX_FX_PARAMS],
}

/// The prepared arena spans for one shader instance, in host binding order.
#[derive(Default)]
pub struct ShaderPorts {
    pub sides: Vec<(usize, Box<[u16]>)>,
    pub outs: Vec<(usize, u16)>,
    pub params: Vec<(usize, Box<[u16]>)>,
}

/// `Y` is an extension's op (the decks of Stage's audio owner).
pub enum Op<Y> {
    /// A changed tap plays the old alignment while the new ring fills, then
    /// fades to the new tap. Its sibling may already use the new latency,
    /// briefly shifting the phase of a dry/wet mix.
    Delay { state: u16, input: u16, out: u16, frames: usize },
    Transport { state: u16, clock: u16, bpm: u16, playing: u16, beats_per_bar: u16, probe: u16, mapped: u16 },
    Filter { state: u16, input: Option<u16>, out: u16, cutoff: u16, resonance: u16 },
    Gain { state: u16, input: Option<u16>, out: u16, gain: u16, pan: u16, mute: u16, probe: u16 },
    Mixer { state: u16, inputs: Box<[u16]>, out: u16, master: u16 },
    StemMixer { state: u16, input: Option<u16>, stems: [Option<u16>; 4], available: Option<u16>, out: u16, gains: [u16; 4], mutes: [u16; 4], solos: [u16; 4] },
    /// `cue`: the headphone cue's sink (CueOutput), else the device's.
    Output { state: u16, input: Option<u16>, out: u16, enabled: u16, probes: (u16, u16), cue: bool },
    Loop { state: u16, clock: Option<u16>, out: u16, amp: u16, pan: u16, mute: u16, solo: u16, probe: u16, inst: makepad_score_player::Inst, ports: ShaderPorts },
    Pads { state: u16, out: u16, amp: u16, mute: u16, probe: u16 },
    Fx { state: u16, kind: makepad_score_player::FxKind, inputs: Box<[u16]>, key: Option<u16>, out: u16, params: [u16; makepad_score_player::fx::MAX_FX_PARAMS], wires: Box<[(usize, Box<[u16]>)]>, count: u8, probe: u16, gr: u16 },
    ShaderFx { state: u16, inputs: Box<[u16]>, sides: Box<[(usize, Box<[u16]>)]>, outs: Box<[(usize, u16)]>, wires: Box<[(usize, Box<[u16]>)]>, out: u16, params: std::sync::Arc<[(u16, f32)]>, probe: Option<u16> },
    Nested { state: u16, clock: Option<u16>, out: u16, probe: u16 },
    Ext(Y),
}

/// `B`: what an extension publishes from the schedule (its telemetry
/// board, sized when the schedule is prepared); nothing by default.
pub struct Schedule<X, Y, B = ()> {
    /// Frames to the aligned device outputs, or the longest path in a hosted graph.
    pub latency: u64,
    pub generation: u64,
    /// Frame at the start of the next render pass.
    pub frame: u64,
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
    /// Used only when several internal passes must publish one host buffer.
    pub(crate) assembled: Box<[f32]>,
    pub(crate) transport_ops: Box<[usize]>,
    pub(crate) clock_inputs: Box<[(u16, ClockChunk)]>,
    pub(crate) outputs: Box<[(u16, bool, bool)]>,
    pub(crate) rest_hold: u64,
    /// Length of the final internal pass described by the published clocks.
    pub clock_frames: usize,
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
    /// These edges carried direct audio in the base. Their old tap is the input.
    fade_from_direct: Box<[u16]>,
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
    /// Frames this node adds.
    pub own_latency: u32,
    /// Frame at which its aligned inputs arrive.
    pub l_in: u64,
    pub type_name: String,
    pub state: u16,
    pub clock_source: Option<String>,
    /// A loop's kind of instrument (its state only carries over to the
    /// same kind: an Ironfish loop's synth, a choir, live in its state).
    pub inst_class: u64,
    pub sounding: bool,
}

/// A delayed input, stable across revisions and independent of mixer entry order.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DelayKey {
    pub consumer: String,
    pub port: String,
    pub source: DPort,
}

#[derive(Clone, Debug)]
pub struct LayoutDelay {
    pub frames: usize,
    capacity: usize,
    previous_capacity: usize,
    fade_pending: Option<Arc<AtomicBool>>,
    state: u16,
}

/// Shared across revisions of a score. Only its host pump updates the reported totals.
#[derive(Debug, Default)]
struct PlaybackCounters {
    reported_dropped_records: AtomicU64,
}

/// What the host keeps about a generation: enough to address its controls
/// and probes by name and to prepare the next generation against it.
#[derive(Clone, Debug)]
pub struct Layout {
    /// Non-error diagnostics for the prepared graph.
    pub plan_notes: Vec<PlanNote>,
    counters: Arc<PlaybackCounters>,
    pub latency: u64,
    nested: Vec<(Arc<AtomicU32>, u32)>,
    pub delays: HashMap<DelayKey, LayoutDelay>,
    pub generation: u64,
    pub rate: u32,
    pub nodes: Vec<LayoutNode>,
    shaders: HashMap<String, Arc<PreparedShader>>,
    song_time_delays: HashMap<String, SongTimeDelay>,
    /// `node.param` to its bank index and authored value.
    pub controls: HashMap<String, (u16, f64)>,
    /// Driven `node.param` to the `node.port` that drives it. A driven
    /// control refuses writes; its bank slot holds the effective value.
    pub drivers: HashMap<String, String>,
    /// `node.port` to its probe index.
    pub probes: HashMap<String, u16>,
    pub probe_taps: HashMap<String, Arc<ProbeReader>>,
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

/// A prepared graph's note about a named input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanNote {
    pub node: String,
    pub port: String,
    /// The authored source endpoint as shown to the user.
    pub source: String,
    /// How many frames late the input arrives.
    pub frames: u64,
    pub message: String,
}

/// Public endpoints and list-entry identity of an authored edge. Several
/// physical paths can contribute to the lateness of this one edge.
#[derive(Eq, Ord, PartialEq, PartialOrd)]
pub struct NoteEdge {
    pub source: String,
    pub node: String,
    pub port: String,
    pub entry: Option<u64>,
}

/// The previous generation's compiler inputs. Weak recording references
/// distinguish replacement or arrival without keeping retired samples alive.
#[derive(Debug)]
struct PreparedShader {
    code: String,
    modules: Option<Literal>,
    oversample: Vec<Literal>,
    samples: Vec<(String, std::sync::Weak<makepad_score_player::Sample>)>,
    shader: AudioShaderRef,
}

/// The host's end of one shader's stream. Layout snapshots share this reader.
pub struct ProbeReader {
    pub names: Vec<String>,
    consumer: Mutex<makepad_lock_free::Consumer<ProbeRecord>>,
    folded: Arc<AtomicU64>,
    _permit: BudgetPermit,
}

impl std::fmt::Debug for ProbeReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProbeReader").field("names", &self.names).finish_non_exhaustive()
    }
}

impl ProbeReader {
    /// Drain the records waiting at entry and return the cumulative fold count.
    pub fn drain(&self, mut read: impl FnMut(ProbeRecord)) -> u64 {
        let mut ring = self.consumer.lock().unwrap_or_else(|e| e.into_inner());
        for _ in 0..ring.len() {
            read(ring.pop().unwrap());
        }
        self.folded.load(Ordering::Relaxed)
    }
}

impl Layout {
    /// Total song-time records lost by this layout's delays, for the host pump to log.
    pub fn dropped_song_time_records(&self) -> u64 {
        self.song_time_delays.values().map(SongTimeDelay::dropped_records).sum()
    }

    /// Called by the score's host pump on its playing layout. Removing a delay
    /// can reduce the dropped total, which becomes the baseline for the next report.
    pub fn take_report(&self) -> Option<String> {
        let dropped = self.dropped_song_time_records();
        let previous = self.counters.reported_dropped_records.swap(dropped, Ordering::Relaxed);
        let added = dropped.saturating_sub(previous);
        (added != 0).then(|| format!("audio: dropped song-time records={dropped} (+{added})"))
    }

    /// Whether a nested player has published a different latency since prepare.
    pub fn nested_changed(&self) -> bool {
        self.nested.iter().any(|(cell, used)| cell.load(Ordering::Acquire) != *used)
    }

    fn probe_tap(&mut self, node: &str, shader: &AudioShader, voices: usize, budget: &Arc<AudioBudget>) -> Option<ProbeTap> {
        if shader.probe_count() == 0 {
            return None;
        }
        const CAPACITY: usize = 8192;
        let bytes = CAPACITY * std::mem::size_of::<ProbeRecord>()
            + voices * shader.probe_count() * (std::mem::size_of::<Option<ProbeRecord>>() + std::mem::size_of::<usize>());
        let (ring, consumer) = makepad_lock_free::ring(CAPACITY);
        let folded = Arc::new(AtomicU64::new(0));
        let names = (0..shader.probe_count()).map(|k| shader.probe_name(k).to_owned()).collect();
        self.probe_taps.insert(node.to_owned(), Arc::new(ProbeReader {
            names, consumer: Mutex::new(consumer), folded: folded.clone(), _permit: budget.reserve(bytes),
        }));
        Some(ProbeTap::new(ring, folded))
    }
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

/// Named audio outputs and the private spans used to align their consumers.
#[derive(Default)]
pub struct AudioSpans {
    named: HashMap<(String, String), u16>,
    private: usize,
}

impl AudioSpans {
    /// Next arena slot, including any private spans already allocated.
    pub fn len(&self) -> usize {
        self.named.len() + self.private
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn private_span(&mut self) -> u16 {
        let next = self.len() as u16;
        self.private += 1;
        next
    }
}

// Extensions address named ports through the same map as the engine.
impl std::ops::Deref for AudioSpans {
    type Target = HashMap<(String, String), u16>;

    fn deref(&self) -> &Self::Target {
        &self.named
    }
}

impl std::ops::DerefMut for AudioSpans {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.named
    }
}

/// What an extension's node sees while it is prepared.
pub struct ExtCx<'a, 'b> {
    pub node: &'a LiveNode,
    pub state: u16,
    pub rate: u32,
    pub generation: u64,
    pub base: Option<&'a Layout>,
    pub layout: &'a mut Layout,
    /// The audio spans by (node, port). These are the sources' own spans,
    /// before alignment. The engine's nodes read an input that needs a
    /// delay through the delay's span. An extension node reading an audio
    /// input here would get it undelayed. No extension node reads audio
    /// inputs yet. The first one needs the aligned lookup.
    pub spans: &'a mut AudioSpans,
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
    /// Take over from `old`, the generation this one was planned against
    /// (`base`): the staying nodes' state, and the frame count, so every
    /// shader stays on the one timebase across a revision.
    pub fn take_over(&mut self, old: &mut Self) {
        for (from, to) in self.migrate.iter() {
            migrate_state(&mut old.states[*from as usize], &mut self.states[*to as usize]);
        }
        for &state in &self.fade_from_direct {
            if let NodeState::Delay(delay) = &mut self.states[state as usize] {
                delay.start_fade();
            }
        }
        self.frame = old.frame;
    }

    /// Set clock slot `clock` (a [`CLOCK_IN`] node's) for the next pass.
    pub fn drive_clock(&mut self, clock: u16, chunk: ClockChunk) {
        self.clocks[clock as usize] = chunk;
    }
}

/// Find a closed path through audio sources for an authored cycle diagnostic.
pub fn wire_cycle(reads: &[Vec<usize>]) -> Option<Vec<usize>> {
    let mut visited = vec![0u8; reads.len()];
    let mut path = Vec::new();
    for root in 0..reads.len() {
        if visited[root] != 0 { continue; }
        path.push((root, 0));
        visited[root] = 1;
        while let Some((node, edge)) = path.last_mut() {
            let Some(&next) = reads[*node].get(*edge) else {
                visited[*node] = 2;
                path.pop();
                continue;
            };
            *edge += 1;
            match visited[next] {
                0 => { visited[next] = 1; path.push((next, 0)); }
                1 => {
                    return Some(path.iter().skip_while(|(id, _)| *id != next).map(|&(id, _)| id)
                        .chain(std::iter::once(next)).collect());
                }
                _ => {}
            }
        }
    }
    None
}

pub fn compile<E: Ext, B: Default>(slice: &OwnerSlice, generation: u64, incarnation: u64, format: u64, base: Option<&Layout>, budget: &Arc<AudioBudget>) -> Result<Compiled<E::State, E::Op, B>, PlanError> {
    compile_controlled::<E, B>(slice, generation, incarnation, format, base, budget, None)
}

/// Prepare with the owner's optional cancellation word, including shader initialization.
pub fn compile_controlled<E: Ext, B: Default>(slice: &OwnerSlice, generation: u64, incarnation: u64, format: u64, base: Option<&Layout>, budget: &Arc<AudioBudget>, control: Option<&PrepareControl>) -> Result<Compiled<E::State, E::Op, B>, PlanError> {
    compile_with_note_labels_controlled::<E, B>(slice, generation, incarnation, format, base, budget, control, &|node, port, source| NoteEdge {
        source: format!("{}.{}", qualified(&source.from.node), source.from.port),
        node: node.id.clone(), port: port.to_owned(), entry: source.conn,
    })
}

/// A host which expands an authored edge can supply its public endpoints and
/// entry identity. All of its physical paths then produce one note.
pub fn compile_with_note_labels<E: Ext, B: Default>(slice: &OwnerSlice, generation: u64, incarnation: u64, format: u64, base: Option<&Layout>, budget: &Arc<AudioBudget>,
    label: &dyn Fn(&LiveNode, &str, &makepad_flow::live::plan::LiveSource) -> NoteEdge) -> Result<Compiled<E::State, E::Op, B>, PlanError> {
    compile_with_note_labels_controlled::<E, B>(slice, generation, incarnation, format, base, budget, None, label)
}

/// Prepare with authored edge labels and the same control used by the owner.
pub fn compile_with_note_labels_controlled<E: Ext, B: Default>(slice: &OwnerSlice, generation: u64, incarnation: u64, format: u64, base: Option<&Layout>, budget: &Arc<AudioBudget>, control: Option<&PrepareControl>,
    label: &dyn Fn(&LiveNode, &str, &makepad_flow::live::plan::LiveSource) -> NoteEdge) -> Result<Compiled<E::State, E::Op, B>, PlanError> {
    let check_control = || {
        if control.is_some_and(PrepareControl::is_cancelled) { Err(PlanError::general("prepare cancelled")) } else { Ok(()) }
    };
    check_control()?;
    let bindings = AudioBindings {
        control: control.map(PrepareControl::word),
        executions: control.map(PrepareControl::execution_counter),
        ..AudioBindings::default()
    };
    let rate = format as u32;
    if !(8_000..=384_000).contains(&rate) {
        return Err(PlanError::general(format!("the device rate {rate} Hz is outside what the DSP was built for")));
    }
    // Refuse resource demand before constructing any delay/reverb state.
    let (mut control_count, mut probes, mut audio_spans, mut heavy) = (0usize, 0usize, 0usize, 0usize);
    let mut ids = HashMap::new();
    let mut factories = HashMap::new();
    for (index, node) in slice.nodes.iter().enumerate() {
        if ids.insert(node.id.as_str(), index).is_some() {
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
            "ShaderFx" | "ShaderControl" => {
                heavy += 1;
                (0, usize::from(node.type_name == "ShaderFx"))
            }
            kind if makepad_score_player::FxKind::parse(kind).is_some_and(|fx| fx.name() == kind) => {
                heavy += 1;
                (makepad_score_player::FxKind::parse(kind).map_or(0, |fx| fx.params().len()), 2)
            }
            other => match E::demand(other) {
                Some((controls, probes, _)) => (controls, probes),
                None => {
                    let factory = crate::nested::factory(other).ok_or_else(|| PlanError::at(&node.id, None, "unsupported audio node type"))?;
                    factories.insert(node.id.as_str(), factory);
                    heavy += 1;
                    (0, 1)
                }
            },
        };
        control_count += controls;
        probes += count;
        audio_spans += match node.type_name.as_str() {
            "Filter" | "Gain" | "AudioMixer" | "StemMixer" | "AudioOutput" | "CueOutput" | "Loop" | "Pads" | "ShaderFx" => 1,
            kind if makepad_score_player::FxKind::parse(kind).is_some_and(|fx| fx.name() == kind) => 1,
            other => E::demand(other).map_or(usize::from(factories.contains_key(node.id.as_str())), |(_, _, spans)| spans),
        };
        // Main audio and parameter wires sum their ordered sources. A native
        // sidechain key and a Flow scalar control each take one.
        let native_fx = makepad_score_player::FxKind::parse(&node.type_name).filter(|fx| fx.name() == node.type_name);
        for input in &node.inputs {
            let audio_input = input.ty.as_str() == AUDIO && input.control.is_none();
            if native_fx.is_some_and(|fx| fx.param_index(&input.port).is_none())
                && audio_input && !input.sources.is_empty() && !matches!(input.port.as_str(), "audio" | "key") {
                return Err(PlanError::at(&node.id, Some(&input.port), "unknown effect parameter"));
            }
            let many = (node.type_name == "Loop" && input.ty.as_str() == AUDIO && input.control.is_none()) || matches!(node.type_name.as_str(), "ShaderFx" | "ShaderControl") || (node.type_name == "AudioMixer" && input.port == "inputs")
                || (native_fx.is_some() && audio_input && input.port != "key");
            if !many && input.sources.len() > 1 {
                return Err(PlanError::at(&node.id, Some(&input.port), "this audio input takes one connection"));
            }
        }
    }
    let mut layout = Layout {
        plan_notes: Vec::new(),
        counters: base.map(|layout| layout.counters.clone()).unwrap_or_default(),
        generation, rate, latency: 0, nested: Vec::with_capacity(factories.len()), delays: HashMap::new(), nodes: Vec::new(), song_time_delays: HashMap::new(), shaders: HashMap::new(), controls: HashMap::new(), probes: HashMap::new(), audio_probes: HashMap::new(), probe_taps: HashMap::new(),
        triggers: HashMap::new(), audio_edges: Vec::new(), drivers: HashMap::new(), transports: HashMap::new(), decks: HashMap::new(),
    };
    let mut shaders = HashMap::new();
    let mut nested = HashMap::with_capacity(factories.len());
    let mut nodes = Vec::with_capacity(slice.nodes.len());
    let mut edges = Vec::new();
    let mut ports = Vec::new();
    let mut outputs = Vec::new();
    for (to, node) in slice.nodes.iter().enumerate() {
        check_control()?;
        let kind = node.type_name.as_str();
        let shader_kind = match kind {
            "ShaderFx" => Some(makepad_score_player::shader::Kind::Effect),
            "ShaderControl" => Some(makepad_score_player::shader::Kind::Control),
            "Loop" if text(node, "inst")? == "shader" => Some(makepad_score_player::shader::Kind::Instrument),
            _ => None,
        };
        let latency = if let Some(kind) = shader_kind {
            let prepared = shader_of(node, kind, base, &mut layout, bindings);
            check_control()?;
            let prepared = prepared?;
            let latency = prepared.0.latency();
            shaders.insert(node.id.as_str(), prepared);
            latency
        } else if let Some(factory) = factories.get(node.id.as_str()) {
            let player = factory(node, rate, generation, base)?;
            let cell = player.latency();
            let latency = cell.load(Ordering::Acquire);
            layout.nested.push((cell, latency));
            nested.insert(node.id.as_str(), player);
            latency
        } else {
            makepad_score_player::FxKind::parse(kind).filter(|fx| fx.name() == kind).map_or(0, |fx| fx.latency())
        };
        nodes.push(AlignNode { latency, source: kind == "Loop" });
        if matches!(kind, "AudioOutput" | "CueOutput") {
            outputs.push(to);
        }
        for input in node.inputs.iter().filter(|input| input.ty.as_str() == AUDIO && input.control.is_none()) {
            for source in &input.sources {
                let from = qualified(&source.from.node);
                let from = *ids.get(from.as_str()).ok_or_else(|| PlanError::at(&node.id, Some(&input.port), format!("unknown audio source `{from}.{}`", source.from.port)))?;
                edges.push(AlignEdge { from, to, deliberate: 0 });
                ports.push((input.port.as_str(), source));
            }
        }
    }
    // Discover the wired outputs before assigning arena slots. Multiple consumers
    // of the same output share a span. All the audio edges above take part in alignment.
    let mut wired_outputs = HashSet::new();
    let mut wired_sides = 0usize;
    let mut declared = HashMap::new();
    for (&id, (shader, _)) in &shaders {
        for i in 0..shader.port_count() {
            let port = shader.port_view(i);
            if shader.kind() != makepad_score_player::shader::Kind::Instrument && crate::nodes::reserved_shader_port(port.name) {
                return Err(PlanError::at(id, Some(port.name), "rename the port: its name is already a ShaderFx field"));
            }
            let name = if port.output && shader.kind() == makepad_score_player::shader::Kind::Instrument {
                crate::nodes::instrument_output(port.name)
            } else { port.name };
            declared.insert((id, name), port.output);
        }
    }
    for node in &slice.nodes {
        for input in node.inputs.iter().filter(|input| !input.sources.is_empty()) {
            let loop_endpoint = node.type_name == "Loop" && (input.port == "clock" || input.control.is_some());
            if node.type_name == "Loop" && !loop_endpoint && !shaders.contains_key(node.id.as_str()) {
                return Err(PlanError::at(&node.id, Some(&input.port), "a built-in instrument has no wired ports or params; wire a Shader instrument instead"));
            }
            if let Some((shader, _)) = shaders.get(node.id.as_str()).filter(|_| !loop_endpoint) {
                let name = input.port.strip_prefix(':').unwrap_or(&input.port);
                if let Some(&output) = declared.get(&(node.id.as_str(), name)) {
                    if output || input.ty.as_str() != AUDIO || input.control.is_some() {
                        return Err(PlanError::at(&node.id, Some(&input.port), "expected an audio input port"));
                    } else {
                        wired_sides += 1;
                    }
                } else if shader_param_wire(shader, &input.port).is_some() {
                    if input.ty.as_str() != AUDIO || input.control.is_some() {
                        return Err(PlanError::at(&node.id, Some(&input.port), "a shader param wire needs an audio span"));
                    }
                } else if node.type_name == "Loop" || node.type_name == "ShaderControl" || (node.type_name == "ShaderFx" && input.port != "audio") {
                    return Err(PlanError::at(&node.id, Some(&input.port), "unknown shader input port"));
                }
            }
            for source in &input.sources {
                let from = qualified(&source.from.node);
                let Some((shader, _)) = shaders.get(from.as_str()) else { continue };
                let port = &source.from.port;
                if port == "audio" || port == "peak" {
                    if shader.kind() == makepad_score_player::shader::Kind::Control {
                        return Err(PlanError::at(&node.id, Some(&input.port), format!("control node `{from}` has only named outputs; use Port{{node: {from} name: @out}}")));
                    }
                    continue;
                }
                match declared.get(&(from.as_str(), port.as_str())) {
                    Some(true) => {
                        if input.ty.as_str() != AUDIO || input.control.is_some() {
                            return Err(PlanError::at(&node.id, Some(&input.port), "a named audio output needs an audio input"));
                        }
                        wired_outputs.insert((from, port.clone()));
                    }
                    _ => return Err(PlanError::at(&from, Some(port), "unknown shader output port")),
                }
            }
        }
    }
    audio_spans += wired_outputs.len();
    let alignment = align(&nodes, &edges, &outputs)
        .map_err(|error| {
            if matches!(error, makepad_score_player::shader::AlignError::Cycle(_)) {
                let mut reads = vec![Vec::new(); nodes.len()];
                for e in &edges { reads[e.to].push(e.from); }
                if let Some(cycle) = wire_cycle(&reads) {
                    let names = cycle.iter().map(|&n| slice.nodes[n].id.as_str()).collect::<Vec<_>>().join(" -> ");
                    return PlanError::general(format!("audio wires make a cycle: {names}"));
                }
            }
            PlanError::general(format!("cannot align the audio graph: {error:?}"))
        })?;
    // A graph driven by a host clock can export its audio directly from
    // the arena. Without a device sink, its longest audio path sets its total.
    layout.latency = if outputs.is_empty() && slice.nodes.iter().any(|n| n.type_name == CLOCK_IN) {
        alignment.nodes.iter().map(|n| n.l_out).max().unwrap_or(0)
    } else {
        alignment.total
    };
    let mut output_pad = vec![0; nodes.len()];
    for (&output, &pad) in outputs.iter().zip(&alignment.pads) {
        output_pad[output] = pad;
    }
    let mut incoming = vec![Vec::new(); nodes.len()];
    let mut distinct = HashSet::new();
    let compatible_base = base.filter(|base| base.rate == rate);
    // Probe by borrowed identity, including edges which now need a direct tap.
    let old_delays: HashMap<_, _> = compatible_base.into_iter().flat_map(|base| &base.delays)
        .map(|(key, delay)| ((key.consumer.as_str(), key.port.as_str(), &key.source), delay)).collect();
    let mut ring_bytes = 0usize;
    let too_large = || PlanError::general("the delay lines exceed the addressable memory");
    for (node, timing) in slice.nodes.iter().zip(&alignment.nodes) {
        if !matches!(node.type_name.as_str(), "ShaderFx" | "ShaderControl") { continue; }
        let old = compatible_base.and_then(|b| b.song_time_delays.get(&node.id));
        let delay = SongTimeDelay::new(timing.l_in, old);
        let bytes = delay.bytes().ok_or_else(too_large)?;
        if bytes != 0 {
            ring_bytes = ring_bytes.checked_add(bytes).ok_or_else(too_large)?;
            layout.song_time_delays.insert(node.id.clone(), delay);
        }
    }
    let mut late = std::collections::BTreeMap::new();
    for ((edge, timing), (port, wire)) in edges.iter().zip(&alignment.edges).zip(ports) {
        if !timing.aligned && timing.lateness > 0 {
            let frames = late.entry(label(&slice.nodes[edge.to], port, wire)).or_insert(0u64);
            *frames = (*frames).max(timing.lateness);
        }
        let source = &wire.from;
        // Repeated mixer entries keep their multiplicity but can read the same tap.
        let frames = timing.delay + output_pad[edge.to];
        let consumer = slice.nodes[edge.to].id.as_str();
        let old = old_delays.get(&(consumer, port, source));
        let still_fading = old.and_then(|old| old.fade_pending.as_ref()).is_some_and(|pending| pending.load(Ordering::Acquire));
        if frames == 0 && !still_fading && !old.is_some_and(|old| old.frames != 0) {
            continue;
        }
        if !distinct.insert((edge.to, port, source)) { continue; }
        let frames = usize::try_from(frames).map_err(|_| too_large())?;
        let capacity = if frames == 0 { 0 } else {
            frames.checked_add(CHUNK).and_then(usize::checked_next_power_of_two).ok_or_else(too_large)?
        };
        let key = DelayKey { consumer: consumer.to_owned(), port: port.to_owned(), source: source.clone() };
        let previous_capacity = old.map_or(0, |old| {
            let older = if still_fading { old.previous_capacity } else { 0 };
            // A changed tap can inherit either ring if prepare races warmup.
            if old.frames == frames { older } else { old.capacity.max(older) }
        });
        let bytes = capacity.checked_add(previous_capacity).and_then(|n| n.checked_mul(std::mem::size_of::<[f32; 2]>())).ok_or_else(too_large)?;
        ring_bytes = ring_bytes.checked_add(bytes).and_then(|n| n.checked_add(std::mem::size_of::<DelayState>())).ok_or_else(too_large)?;
        incoming[edge.to].push((port, key, frames, capacity, previous_capacity));
    }
    layout.plan_notes = late.into_iter().map(|(edge, frames)| {
        let NoteEdge { source, node, port, .. } = edge;
        let message = format!("{source} -> {node}.{port} arrives {frames} frames late");
        PlanNote { node, port, source, frames, message }
    }).collect();
    let state_count = slice.nodes.len() + distinct.len();
    audio_spans += distinct.len();
    // Controls, probes and spans are indexed by u16 in the realtime ops.
    let indices = u16::MAX as usize;
    if control_count > indices || probes > indices || audio_spans > indices || state_count > indices {
        return Err(PlanError::general(format!("the audio graph has more than {indices} controls, probes or nodes")));
    }
    // Upper bound includes the exact variable delay/chorus lengths, the
    // fixed reverb, inline DSP state, arenas and conservative metadata room.
    // A loop's voice pool and queue, or an FX's delay lines and rooms, at
    // most: the four-second delay's two lines are the largest.
    let heavy_bytes = 2 * (4 * rate as usize + 8) * 4 + 262_144;
    let bytes = heavy * heavy_bytes + audio_spans * SPAN * 8 + slice.nodes.len() * 4096 + 16_384
        + control_count * (std::mem::size_of::<Route>() + 2 * std::mem::size_of::<AtomicU32>());
    // The machine is the limit: a schedule that cannot get its memory says
    // so here instead of aborting halfway through building its nodes.
    let wired_bytes = wired_sides.checked_mul(3)
        .and_then(|planes| planes.checked_add(wired_outputs.len() * 2))
        .and_then(|planes| planes.checked_mul(CHUNK * std::mem::size_of::<f32>()))
        .ok_or_else(|| PlanError::general("the shader port buffers exceed addressable memory"))?;
    let bytes = bytes.checked_add(wired_bytes).and_then(|b| b.checked_add(ring_bytes)).ok_or_else(too_large)?;
    if Vec::<u8>::new().try_reserve_exact(bytes).is_err() {
        return Err(PlanError::general(format!("not enough memory to prepare the audio graph ({} MiB)", bytes >> 20)));
    }
    let permit = budget.reserve(bytes);
    let mut ops = Vec::with_capacity(state_count);
    let mut deck_states: Vec<u16> = Vec::new();
    let mut states: Vec<NodeState<E::State>> = Vec::with_capacity(state_count);
    let mut bank: Vec<f32> = Vec::new();
    // One slot of its kind per output port, in node order: an output is
    // always allocated after every input it could read.
    let mut spans = AudioSpans::default();
    let mut clocks: HashMap<(String, String), u16> = HashMap::new();
    let mut probe_count = 0u16;
    let mut sinks = Vec::new();

    let mut routes: Vec<Route> = Vec::with_capacity(control_count);
    for (index, node) in slice.nodes.iter().enumerate() {
        let mut delayed = HashMap::new();
        for (port, key, frames, capacity, previous_capacity) in incoming[index].drain(..) {
            let input = *spans.get(&(qualified(&key.source.node), key.source.port.clone()))
                .ok_or_else(|| PlanError::at(&node.id, Some(port), format!("no audio span for `{}.{}`", qualified(&key.source.node), key.source.port)))?;
            let out = spans.private_span();
            let state = states.len() as u16;
            let fade_pending = (previous_capacity != 0 || frames == 0).then(|| Arc::new(AtomicBool::new(true)));
            states.push(NodeState::Delay(Box::new(DelayState {
                current: DelayTap { ring: vec![[0.0; 2]; capacity].into_boxed_slice(), write: 0, frames },
                previous: DelayTap::default(), warmup: 0, fade_left: 0,
                fade_pending: fade_pending.clone(), report_completion: fade_pending.is_some(),
            })));
            ops.push(Op::Delay { state, input, out, frames });
            delayed.insert((port, input), out);
            layout.delays.insert(key, LayoutDelay { frames, capacity, previous_capacity, fade_pending, state });
        }
        let aligned = |port: &str, span: u16| delayed.get(&(port, span)).copied().unwrap_or(span);
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
                // A score effect reads these spans on the block grid. Flow's
                // scalar endpoints below retain their existing per-pass routes.
                if input.control.is_none() && input.ty.as_str() == AUDIO &&
                    makepad_score_player::FxKind::parse(&node.type_name).is_some_and(|fx| fx.name() == node.type_name) {
                    let drivers = input.sources.iter().map(|s| format!("{}.{}", qualified(&s.from.node), s.from.port)).collect::<Vec<_>>();
                    layout.drivers.insert(format!("{}.{name}", node.id), drivers.join(", "));
                    return Ok(index);
                }
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
            let span = *table.get(&(qualified(&source.from.node), source.from.port.clone()))
                .ok_or_else(|| PlanError::at(&node.id, Some(port), format!("no audio span for `{}.{}`", qualified(&source.from.node), source.from.port)))?;
            Ok(Some(if key == AUDIO { aligned(port, span) } else { span }))
        };
        let audio_inputs = |port: &str, spans: &AudioSpans| -> Result<Box<[u16]>, PlanError> {
            node.input(port).into_iter().flat_map(|input| &input.sources).map(|source| {
                spans.get(&(qualified(&source.from.node), source.from.port.clone()))
                    .map(|&span| aligned(port, span))
                    .ok_or_else(|| PlanError::at(&node.id, Some(port), format!("no audio span for `{}.{}`", qualified(&source.from.node), source.from.port)))
            }).collect()
        };
        let shader_ports = |shader: &makepad_score_player::shader::AudioShader, spans: &mut AudioSpans| -> Result<ShaderPorts, PlanError> {
            let mut wires = Vec::new();
            for input in node.inputs.iter().filter(|input| input.ty.as_str() == AUDIO && input.control.is_none() && !input.sources.is_empty()) {
                if let Some(index) = shader_param_wire(shader, &input.port) {
                    wires.push((index, audio_inputs(&input.port, spans)?));
                }
            }
            let mut sides = Vec::new();
            let mut outs = Vec::new();
            for i in 0..shader.port_count() {
                let port = shader.port_view(i);
                if port.output {
                    let name = if node.type_name == "Loop" { crate::nodes::instrument_output(port.name) } else { port.name };
                    let key = (node.id.clone(), name.to_owned());
                    if wired_outputs.contains(&key) {
                        let span = spans.len() as u16;
                        spans.insert(key, span);
                        outs.push((i, span));
                    }
                } else {
                    let name = if node.type_name == "Loop" { crate::nodes::instrument_input(port.name) } else { port.name };
                    if node.input(name).is_some_and(|input| !input.sources.is_empty()) {
                        sides.push((i, audio_inputs(name, spans)?));
                    }
                }
            }
            Ok(ShaderPorts { sides, outs, params: wires })
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
                let beats_per_bar = control(&mut layout, "beats_per_bar")?;
                let mapped = probe(&mut layout, "mapped");
                let probe = probe(&mut layout, "clock");
                states.push(NodeState::Transport(Box::new(TransportState {
                    beat: 0.0, anchor: 0.0, anchor_frame: 0.0, frames: 0, bpm: f32::NAN, playing: false, tempo: 120.0,
                    // A transport that carries its state over keeps its feed.
                    feed: (!base.filter(|base| base.rate == rate).is_some_and(|base| base.nodes.iter().any(|old| old.id == node.id && old.type_name == "Transport")))
                        .then(|| crate::feed::tempo_feed(&node.id).fresh()),
                    pending: Vec::with_capacity(crate::feed::TEMPO_EVENTS), ramp: None, mapped: false, located: 0, shader_anchor: (0.0, 0),
                })));
                ops.push(Op::Transport { state, clock, bpm, playing, beats_per_bar, probe, mapped });
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
                states.push(NodeState::Filter { left: Svf::default(), right: Svf::default(), cutoff: bank[cutoff as usize], q: bank[resonance as usize], coeffs: makepad_audio_dsp::svf::SvfCoeffs::new(bank[cutoff as usize], bank[resonance as usize], rate as f32) });
                ops.push(Op::Filter { state, input, out, cutoff, resonance });
            }
            "Gain" => {
                let input = source("audio", AUDIO, &spans)?;
                let out = spans.len() as u16;
                spans.insert((node.id.clone(), "audio".into()), out);
                let (gain, pan) = (control(&mut layout, "gain")?, control(&mut layout, "pan")?);
                let mute = control(&mut layout, "mute")?;
                let probe = probe(&mut layout, "peak");
                states.push(NodeState::Gain { left: GainRamp::new(0.0), right: GainRamp::new(0.0), peak: 0.0 });
                ops.push(Op::Gain { state, input, out, gain, pan, mute, probe });
            }
            "AudioMixer" => {
                let inputs = audio_inputs("inputs", &spans)?;
                let out = spans.len() as u16;
                spans.insert((node.id.clone(), "audio".into()), out);
                let master = control(&mut layout, "master")?;
                states.push(NodeState::Mixer { master: GainRamp::new(bank[master as usize]) });
                ops.push(Op::Mixer { state, inputs, out, master });
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
                states.push(NodeState::StemMixer { gains: [GainRamp::new(0.0); 4] });
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
                let inst = loop_inst(node, shaders.remove(node.id.as_str()))?;
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
                    #[cfg(all(feature = "neural", not(target_arch = "wasm32")))]
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
                let mut engine = makepad_score_player::loop_engine::LoopEngine::for_inst(crate::feed::key_of(&node.id), &inst, rate as f32, carries)
                    .map_err(|error| PlanError::at(&node.id, Some("inst"), error))?;
                let mut ports = ShaderPorts::default();
                if let Some(inst) = engine.shader_mut() {
                    ports = shader_ports(inst.shader(), &mut spans)?;
                    inst.prepare_param_wires(ports.params.iter().map(|v| v.0));
                    inst.prepare_wires(ports.sides.iter().map(|v| v.0), ports.outs.iter().map(|v| v.0));
                    if let Some(tap) = layout.probe_tap(&node.id, inst.shader(), makepad_score_player::shader::SHADER_PROBE_VOICES, budget) {
                        inst.attach_probes(tap).map_err(|error| PlanError::at(&node.id, Some("inst"), error))?;
                    }
                }
                let midi_live = matches!(inst, makepad_score_player::Inst::Midi { .. }) && crate::feed::midi_out::is_live(&node.id);
                let input = matches!(inst, makepad_score_player::Inst::Input { .. }).then(crate::feed::audio_in::fresh);
                // A recipe synth's or a shader's settings can be held live.
                let live = matches!(inst, makepad_score_player::Inst::Synth { .. } | makepad_score_player::Inst::Shader { .. }).then(|| crate::feed::live_params(&node.id));
                // Played from MIDI in: a ring of the device's messages.
                let stream = match node.param("stream") {
                    Some(Literal::Arr(words)) => {
                        let words: Vec<f64> = words.iter().map(|w| match w {
                            Literal::Num(x) => *x,
                            _ => 0.0,
                        }).collect();
                        makepad_score_player::stream::StreamDef::decode(&words).map(|def| {
                            let device = text(node, "midi_device").unwrap_or("");
                            let channel = number(node, "midi_channel").unwrap_or(0.0).clamp(0.0, 16.0) as u8;
                            let latency = number(node, "midi_latency").unwrap_or(8.0) / 1000.0;
                            // Only a graph that plays live takes the keys
                            // (the live score's; never a render or a preview).
                            let midi = crate::feed::midi_out::is_live(&node.id).then(|| crate::feed::midi_in::fresh(&node.id, device, channel));
                            Box::new(crate::midi_stream::StreamRun::new(def, midi, latency, Some(crate::feed::streams::view(&node.id))))
                        })
                    }
                    _ => None,
                };
                states.push(NodeState::Loop(Box::new(LoopState { engine, feed, gain_l: GainRamp::new(0.0), gain_r: GainRamp::new(0.0), peak: 0.0, playing: false, inst_class, midi_live, input, live, live_seen: None, stream })));
                ops.push(Op::Loop { state, clock, out, amp, pan, mute, solo, probe, inst, ports });
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
                    serial: 0, gain: GainRamp::new(0.0), peak: 0.0,
                })));
                ops.push(Op::Pads { state, out, amp, mute, probe });
            }
            kind if makepad_score_player::FxKind::parse(kind).is_some_and(|fx| fx.name() == kind) => {
                let fx = makepad_score_player::FxKind::parse(kind).unwrap_or(makepad_score_player::FxKind::Bus);
                let inputs = audio_inputs("audio", &spans)?;
                let key = source("key", AUDIO, &spans)?;
                let out = spans.len() as u16;
                spans.insert((node.id.clone(), "audio".into()), out);
                let mut params = [0u16; makepad_score_player::fx::MAX_FX_PARAMS];
                let mut values = fx.defaults();
                let mut wired = [false; makepad_score_player::fx::MAX_FX_PARAMS];
                let mut wires = Vec::new();
                for (i, &(name, ..)) in fx.params().iter().enumerate() {
                    params[i] = control(&mut layout, name)?;
                    if node.input(name).is_some_and(|input| input.control.is_none() && input.ty.as_str() == AUDIO && !input.sources.is_empty()) {
                        wires.push((i, audio_inputs(name, &spans)?));
                        wired[i] = true;
                        values[i] = number(node, name)? as f32;
                    }
                }
                // A Comp's gain reduction (dB, ≤ 0) for its meter.
                let gr = probe(&mut layout, "gr");
                let probe = probe(&mut layout, "peak");
                sounding = true;
                // An FX that carries its state over keeps its automation feed.
                let carries = base.filter(|base| base.rate == rate).is_some_and(|base| base.nodes.iter().any(|old| old.id == node.id && old.type_name == node.type_name));
                states.push(NodeState::Fx(Box::new(FxNodeState {
                    fx: makepad_score_player::fx::FxState::new(fx, rate as f32), peak: 0.0, values, wired, automated: [false; makepad_score_player::fx::MAX_FX_PARAMS],
                    key_peak: None, previous_key_level: 0.0,
                    feed: (!carries).then(|| crate::feed::automation_feed(&node.id).fresh()),
                    waiting: Vec::with_capacity(crate::feed::AUTOMATION_EVENTS), ramps: [None; makepad_score_player::fx::MAX_FX_PARAMS],
                })));
                ops.push(Op::Fx { state, kind: fx, inputs, key, out, params, wires: wires.into_boxed_slice(), count: fx.params().len() as u8, probe, gr });
            }
            "ShaderFx" | "ShaderControl" => {
                let control_node = node.type_name == "ShaderControl";
                let inputs = audio_inputs("audio", &spans)?;
                // The split between source and output spans. A control's first
                // named output starts here; it has no main stereo span.
                let out = spans.len() as u16;
                if !control_node { spans.insert((node.id.clone(), "audio".into()), out); }
                let probe = (!control_node).then(|| probe(&mut layout, "peak"));
                sounding = !control_node;
                let (shader, params) = shaders.remove(node.id.as_str()).expect("shader prepared in the alignment pass");
                let ShaderPorts { sides, outs, params: wires } = shader_ports(&shader, &mut spans)?;
                let mut fx = makepad_score_player::shader::ShaderFx::try_new(shader, rate as f32)
                    .map_err(|error| PlanError::at(&node.id, Some("shader"), error))?;
                fx.prepare_param_wires(wires.iter().map(|v| v.0));
                fx.prepare_wires(sides.iter().map(|v| v.0), outs.iter().map(|v| v.0));
                if let Some(delay) = layout.song_time_delays.get(&node.id) { fx.prepare_song_time(delay); }
                fx.set_seed(shader_seed(node, crate::feed::key_of(&node.id))?);
                if let Some(tap) = layout.probe_tap(&node.id, fx.shader(), 1, budget) {
                    fx.attach_probes(tap).map_err(|error| PlanError::at(&node.id, None, error))?;
                }
                for (index, value) in params.iter() {
                    fx.set_param(*index as usize, *value);
                }
                let applied = std::sync::Arc::as_ptr(&params) as *const u8 as usize;
                states.push(NodeState::ShaderFx(Box::new(ShaderFxState { fx, input: vec![0.0; 2 * CHUNK].into_boxed_slice(), params: applied, peak: 0.0 })));
                ops.push(Op::ShaderFx { state, inputs, wires: wires.into_boxed_slice(), sides: sides.into_boxed_slice(), outs: outs.into_boxed_slice(), out, params, probe });
            }
            "AudioOutput" | "CueOutput" => {
                let cue = node.type_name == "CueOutput";
                let input = source("audio", AUDIO, &spans)?;
                let out = spans.len() as u16;
                spans.insert((node.id.clone(), "waveform".into()), out);
                sinks.push(out);
                let enabled = control(&mut layout, "enabled")?;
                let probes = (probe(&mut layout, "peak_l"), probe(&mut layout, "peak_r"));
                let hold = alignment.nodes[index].l_in + output_pad[index];
                states.push(NodeState::Output { peak_l: 0.0, peak_r: 0.0, gate: 0.0, hold });
                ops.push(Op::Output { state, input, out, enabled, probes, cue });
            }
            other => {
                if let Some(player) = nested.remove(node.id.as_str()) {
                    let clock = source("clock", CLOCK, &clocks)?;
                    clock_source = node.input("clock").and_then(|input| input.sources.first()).map(|source| format!("{}.{}", qualified(&source.from.node), source.from.port));
                    let out = spans.len() as u16;
                    spans.insert((node.id.clone(), "audio".into()), out);
                    let probe = probe(&mut layout, "peak");
                    sounding = true;
                    states.push(NodeState::Nested(player));
                    ops.push(Op::Nested { state, clock, out, probe });
                } else {
                    let mut cx = ExtCx {
                        node, state, rate, generation, base, layout: &mut layout, spans: &mut spans, observed: &mut deck_states,
                        sounding: &mut sounding, control: &mut control, probe: &mut probe,
                    };
                    let (ext_state, ext_op) = E::compile(&mut cx)?
                        .ok_or_else(|| PlanError::at(&node.id, None, format!("the audio owner has no node type `{other}`")))?;
                    states.push(NodeState::Ext(ext_state));
                    ops.push(Op::Ext(ext_op));
                }
            }
        }
        layout.nodes.push(LayoutNode {
            id: node.id.clone(), type_name: node.type_name.clone(), state, clock_source, inst_class, sounding,
            own_latency: nodes[index].latency, l_in: alignment.nodes[index].l_in,
        });
    }
    for ((node, port), index) in &spans.named {
        layout.audio_probes.insert(format!("{node}.{port}"), *index);
    }

    // State transfer is only defined against the exact base this was
    // prepared for, at the same device rate.
    let mut migrate = Vec::new();
    let mut fade_from_direct = Vec::new();
    let mut migrate_decks = Vec::new();
    let mut dip = false;
    if let Some(base) = base {
        let compatible = base.rate == rate;
        if compatible {
            // The exact edge carries its tap and write position across the swap.
            let mut base_edges = None;
            for (key, delay) in &layout.delays {
                if let Some(old) = base.delays.get(key) {
                    migrate.push((old.state, delay.state));
                } else {
                    let edges = base_edges.get_or_insert_with(|| base.audio_edges.iter()
                        .map(|(from, to, _)| (from.as_str(), to.as_str())).collect::<HashSet<_>>());
                    let from = format!("{}.{}", qualified(&key.source.node), key.source.port);
                    let to = format!("{}.{}", key.consumer, key.port);
                    if edges.contains(&(from.as_str(), to.as_str())) { fade_from_direct.push(delay.state); }
                }
            }
        }
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

    let echoes = ops.iter().any(|op| matches!(op, Op::Fx { kind: makepad_score_player::FxKind::Delay, .. } | Op::ShaderFx { probe: Some(_), .. }));
    let seconds = if echoes { crate::rest::ECHO_HOLD_SECONDS } else { crate::rest::HOLD_SECONDS };
    let rest_hold = ((seconds * rate as f64) as u64).saturating_add(layout.latency);
    let transport_ops = ops.iter().enumerate().filter_map(|(i, op)| matches!(op, Op::Transport { .. }).then_some(i)).collect();
    let clock_inputs = clocks.iter().filter_map(|((node, _), &slot)| {
        (!layout.transports.contains_key(node)).then_some((slot, ClockChunk::default()))
    }).collect();
    let outputs = ops.iter().filter_map(|op| match op {
        Op::Output { out, cue, .. } => Some((*out, *cue, false)),
        _ => None,
    }).collect();
    let schedule = Box::new(Schedule {
        latency: layout.latency,
        generation,
        frame: 0,
        incarnation,
        format,
        base: base.map(|base| base.generation),
        rate,
        dip,
        ops: ops.into_boxed_slice(),
        states: states.into_boxed_slice(),
        arena: vec![0.0; spans.len() * SPAN].into_boxed_slice(),
        assembled: vec![0.0; spans.len() * SPAN].into_boxed_slice(),
        transport_ops, clock_inputs, outputs, rest_hold, clock_frames: 0,
        last_frames: 0,
        sinks: sinks.into_boxed_slice(),
        clocks: vec![ClockChunk::default(); clocks.len()].into_boxed_slice(),
        probes: vec![0.0; probe_count as usize].into_boxed_slice(),
        controls: Arc::new(ControlBank::new(&bank)),
        routes: routes.into_boxed_slice(),
        migrate: migrate.into_boxed_slice(),
        fade_from_direct: fade_from_direct.into_boxed_slice(),
        migrate_decks: migrate_decks.into_boxed_slice(),
        triggers: triggers.into_boxed_slice(),
        decks: deck_states.into_boxed_slice(),
        board: B::default(),
        rest: crate::rest::Rest::new(bank.len()),
        _permit: permit,
    });
    check_control()?;
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

/// Combine the prepared stream selector with this node's stable name hash.
fn shader_seed(node: &LiveNode, key: u64) -> Result<u32, PlanError> {
    let seed = if node.param("seed").is_some() { number(node, "seed")? } else { 0.0 };
    if seed.fract() != 0.0 || !(0.0..=f64::from(u32::MAX)).contains(&seed) {
        return Err(PlanError::at(&node.id, Some("seed"), "a shader seed is a 32-bit unsigned integer"));
    }
    Ok(makepad_score_player::shader::mix_seed(seed as u32, (key ^ (key >> 32)) as u32))
}

/// A `Loop` node's instrument, built from its authored settings.
fn loop_inst(node: &LiveNode, prepared: Option<ShaderSettings>) -> Result<makepad_score_player::Inst, PlanError> {
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
            let (shader, params) = prepared.expect("instrument prepared in the alignment pass");
            use makepad_score_player::{loop_engine::ShaderMode, shader::Steal};
            let mono = match node.param("mono") {
                None | Some(Literal::Num(-1.0)) => shader.mono(),
                Some(Literal::Num(n)) if n.is_finite() => *n >= 0.5,
                _ => return Err(PlanError::at(&node.id, Some("mono"), "mono takes 0 or 1")),
            };
            let legato = match node.param("legato") {
                None | Some(Literal::Num(1.0)) => true,
                Some(Literal::Num(0.0)) => false,
                _ => return Err(PlanError::at(&node.id, Some("legato"), "legato takes 0 or 1")),
            };
            let glide = if node.param("glide").is_some() { number(node, "glide")? as f32 } else { 0.0 };
            if !glide.is_finite() || glide < 0.0 {
                return Err(PlanError::at(&node.id, Some("glide"), "glide is a nonnegative number of seconds"));
            }
            let steal = match node.param("steal") {
                None => Steal::Oldest,
                Some(_) => Steal::from_name(text(node, "steal")?).ok_or_else(|| PlanError::at(&node.id, Some("steal"), "steal takes oldest or quietest"))?,
            };
            let mode = ShaderMode { mono, legato, glide, steal };
            Inst::Shader { key, seed: shader_seed(node, key)?, shader, params, mode }
        }
        // The neural voice: its singer is loaded here (off the audio thread)
        // from the installed voice; none installed is silence, never an error
        // (a browser build has no singer: the voice sings as a choir voice). A
        // native build without it (its app's `localai`) says so.
        #[cfg(all(feature = "neural", not(target_arch = "wasm32")))]
        "voice" if text(node, "engine").is_ok_and(|e| e == "neural") => match makepad_score_player::sung::neural::shared() {
            Some(singer) => Inst::Neural { key, singer },
            None => Inst::Silent,
        },
        #[cfg(all(not(feature = "neural"), not(target_arch = "wasm32")))]
        "voice" if text(node, "engine").is_ok_and(|e| e == "neural") => return Err(PlanError::at(&node.id, Some("engine"), makepad_score_player::NEURAL_NEEDS_LOCALAI)),
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

type ShaderSettings = (AudioShaderRef, Arc<[(u16, f32)]>);

/// Applies the node's authored rate to the same compiler bindings used for
/// score validation and schedule preparation. An empty list follows the shader.
pub fn with_shader_oversample<'a>(mut bindings: AudioBindings<'a>, values: &[Literal], kind: makepad_score_player::shader::Kind) -> Result<AudioBindings<'a>, &'static str> {
    let Some((factor, modes)) = values.split_first() else { return Ok(bindings) };
    let Literal::Num(factor) = factor else { return Err("the oversample factor must be 1, 2, 4 or 8") };
    if ![1.0, 2.0, 4.0, 8.0].contains(factor) { return Err("the oversample factor must be 1, 2, 4 or 8"); }
    let mode = |value: &Literal| match value {
        Literal::Id(s) | Literal::Str(s) => s.parse(),
        _ => Err("a conversion mode must be filter, linear or hold"),
    };
    let (up, down) = match modes {
        [] => Default::default(),
        [both] => { let both = mode(both)?; (both, both) }
        [_, _] if kind == makepad_score_player::shader::Kind::Instrument => return Err("an instrument converts down only, use one oversample mode"),
        [up, down] => (mode(up)?, mode(down)?),
        _ => return Err("oversample takes at most two conversion modes"),
    };
    bindings.oversample.factor = *factor as u32;
    bindings.oversample.up = up;
    bindings.oversample.down = down;
    Ok(bindings)
}

fn shader_param_wire(shader: &makepad_score_player::shader::AudioShader, port: &str) -> Option<usize> {
    if port == "audio" { return None; }
    let name = port.strip_prefix(':').unwrap_or(port);
    makepad_score_player::shader::param_index(shader, name)
}

/// A shader node's program (compiled here, off the audio thread) and its
/// settings as (param index, value).
fn shader_of(node: &LiveNode, kind: makepad_score_player::shader::Kind, base: Option<&Layout>, layout: &mut Layout, bindings: AudioBindings<'_>) -> Result<ShaderSettings, PlanError> {
    let code = text(node, "shader")?;
    let mut loaded = Vec::new();
    if let Some(Literal::Arr(items)) = node.param("shader_samples") {
        for pair in items.chunks(2) {
            let [Literal::Str(name), Literal::Str(asset)] = pair else {
                return Err(PlanError::at(&node.id, Some("shader_samples"), "a shader table needs a name and sample"));
            };
            if let Some(sample) = crate::feed::samples::get(asset) {
                loaded.push((name.as_str(), sample));
            }
        }
    }
    let oversample = match node.param("oversample") {
        None => &[][..],
        Some(Literal::Arr(items)) => items.as_slice(),
        _ => return Err(PlanError::at(&node.id, Some("oversample"), "expected a factor and optional conversion modes in a list")),
    };
    let modules = node.param("shader_modules");
    let previous = base.filter(|b| b.rate == layout.rate).and_then(|b| b.shaders.get(&node.id)).filter(|p| {
        p.code == code && p.modules.as_ref() == modules && p.oversample == oversample && p.samples.len() == loaded.len() &&
            p.samples.iter().zip(&loaded).all(|((name, old), (table, sample))| name.as_str() == *table && old.as_ptr() == Arc::as_ptr(sample))
    });
    let program = match previous {
        Some(p) => p.clone(),
        None => {
            let bindings = with_shader_oversample(bindings, oversample, kind)
                .map_err(|e| PlanError::at(&node.id, Some("oversample"), e))?;
            Arc::new(PreparedShader {
                shader: compile_shader(node, code, &loaded, layout.rate, bindings)?,
                code: code.to_owned(),
                modules: modules.cloned(),
                oversample: oversample.to_vec(),
                samples: loaded.iter().map(|(name, sample)| ((*name).to_owned(), Arc::downgrade(sample))).collect(),
            })
        }
    };
    let shader = program.shader.clone();
    layout.shaders.insert(node.id.clone(), program);
    if shader.kind() != kind {
        let want = match kind { makepad_score_player::shader::Kind::Instrument => "an instrument (with voice())", makepad_score_player::shader::Kind::Effect => "an effect", makepad_score_player::shader::Kind::Control => "a control node" };
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

fn compile_shader(node: &LiveNode, code: &str, loaded: &[(&str, Arc<makepad_score_player::Sample>)], rate: u32, bindings: AudioBindings<'_>) -> Result<AudioShaderRef, PlanError> {
    use makepad_score_player::shader::{compile_with, Module, SampleView};
    use std::borrow::Cow;
    let mut modules = Vec::new();
    if let Some(Literal::Arr(items)) = node.param("shader_modules") {
        for pair in items.chunks(2) {
            let [Literal::Str(path), Literal::Str(source)] = pair else {
                return Err(PlanError::at(&node.id, Some("shader_modules"), "a shader module needs a path and source"));
            };
            modules.push(Module { path, source });
        }
    }
    let mut recordings = Vec::new();
    for (name, sample) in loaded {
        let error = |reason: &str| PlanError::at(&node.id, Some(name), format!("sample table `{name}` {reason}"));
        if !sample.rate.is_finite() || sample.rate <= 0.0 || sample.right.as_ref().is_some_and(|r| r.len() != sample.len()) {
            return Err(error("has an invalid sample rate or channel length"));
        }
        let ratio = f64::from(rate) / f64::from(sample.rate);
        if (sample.len() as f64 * ratio).ceil() > f64::from(u32::MAX) {
            return Err(error("exceeds the frame address range"));
        }
        let channel = |data: &'_ [f32]| makepad_audio_synth::kit::fxs::convolution::resample(data, ratio);
        let left = if rate as f32 == sample.rate { Cow::Borrowed(&sample.left[..]) } else { Cow::Owned(channel(&sample.left)) };
        let frames = left.len() as u32;
        let (channels, data) = if let Some(right) = &sample.right {
            let right = if rate as f32 == sample.rate { Cow::Borrowed(&right[..]) } else { Cow::Owned(channel(right)) };
            let mut data = Vec::new();
            let words = u64::from(frames) * 2;
            let size = usize::try_from(words).ok().and_then(|n| data.try_reserve_exact(n).ok());
            if size.is_none() {
                return Err(PlanError::at(&node.id, Some(name), format!("could not allocate {words} words for sample table `{name}`")));
            }
            for (&l, &r) in left.iter().zip(right.iter()) {
                data.extend_from_slice(&[l, r]);
            }
            (2, Cow::Owned(data))
        } else {
            (1, left)
        };
        recordings.push((*name, channels, frames, data));
    }
    let views: Vec<_> = recordings.iter().map(|(name, channels, frames, data)| SampleView { name, channels: *channels, frames: *frames, data }).collect();
    compile_with(code, &AudioBindings { samples: &views, modules: &modules, ..bindings }).map_err(|errors| {
        let first = errors.first().map(|e| e.message.clone()).unwrap_or_default();
        PlanError::at(&node.id, Some("shader"), format!("the shader does not compile: {first}"))
    })
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

#[test]
fn parallel_paths_keep_sample_time_across_passes_and_edits() {
    use crate::feed::{pad_feed, samples, PadHit};
    use crate::run::{rests, run, run_awake, NoExt};
    use makepad_flow::document::types::TypeKey;
    use makepad_flow::live::plan::{dnode, LiveInput, LiveSource};
    use makepad_score_player::Sample;

    const RATE: u64 = 8000;
    const PAD: &str = "alignment/pad";
    const PULSE: &str = "alignment/pulse";
    const SIGNAL: &str = "alignment/signal";
    const SHADER: &str = "
        latency(37)
        var left = [0.0; 64]
        var right = [0.0; 64]
        var w = int(0)
        fn effect(l, r) {
            let sample = vec2(left[w - 37], right[w - 37])
            left[w] = l
            right[w] = r
            w = w + 1
            -sample
        }";
    let signal = |frame: usize| ((frame % 31 + 1) as f32) / 32.0;
    let left: Vec<_> = (0..=2 * CHUNK).map(signal).collect();
    let right: Vec<_> = left.iter().map(|x| -0.5 * x).collect();
    samples::register(SIGNAL, Arc::new(Sample::new(left.into(), Some(right.into()), RATE as f32)));
    samples::register(PULSE, Arc::new(Sample::new([1.0, 0.0].into(), Some([-0.5, 0.0].into()), RATE as f32)));

    let node = |id: &str, kind: &str, settings: &[(&str, Literal)], port: &str, from: &[&str]| {
        let descriptor = crate::nodes::descriptor(kind).unwrap();
        let params = descriptor.params.iter().map(|p| {
            let value = settings.iter().find(|(name, _)| *name == p.name).map_or(&p.default, |(_, v)| v);
            (p.name.clone(), value.clone())
        }).collect();
        let inputs = if from.is_empty() { vec![] } else {
            vec![LiveInput {
                port: port.into(), ty: TypeKey::new(AUDIO), many: descriptor.inputs.iter().find(|p| p.name == port).unwrap().many,
                sources: from.iter().map(|&id| LiveSource {
                    from: DPort { node: dnode(id), port: "audio".into() },
                    conn: None, via: vec![], lineage: vec![],
                }).collect(), control: None,
            }]
        };
        LiveNode { id: id.into(), node: dnode(id), type_name: kind.into(), params, inputs, outputs: vec![] }
    };
    let mut slice = OwnerSlice { owner: crate::OWNER.into(), nodes: vec![
        node(PAD, "Pads", &[("samples", Literal::Arr(vec![Literal::Str(PULSE.into()), Literal::Str(SIGNAL.into())]))], "", &[]),
        node("invert", "ShaderFx", &[("shader", Literal::Str(SHADER.into()))], "audio", &[PAD]),
        node("mix", "AudioMixer", &[], "inputs", &[PAD, "invert"]),
        node("program", "AudioOutput", &[("enabled", Literal::Num(1.0))], "audio", &["mix"]),
        node("dry", "CueOutput", &[], "audio", &[PAD]),
        node("unused", "Gain", &[], "", &[]),
    ] };
    let budget = Arc::new(AudioBudget::default());
    let prepare = |slice: &OwnerSlice, generation, base: Option<&Layout>| compile::<NoExt, ()>(slice, generation, 1, RATE, base, &budget).unwrap();
    let cancelled = PrepareControl::default();
    cancelled.cancel();
    assert!(compile_controlled::<NoExt, ()>(&slice, 1, 1, RATE, None, &budget, Some(&cancelled)).is_err());
    assert_eq!(budget.prepared_bytes(), 0, "a cancelled graph publishes no prepared state");

    // A layout miss compiles at the device rate using the host prepare's counter.
    let mut rebuilding = slice.clone();
    let runaway = "let t=[0.0;2] fn init(){ for i in 0..1000000000 { for j in 0..1024 { t[0] += float(j) } } } fn effect(l,r){vec2(l+t[0],r)}";
    rebuilding.nodes[1] = node("invert", "ShaderFx", &[("shader", Literal::Str(runaway.into()))], "audio", &[PAD]);
    let rate_request = PrepareControl::default();
    let worker_control = rate_request.clone();
    let worker_budget = budget.clone();
    let (finished, result) = std::sync::mpsc::channel();
    let job = std::thread::spawn(move || {
        let compiled = compile_controlled::<NoExt, ()>(&rebuilding, 1, 1, RATE, None, &worker_budget, Some(&worker_control));
        finished.send(compiled.err()).unwrap();
    });
    let timeout = std::time::Duration::from_secs(5);
    let deadline = std::time::Instant::now() + timeout;
    while !rate_request.executing() && !job.is_finished() && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    let running = rate_request.executing();
    rate_request.cancel();
    let error = result.recv_timeout(timeout).expect("device-rate init stopped on its request's word");
    job.join().unwrap();
    assert!(running, "the device-rate prepare reported the running init");
    assert_eq!(error.unwrap().message, "prepare cancelled");
    assert!(!rate_request.executing(), "device-rate init released the execution guard");
    assert_eq!(budget.prepared_bytes(), 0, "a cancelled init leaves no prepared graph");
    let control = PrepareControl::default();
    let mut playing = compile_controlled::<NoExt, ()>(&slice, 1, 1, RATE, None, &budget, Some(&control)).unwrap();
    assert_eq!((playing.layout.latency, playing.schedule.latency), (37, 37), "layout and schedule must report the device latency");

    // Open the output gates and bring a silent graph to one frame before its usual rest.
    let quiet = (crate::rest::ECHO_HOLD_SECONDS * RATE as f64) as usize - 1;
    run(&mut playing.schedule, CHUNK, &mut |_, _, _| {}, &mut NoExt);
    // Exercise the same per-frame quiet count as the device.
    for at in (0..quiet).step_by(CHUNK) {
        let frames = CHUNK.min(quiet - at);
        run_awake(&mut playing.schedule, frames, &mut |_, _, _| {}, &mut NoExt, true);
        assert!(!playing.schedule.rest.asleep(), "rest before the ordinary silence hold expired");
    }

    let check = |schedule: &mut Schedule<(), ()>, frames, expected: &dyn Fn(usize) -> f32| {
        assert!(!rests(schedule, &NoExt), "rest cut off the delayed signal");
        let mut heard = [false; 2];
        run_awake(schedule, frames, &mut |cue, l, r| {
            heard[cue as usize] = true;
            for i in 0..frames {
                let left = if cue { expected(i) } else { 0.0 };
                assert_eq!((l[i], r[i]), (left, -0.5 * left), "stereo alignment at frame {i} of the pass, cue={cue}");
            }
        }, &mut NoExt, true);
        assert_eq!(heard, [true, true], "both the null and the padded dry output must reach the sink");
    };
    let feed = pad_feed(PAD);
    assert!(feed.send(PadHit { pad: 0, gain: 1.0, stop: false }), "the impulse must enter the graph");
    check(&mut playing.schedule, 37, &|_| 0.0);
    check(&mut playing.schedule, 97, &|i| if i == 0 { 1.0 } else { 0.0 });

    for pass in [CHUNK, 97] {
        assert!(feed.send(PadHit { pad: 1, gain: 1.0, stop: false }), "the signal must enter the graph");
        let end = 2 * CHUNK + 37 + 1;
        for start in (0..end).step_by(pass) {
            if start == pass {
                let unused = slice.nodes.last_mut().unwrap();
                unused.params.iter_mut().find(|(name, _)| name == "gain").unwrap().1 = Literal::Num(1.0 / pass as f64);
                let mut next = prepare(&slice, playing.schedule.generation + 1, Some(&playing.layout));
                next.schedule.take_over(&mut playing.schedule);
                playing = next;
            }
            check(&mut playing.schedule, pass.min(end - start), &|i| {
                match (start + i).checked_sub(37) {
                    Some(frame) if frame < 2 * CHUNK => signal(frame),
                    _ => 0.0,
                }
            });
        }
    }
    samples::forget(PULSE);
    samples::forget(SIGNAL);
    crate::feed::forget_instance("alignment");
}

#[test]
fn changed_delays_crossfade_to_the_new_tap_across_pass_sizes() {
    use crate::run::{run, NoExt};
    use makepad_flow::{document::{closure::Closure, Registry}, graph::FlowVm, live::plan::partition};
    use makepad_score_player::shader::FADE;

    const RATE: u64 = 48_000;
    const AMP: f32 = 0.25;
    const FREQ: f32 = 1500.0;
    const SECTION: usize = 1021; // Move the swap relative to the sine and the pass boundaries.
    const DELAYS: [usize; 6] = [37, 512, 37, 0, 37, 0];
    let bound = std::f32::consts::TAU * FREQ / RATE as f32 * AMP;
    // Each tap change is five samples apart modulo the sine's 32-frame
    // period. Moving the weight by 1/FADE can add their peak difference
    // divided by FADE to one step. Allow another 0.000002 for f32 arithmetic.
    let tap_difference = 2.0 * AMP * (std::f32::consts::PI * 5.0 / 32.0).sin();
    let margin = tap_difference / FADE as f32 + 0.000002;
    let mut registry = Registry::with_builtins();
    crate::nodes::register_nodes(&mut registry).unwrap();
    let source = r#"use mod.flow.*
        let sine = ShaderFx{shader: "var k = int(0)\nfn effect(l,r) { let a = TAU * float(k) / 32.0\nk = (k + 1) % 32\nvec2(sin(a), cos(a)) * 0.25 }"}
        let wet = ShaderFx{audio: [conn(1, sine.out(@audio))]}
        let mix = AudioMixer{master: 1 inputs: [conn(1, sine.out(@audio)), conn(2, wet.out(@audio))]}
        let out = AudioOutput{enabled: 1 audio: mix.out(@audio)}
        let dry = CueOutput{enabled: 1 audio: sine.out(@audio)}
        Flow{label: "Alignment" sine, wet, mix, out, dry}"#;
    let (_, graph) = FlowVm::load_with_registry(source, "alignment.splash", &Closure { modules: vec![] }, &registry).unwrap();
    let lowered = makepad_flow::engine::lower::lower(&graph).unwrap();
    let mut slice = partition(&lowered, &registry, &|owner| owner == crate::OWNER, "alignment", 1).unwrap().slices.remove(0);
    let wet = slice.nodes.iter().position(|n| n.id == "wet").unwrap();
    let budget = Arc::new(AudioBudget::default());
    let mut reference = Vec::new();
    for pass in [CHUNK, 97] {
        let mut playing: Option<Compiled<(), (), ()>> = None;
        let mut signal = Vec::new();
        let mut last = [[0.0f32; 2]; 2];
        let mut worst = [0.0f32; 2];
        for (revision, delay) in DELAYS.into_iter().enumerate() {
            // The history and cursor carry. The newly named tap makes the wet
            // shader crossfade its own latency edit using the normal shader swap.
            let code = format!("latency({delay})
                var tap_{delay} = int({delay})
                var h = [0.0; 1024]
                var k = [0.0; 1024]
                var w = int(0)
                fn effect(l,r) {{
                    h[w] = l
                    k[w] = r
                    let v = vec2(h[w - tap_{delay}], k[w - tap_{delay}])
                    w += 1
                    -v
                }}");
            slice.nodes[wet].params.iter_mut().find(|(name, _)| name == "shader").unwrap().1 = Literal::Str(code);
            let base = playing.as_ref().map(|p| &*p.layout);
            let mut next = compile::<NoExt, ()>(&slice, revision as u64 + 1, 1, RATE, base, &budget).unwrap();
            assert_eq!(next.schedule.latency, delay as u64, "reported latency after edit {revision}");
            if let Some(old) = &mut playing {
                assert!(!next.schedule.dip, "a latency edit alone must not dip the outputs");
                next.schedule.take_over(&mut old.schedule);
            }
            for at in (0..SECTION).step_by(pass) {
                // At 97 the fade to direct is still audible; at 256 it has
                // finished. An unrelated prepare must preserve both cases.
                if revision == 3 && at == pass {
                    let mut edit = compile::<NoExt, ()>(&slice, 100, 1, RATE, Some(&next.layout), &budget).unwrap();
                    edit.schedule.take_over(&mut next.schedule);
                    next = edit;
                }
                let frames = pass.min(SECTION - at);
                let mut output = [[[f32::NAN; 2]; CHUNK]; 2];
                run(&mut next.schedule, frames, &mut |cue, l, r| {
                    for i in 0..frames { output[cue as usize][i] = [l[i], r[i]]; }
                }, &mut NoExt);
                let source_span = next.layout.audio_probes["sine.audio"] as usize * SPAN;
                let raw = &next.schedule.arena[source_span..][..SPAN];
                for i in 0..frames {
                    let t = at + i;
                    let n = signal.len();
                    signal.push([raw[i], raw[CHUNK + i]]);
                    let heard = [output[0][i], output[1][i]];
                    if revision > 0 {
                        let old = signal[n - DELAYS[revision - 1]];
                        let new = signal[n - delay];
                        let g = ((t + 1).saturating_sub(delay).min(FADE as usize)) as f32 / FADE as f32;
                        let expected = std::array::from_fn(|ch| old[ch] * (1.0 - g) + new[ch] * g);
                        assert_eq!(heard[1], expected, "dry stereo tap at edit {revision}, frame {t}, pass {pass}");
                        for port in 0..2 {
                            for ch in 0..2 {
                                let step = (heard[port][ch] - last[port][ch]).abs();
                                assert!(step <= bound + margin, "step {step} at edit {revision}, frame {t}, port {port}, channel {ch}");
                                worst[port] = worst[port].max(step);
                            }
                        }
                    }
                    if t >= delay + FADE as usize {
                        assert_eq!(heard[0], [0.0; 2], "stereo null after edit {revision} at frame {t}, pass {pass}");
                    }
                    if pass == CHUNK { reference.push(heard); }
                    else { assert_eq!(heard, reference[n], "pass slicing changed stereo output at frame {n}"); }
                    last = heard;
                }
            }
            playing = Some(next);
        }
        assert!(signal.iter().any(|s| *s == [0.0, AMP]), "the stereo sine must reach its authored amplitude");
        eprintln!("delay swap, pass {pass}: worst mix step {}, dry step {}, bound {bound} + {margin}", worst[0], worst[1]);
    }
}

#[cfg(test)]
fn song_time_graph(clock: &str, effects: &[(&str, &str, Option<&str>)]) -> OwnerSlice {
    use makepad_flow::document::types::TypeKey;
    use makepad_flow::live::plan::{dnode, LiveInput, LiveSource};

    let node = |id: &str, kind: &str| {
        let params = crate::nodes::descriptor(kind).unwrap().params.iter().map(|p| (p.name.clone(), p.default.clone())).collect();
        LiveNode { id: id.into(), node: dnode(id), type_name: kind.into(), params, inputs: vec![], outputs: vec![] }
    };
    let mut transport = node(clock, "Transport");
    transport.params.iter_mut().find(|(k, _)| k == "playing").unwrap().1 = Literal::Num(1.0);
    let mut nodes = vec![transport];
    for &(id, code, input) in effects {
        let mut fx = node(id, "ShaderFx");
        fx.params.iter_mut().find(|(k, _)| k == "shader").unwrap().1 = Literal::Str(code.into());
        if let Some(from) = input {
            fx.inputs.push(LiveInput {
                port: "audio".into(), ty: TypeKey::new(AUDIO), many: crate::nodes::descriptor("ShaderFx").unwrap().inputs[0].many, control: None,
                sources: vec![LiveSource { from: DPort { node: dnode(from), port: "audio".into() },
                    conn: None, via: vec![], lineage: vec![] }],
            });
        }
        nodes.push(fx);
    }
    OwnerSlice { owner: crate::OWNER.into(), nodes }
}

#[cfg(test)]
fn song_time_copy(delay: usize) -> String {
    format!("latency({delay})
        var left = [0.0; 1024]
        var right = [0.0; 1024]
        var w = int(0)
        fn effect(l, r) {{
            left[w] = l
            right[w] = r
            let sample = vec2(left[w - {delay}], right[w - {delay}])
            w += 1
            sample
        }}")
}

#[test]
fn shader_sidechain_aligns_named_outputs_and_survives_both_edits() {
    use crate::run::{run, NoExt};
    use makepad_flow::live::plan::dnode;

    let budget = Arc::new(AudioBudget::default());
    let delay = song_time_copy(7);
    const SECTION: usize = 311;
    for pass in [CHUNK, 97, 1] {
        let mut playing: Option<Compiled<(), (), ()>> = None;
        for revision in 0..3 {
            // The producer changes the position and width of its connected output.
            // The stereo edit changes the balance but preserves the mono average.
            let split = if revision == 0 {
                "let low = output(1) fn effect(l,r) { low = l; 0.0 }"
            } else {
                "let unused = output(1) let low = output(2) fn effect(l,r) { unused = r; low = vec2(l * 0.5,l * 1.5); 0.0 }"
            };
            // The consumer keeps its running counter across the producer edit.
            let comp = if revision == 2 {
                "let unused = input(2) let key = input(2) var age = int(0) var latest = 0.0
                 fn effect(l,r) { age += 1; latest = (key.x + key.y) * 0.5; vec2(l / max(1.0,latest), float(age)) }"
            } else {
                "let key = input(1) var age = int(0)
                 fn effect(l,r) { age += 1; vec2(l / max(1.0,key), float(age)) }"
            };
            let mut slice = song_time_graph("ports_clock", &[
                ("main", "fn effect(l,r) { 0.5 }", None),
                ("kick", "var tick = int(0) fn effect(l,r) { tick += 1; float(tick % 5) }", None),
                ("late", &delay, Some("kick")), ("split", split, Some("late")), ("comp", comp, Some("main")),
            ]);
            let node = slice.nodes.last_mut().unwrap();
            let mut key = node.inputs[0].clone();
            key.port = "key".into();
            key.sources[0].from = DPort { node: dnode("split"), port: "low".into() };
            key.sources.push(key.sources[0].clone());
            node.inputs.push(key);
            let mut next = compile::<NoExt, ()>(&slice, revision + 1, 1, 48000,
                playing.as_ref().map(|p| &*p.layout), &budget).unwrap();
            if let Some(old) = &mut playing { next.schedule.take_over(&mut old.schedule); }
            assert!(!next.layout.audio_probes.contains_key("split.unused"));
            let span = usize::from(next.layout.audio_probes["comp.audio"]) * SPAN;
            for at in (0..SECTION).step_by(pass) {
                let n = pass.min(SECTION - at);
                run(&mut next.schedule, n, &mut |_, _, _| {}, &mut NoExt);
                for i in 0..n {
                    let t = revision as usize * SECTION + at + i;
                    let expected = if t < 7 { 0.0 } else { 0.5 / (2.0 * ((t - 7 + 1) % 5) as f32).max(1.0) };
                    // The consumer edit renders the old and new gains independently.
                    // Each weighted sample rounds before the two sides are added.
                    let fade = makepad_score_player::shader::FADE as usize;
                    let expected = if revision == 2 && at + i < fade {
                        let mix = (at + i) as f32 / fade as f32;
                        expected * mix + expected * (1.0 - mix)
                    } else { expected };
                    let arena = &next.schedule.arena;
                    assert_eq!(arena[span + i].to_bits(), expected.to_bits(), "key and delayed main at {t}, pass {pass}");
                    assert_eq!(arena[span + CHUNK + i], (t + 1) as f32, "consumer history at {t}, pass {pass}");
                }
            }
            playing = Some(next);
        }
    }
}

#[test]
fn shader_song_time_follows_the_input_across_transport_changes() {
    use crate::feed::{tempo_feed, TempoEvent};
    use crate::run::{run, NoExt};

    const DELAY: usize = 37;
    const METER: usize = 205;
    const STOP: usize = 421;
    const START: usize = 563;
    const END: usize = 807;
    const CLOCK: &str = "song_time_clock";
    let copy = song_time_copy(DELAY);
    let tremolo = "fn effect(l, r) {
        let gain = if playing { 1.0 + lfo_beat(0.0625, 2) } else { 0.5 }
        vec2(l * gain, r * beats_per_bar)
    }";
    let slice = song_time_graph(CLOCK, &[
        ("source", "fn effect(l, r) { vec2(0.75, 1.0) }", None),
        ("early", tremolo, Some("source")), ("early_copy", &copy, Some("early")),
        ("late_copy", &copy, Some("source")), ("late", tremolo, Some("late_copy")),
    ]);
    let mut reference = Vec::new();
    for passes in [&[CHUNK][..], &[97, 1, CHUNK]] {
        let plan = compile::<NoExt, ()>(&slice, 1, 1, 8192, None, &Arc::new(AudioBudget::default())).unwrap();
        let span = |key| usize::from(plan.layout.audio_probes[key]) * SPAN;
        let (early, late, source_fx) = (span("early_copy.audio"), span("late.audio"), span("early.audio"));
        let control = |key| plan.layout.controls[key].0;
        let (playing, meter) = (control("song_time_clock.playing"), control("song_time_clock.beats_per_bar"));
        let mut schedule = plan.schedule;
        // The map changes tempo inside a pass, at the first whole frame (150).
        assert!(tempo_feed(CLOCK).send(TempoEvent { beat: 149.25 / 4096.0, bpm: 180.0, ..Default::default() }));
        let mut split = false;
        let mut at = 0;
        for &pass in passes.iter().cycle() {
            if at == METER { schedule.controls.set_manual(meter, 3.0); }
            if at == STOP { schedule.controls.set_manual(playing, 0.0); }
            if at == START { schedule.controls.set_manual(playing, 1.0); }
            let boundary = [METER, STOP, START, END].into_iter().find(|&t| t > at).unwrap();
            let n = pass.min(boundary - at);
            run(&mut schedule, n, &mut |_, _, _| {}, &mut NoExt);
            split |= schedule.clocks[0].time.count > 1;
            for i in 0..n {
                let h = at + i;
                let stereo = |span: usize| [schedule.arena[span + i].to_bits(), schedule.arena[span + CHUNK + i].to_bits()];
                let heard = stereo(late);
                assert_eq!(stereo(source_fx)[1], (if h < METER { 4.0f32 } else { 3.0 }).to_bits(), "source meter at {h}");
                if h >= DELAY {
                    assert_eq!(heard, stereo(early), "tremolo and copy at frame {h}, passes {passes:?}");
                    assert_eq!(heard[1], (if h < METER + DELAY { 4.0f32 } else { 3.0 }).to_bits(), "delayed meter at {h}");
                }
                if passes.len() == 1 { reference.push(heard); }
                else { assert_eq!(heard, reference[h], "pass slicing at {h}"); }
            }
            at += n;
            if at == END { break; }
        }
        assert!(split, "the tempo map must split a pass");
    }
    crate::feed::forget_instance("song_time");
}

#[test]
fn shader_song_time_switches_offset_after_the_latest_input_fade() {
    use crate::run::{run, NoExt};
    use makepad_score_player::shader::FADE;

    const CLOCK: &str = "song_hold_clock";
    const STEP: f32 = 1.0 / 4096.0; // 120 bpm at 8192 Hz, exactly representable.
    let budget = Arc::new(AudioBudget::default());
    let mut playing: Option<Compiled<(), (), ()>> = None;
    let mut source = Vec::new();
    let mut last_beat = 0.0;
    // Target, section length, offset audible on entry, changed reader program.
    // 512 -> 165 -> 37 interrupts a hold with a third offset. Later,
    // 37 -> 512 -> 37 returns to the already audible tap during warmup.
    for (rev, (delay, frames, previous, program_b)) in [
        (37, 873, 37, false), (512, 999, 37, true), (165, 93, 512, true),
        (37, 421, 512, true), (512, 99, 37, false), (37, 811, 37, false),
        (0, 421, 37, false), (37, 411, 0, false),
    ].into_iter().enumerate() {
        let copy = song_time_copy(delay);
        // Renaming live state forces a program fade. Both programs must see
        // the delayed line, including the program fading out.
        let counter = if program_b { "b" } else { "a" };
        let read = format!("var {counter} = int(0)\nfn effect(l, r) {{ {counter} += 1\nvec2(beats_per_bar, beat) }}");
        let slice = song_time_graph(CLOCK, &[
            ("source", "fn effect(l, r) { vec2(beats_per_bar, beat) }", None),
            ("copy", &copy, Some("source")), ("read", &read, Some("copy")),
        ]);
        let mut next = compile::<NoExt, ()>(&slice, rev as u64 + 1, 1, 8192, playing.as_ref().map(|p| &*p.layout), &budget).unwrap();
        if let Some(old) = &mut playing { next.schedule.take_over(&mut old.schedule); }
        let raw = usize::from(next.layout.audio_probes["source.audio"]) * SPAN;
        let read = usize::from(next.layout.audio_probes["read.audio"]) * SPAN;
        let meter = next.layout.controls["song_hold_clock.beats_per_bar"].0;
        let mut at = 0;
        for &pass in [97, 1, CHUNK].iter().cycle() {
            // A new meter on every single-frame pass fills the old window
            // before it migrates through two rapid edits to a smaller target.
            let n = if (1..=2).contains(&rev) { 1 } else { pass.min(frames - at) };
            next.schedule.controls.set_manual(meter, (2 + source.len() % 7) as f32);
            run(&mut next.schedule, n, &mut |_, _, _| {}, &mut NoExt);
            let arena = &next.schedule.arena;
            for i in 0..n {
                let h = source.len();
                source.push([arena[raw + i], arena[raw + CHUNK + i]]);
                let t = at + i;
                let switch = delay + FADE as usize;
                let offset = if t < switch { previous } else { delay };
                let beat = arena[read + CHUNK + i];
                if let Some(record) = h.checked_sub(offset).map(|s| source[s]) {
                    assert_eq!(arena[read + i].to_bits(), record[0].to_bits(), "meter history, revision {rev}, frame {t}");
                    assert_eq!(beat.to_bits(), record[1].to_bits(), "delayed source beat, revision {rev}, frame {t}");
                }
                if h > 0 {
                    let delta = if t == switch { 1 + previous as i32 - delay as i32 } else { 1 };
                    assert_eq!(beat - last_beat, delta as f32 * STEP, "beat step, revision {rev}, frame {t}");
                }
                last_beat = beat;
            }
            at += n;
            if at == frames { break; }
        }
        assert_eq!(next.layout.dropped_song_time_records(), 0, "song-time records lost in revision {rev}");
        playing = Some(next);
    }
    crate::feed::forget_instance("song_hold");
}

#[test]
fn a_playing_transport_renders_quiet_passes_whole() {
    use crate::run::{run_awake, NoExt};

    const CLOCK: &str = "quiet_pass/clock";
    const RATE: usize = 8192;
    const STOP: usize = RATE + 2 * CHUNK + 17;
    let slice = song_time_graph(CLOCK, &[]);
    let compiled = compile::<NoExt, ()>(&slice, 1, 1, RATE as u64, None, &Arc::new(AudioBudget::default())).unwrap();
    let mut schedule = compiled.schedule;
    let playing = compiled.layout.controls["quiet_pass/clock.playing"].0;
    // Keep playing beyond the one-second hold, then stop off the grid.
    for start in (0..STOP).step_by(CHUNK) {
        let frames = CHUNK.min(STOP - start);
        run_awake(&mut schedule, frames, &mut |_, _, _| {}, &mut NoExt, true);
        assert_eq!(schedule.clock_frames, frames, "rest split a playing pass ending at {}", schedule.frame);
        assert!(!schedule.rest.asleep());
    }
    schedule.controls.set_manual(playing, 0.0);
    run_awake(&mut schedule, CHUNK, &mut |_, _, _| {}, &mut NoExt, true);
    assert!(schedule.rest.asleep(), "silence during playback counts toward the hold");
    assert_eq!(schedule.frame, STOP.next_multiple_of(64) as u64, "sleep on the first grid frame after the stop");
    crate::feed::forget_instance("quiet_pass");
}

#[test]
fn dense_tempo_map_keeps_its_exact_frames_across_a_hot_swap() {
    use crate::feed::{tempo_feed, tempo_seconds, TempoEvent};
    use crate::run::{run, NoExt};

    const CLOCK: &str = "split_report_clock";
    const RATE: u64 = 8192;
    let slice = song_time_graph(CLOCK, &[("beat", "fn effect(l, r) { vec2(beat, bar) }", None)]);
    let budget = Arc::new(AudioBudget::default());
    let prepare = |generation, base: Option<&Layout>| compile::<NoExt, ()>(&slice, generation, 1, RATE, base, &budget).unwrap();
    let mut playing = prepare(1, None);
    let span = playing.layout.audio_probes["beat.audio"] as usize * SPAN;
    let feed = tempo_feed(CLOCK);
    let map: Vec<_> = (1..=12).map(|i| TempoEvent {
        beat: i as f64 / 4096.0, bpm: if i % 2 == 1 { 60.0 } else { 120.0 }, ..Default::default()
    }).collect();
    for (index, pass) in [256, 97, 1].into_iter().enumerate() {
        if index != 0 {
            let mut next = prepare(index as u64 + 1, Some(&playing.layout));
            next.schedule.take_over(&mut playing.schedule);
            playing = next;
        }
        assert!(feed.send(TempoEvent { locate: true, bpm: 120.0, ..Default::default() }));
        for &change in &map { assert!(feed.send(change)); }
        for frame in (0..256).step_by(pass) {
            let n = pass.min(256 - frame);
            run(&mut playing.schedule, n, &mut |_, _, _| {}, &mut NoExt);
            for i in 0..n {
                let beat = playing.schedule.arena[span + i] as f64;
                assert_eq!(tempo_seconds(120.0, &map, beat) * RATE as f64, (frame + i) as f64,
                    "wrong beat at frame {}, pass {pass}", frame + i);
            }
        }
        assert_eq!(playing.layout.take_report(), None);
    }
    // Five different anchors between adjacent samples. Phrase playback also
    // uses the fractional frame within that interval to interpolate its PCM.
    assert!(feed.send(TempoEvent { locate: true, bpm: 120.0, ..Default::default() }));
    let unit = 1.0 / 65536.0;
    for (beat, bpm) in [(2.0, 60.0), (3.0, 120.0), (5.0, 60.0), (6.0, 120.0), (8.0, 60.0)] {
        assert!(feed.send(TempoEvent { beat: beat * unit, bpm, ..Default::default() }));
    }
    run(&mut playing.schedule, 1, &mut |_, _, _| {}, &mut NoExt);
    let time = &playing.schedule.clocks[0].tempo;
    assert_eq!(time.end_beat(1), 11.0 * unit, "the sample ends on the integrated beat");
    assert_eq!(time.frame_of(2.0 * unit), 0.125, "the first tempo change is one eighth of a sample in");
    let shader = &playing.schedule.clocks[0].time;
    assert_eq!(&shader.anchors[1..shader.count as usize], &[(11.0 * unit, 1, 8.0 * unit, 60.0)],
        "one shader anchor at the whole frame carries the last tempo");
    run(&mut playing.schedule, 1, &mut |_, _, _| {}, &mut NoExt);
    let next = &playing.schedule.clocks[0];
    assert_eq!(next.start, 11.0 * unit, "the next pass starts on the integrated whole-frame beat");
    assert_eq!(next.tempo.end_beat(1), 19.0 * unit, "the final tempo carries into the following sample");

    // An event on the shared boundary replaces the preceding sample's
    // endpoint. The next dense sample and the cut after it both stay in time.
    for (beat, bpm) in [(20.0, 120.0), (22.0, 60.0), (23.0, 120.0), (33.0, 60.0),
                        (34.0, 120.0), (36.0, 60.0), (44.0, 120.0)] {
        assert!(feed.send(TempoEvent { beat: beat * unit, bpm, ..Default::default() }));
    }
    run(&mut playing.schedule, 3, &mut |_, _, _| {}, &mut NoExt);
    for (i, beat) in [19.0, 33.0, 42.0].into_iter().enumerate() {
        assert_eq!(playing.schedule.arena[span + i] as f64, beat * unit, "whole-frame shader beat {i}");
    }
    assert_eq!(playing.schedule.clocks[0].tempo.end_beat(playing.schedule.clock_frames), 56.0 * unit,
        "the final internal pass ends on the integrated beat");
    crate::feed::forget_instance("split_report");
}
