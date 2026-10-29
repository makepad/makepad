//! A loop's sound: its instrument and the fixed pool of voices that play
//! its events, rendered into a chunk with every note starting and stopping
//! on its exact frame. Whoever hosts loops (Stage's audio graph, a game's
//! bus, an offline render) turns event beats into frames and calls this;
//! nothing here allocates, locks or frees.

use crate::event::{Event, EventKind, NoteOpts};
use makepad_audio_synth::ironfish::{Ironfish, IronfishPatch};
use makepad_audio_synth::morph::{Morph, MorphPatch, MorphSources};
use makepad_audio_synth::tandem::{program::Program as TandemProgram, Tandem};
use makepad_audio_synth::instrument::{Control, Instrument as NativeInstrument};
use makepad_audio_synth::instruments::InstrumentKind;
use makepad_audio_synth::recipe::Recipe;
use makepad_script_audio_aot::{AudioShaderRef};
use crate::shader::{ShaderInst, SHADER_VOICES};
#[cfg(not(target_arch = "wasm32"))]
use makepad_score_voice::neural::NeuralSinger;
use makepad_score_voice::{Choir, ChoirStyle, PhonemeSeq, SungNote, VoiceProfile};
use std::sync::Arc;
use crate::presets::CHOKES;
use crate::voice::{pan_gains, with_opts, SamplePlay, Voice};

pub const MAX_LOOP_VOICES: usize = 12;

/// Tempo changes one pass may hold.
pub const MAX_TEMPO_SPLITS: usize = 4;

/// Where the beats fall in one pass: the beat at its first frame and beats
/// per frame, changing at up to [`MAX_TEMPO_SPLITS`] beats (a tempo map's
/// steps). Frames are fractional pass frames, so a change lands between
/// samples exactly where the tempo map puts it, whatever the pass size.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tempo {
    pub beat0: f64,
    pub per_frame: f64,
    /// (beat, frame, beats per frame from there).
    pub splits: [(f64, f64, f64); MAX_TEMPO_SPLITS],
    pub nsplits: u8,
}

impl Tempo {
    pub fn linear(beat0: f64, per_frame: f64) -> Tempo {
        Tempo { beat0, per_frame, ..Tempo::default() }
    }

    /// Whether beats advance (the transport plays).
    pub fn moving(&self) -> bool {
        self.per_frame > 0.0 || self.splits[..self.nsplits as usize].iter().any(|s| s.2 > 0.0)
    }

    fn segment(&self, beat: f64) -> (f64, f64, f64) {
        let mut at = (self.beat0, 0.0, self.per_frame);
        for split in &self.splits[..self.nsplits as usize] {
            if beat >= split.0 {
                at = *split;
            }
        }
        at
    }

    /// The (fractional) frame of the pass `beat` falls on.
    pub fn frame_of(&self, beat: f64) -> f64 {
        let (b, f, per) = self.segment(beat);
        if per > 0.0 { f + (beat - b) / per } else { f64::INFINITY }
    }

    /// The beat at (fractional) frame `frame`.
    pub fn beat_at(&self, frame: f64) -> f64 {
        let mut at = (self.beat0, 0.0, self.per_frame);
        for split in &self.splits[..self.nsplits as usize] {
            if frame >= split.1 {
                at = *split;
            }
        }
        at.0 + (frame - at.1) * at.2
    }

    /// The beat where a pass of `frames` frames ends.
    pub fn end_beat(&self, frames: usize) -> f64 {
        self.beat_at(frames as f64)
    }
}

/// Decoded audio for a sampler or a kit slot, with the frames its hits
/// start at (for `onset:` slicing).
pub struct Sample {
    pub left: Arc<[f32]>,
    pub right: Option<Arc<[f32]>>,
    pub rate: f32,
    pub onsets: Arc<[u32]>,
    /// Frames `from..to` the sample loops over once played into, until the
    /// note ends (a tracker or SoundFont sample's sustain loop); none plays
    /// it once through.
    pub sustain: Option<(u32, u32)>,
}

impl Sample {
    pub fn new(left: Arc<[f32]>, right: Option<Arc<[f32]>>, rate: f32) -> Sample {
        let onsets = onsets(&left, rate).into();
        Sample { left, right, rate, onsets, sustain: None }
    }

    /// The sample with a sustain loop over frames `from..to` (ignored when
    /// it is not inside the sample or shorter than two frames).
    pub fn with_loop(mut self, from: u32, to: u32) -> Sample {
        let to = to.min(self.len() as u32);
        self.sustain = (to > from + 1).then_some((from, to));
        self
    }

    /// The region `opts` names, in frames (`from` < `to`).
    pub fn region(&self, opts: &NoteOpts) -> (f64, f64) {
        let len = self.len() as f64;
        if opts.onset.is_finite() && !self.onsets.is_empty() {
            let n = self.onsets.len();
            let k = (opts.onset.max(0.0) as usize) % n;
            let to = self.onsets.get(k + 1).map_or(len, |&o| o as f64);
            return (self.onsets[k] as f64, to);
        }
        let frac = |v: f32, d: f64| if v.is_finite() { (v as f64).clamp(0.0, 1.0) } else { d };
        let (a, b) = (frac(opts.start, 0.0) * len, frac(opts.finish, 1.0) * len);
        if b - a < 2.0 { (a.min(len - 2.0).max(0.0), (a + 2.0).min(len)) } else { (a, b) }
    }

    pub fn len(&self) -> usize {
        self.left.len()
    }

    pub fn is_empty(&self) -> bool {
        self.left.is_empty()
    }
}

/// Where the hits in `left` start: energy rising well above what came just
/// before, at least 50 ms apart; the first sound always counts.
pub fn onsets(left: &[f32], rate: f32) -> Vec<u32> {
    const HOP: usize = 256;
    let energy: Vec<f32> = left.chunks(HOP).map(|c| c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).collect();
    let peak = energy.iter().cloned().fold(0.0f32, f32::max);
    let floor = peak * 1e-3;
    let gap = ((0.05 * rate) as usize / HOP).max(1);
    let mut out: Vec<u32> = Vec::new();
    let mut last: Option<usize> = None;
    for i in 0..energy.len() {
        let before = &energy[i.saturating_sub(8)..i];
        let mean = if before.is_empty() { 0.0 } else { before.iter().sum::<f32>() / before.len() as f32 };
        let rising = energy[i] > floor && energy[i] > mean * 4.0 + floor;
        if rising && last.map_or(true, |l| i - l >= gap) {
            // To the first frame of the hop that sounds.
            let level = energy[i].sqrt() * 0.1;
            let hop = &left[i * HOP..((i + 1) * HOP).min(left.len())];
            let at = hop.iter().position(|x| x.abs() > level).unwrap_or(0);
            out.push((i * HOP + at) as u32);
            last = Some(i);
        }
        if out.len() >= 256 {
            break;
        }
    }
    if out.is_empty() {
        out.push(0);
    }
    out
}

#[derive(Clone)]
pub struct KitSlot {
    pub recipe: Option<Recipe>,
    pub sample: Option<Arc<Sample>>,
    pub gain: f32,
    /// Pitch multiplier (1 = as recorded or written).
    pub pitch: f32,
}

impl Default for KitSlot {
    fn default() -> Self {
        KitSlot { recipe: None, sample: None, gain: 1.0, pitch: 1.0 }
    }
}

/// A loop's instrument. `key` names the instrument node, so a knob on it
/// reaches every loop that plays it.
#[derive(Clone)]
pub enum Inst {
    Synth { key: u64, recipe: Recipe, mono: bool },
    Drums { key: u64, slots: Arc<[KitSlot]> },
    Sampler { key: u64, sample: Arc<Sample>, root: f32, attack: f32, release: f32 },
    /// Ironfish: its own voices and effects, which live in the loop's
    /// engine (see [`LoopEngine::for_inst`]); the patch plays from
    /// the next chunk when it changes.
    Ironfish { key: u64, patch: IronfishPatch },
    /// Morph, the wavetable synth: its voices and rack live in the loop's
    /// engine; a new patch (another `Arc`) plays from the next chunk.
    Morph { key: u64, patch: Arc<MorphPatch>, src: Arc<MorphSources> },
    /// Tandem, the hybrid synth: its compiled patch (built off the audio
    /// thread); its two scenes and effects live in the loop's engine.
    Tandem { key: u64, prog: Arc<TandemProgram> },
    /// An audio shader instrument: its voices live in the loop's engine;
    /// `params` are (index, value) pairs set on it.
    Shader { key: u64, shader: AudioShaderRef, params: Arc<[(u16, f32)]>, mono: bool },
    /// Sung voices: a choir of `singers` (1 = a solo voice), their
    /// profiles cycled; lives in the loop's engine.
    Voice { key: u64, profiles: Arc<[Arc<VoiceProfile>]>, singers: u8, style: ChoirStyle },
    /// A neural singer: plays its phrases (`EventKind::Phrase`), rendered
    /// ahead by the singer's own thread, from where each starts; a phrase
    /// not rendered yet joins at the place it has reached when it arrives.
    /// (Native builds: the browser has no singer model.)
    #[cfg(not(target_arch = "wasm32"))]
    Neural { key: u64, singer: NeuralSinger },
    /// A native Makepad instrument (the modelled piano, the modelled drum
    /// kit, a SoundFont program...), one per General MIDI program the loop
    /// plays (the first plays until a program change); built in the loop's
    /// engine off the audio thread.
    Native { key: u64, kinds: Arc<[(u8, InstrumentKind)]>, gain: f32 },
    /// An outboard instrument: the notes go out as MIDI on `channel`
    /// (0..15) to the output port named by `port` (a hash; 0 is every
    /// output), frame-stamped for the host to send (see
    /// [`LoopEngine::take_midi`]). The loop renders silence.
    Midi { key: u64, channel: u8, port: u64 },
    /// Live audio in (a microphone, a line in) at `gain`: the host fills
    /// the loop's output from the input; the engine renders silence.
    Input { key: u64, gain: f32 },
    Silent,
}

impl Inst {
    pub fn key(&self) -> u64 {
        match self {
            Inst::Synth { key, .. } | Inst::Drums { key, .. } | Inst::Sampler { key, .. } | Inst::Ironfish { key, .. } | Inst::Morph { key, .. } | Inst::Tandem { key, .. } | Inst::Shader { key, .. } | Inst::Voice { key, .. } | Inst::Native { key, .. } | Inst::Midi { key, .. } | Inst::Input { key, .. } => *key,
            #[cfg(not(target_arch = "wasm32"))]
            Inst::Neural { key, .. } => *key,
            Inst::Silent => 0,
        }
    }
}

/// Actions one chunk of one loop may hold.
pub const MAX_LOOP_ACTS: usize = 128;
/// Events a loop may have waiting for their chunk.
pub const MAX_LOOP_PENDING: usize = 1024;
/// The longest chunk rendered at once.
pub const MAX_LOOP_FRAMES: usize = 256;

/// Neural phrases a loop may have playing at once.
pub const MAX_PHRASES: usize = 16;

/// A neural phrase playing: its audio once it is there, where the next
/// frame reads it (samples of the phrase's rate; below 0 it has not begun),
/// and its level.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
struct PhrasePlay {
    key: u64,
    audio: Option<Arc<[f32]>>,
    pos: f64,
    gain: f32,
}

#[derive(Clone, Copy, Debug)]
struct Act {
    frame: u16,
    event: Option<Event>,
    release: u8,
}

pub struct LoopEngine {
    pub key: u64,
    voices: Box<[Voice]>,
    scratch: Box<[f32]>,
    acts: Box<[Act]>,
    nacts: usize,
    serial: u32,
    pending: Box<[Event]>,
    npending: usize,
    /// Events that arrived after their frame had played.
    pub late: u32,
    /// Events that did not fit.
    pub dropped: u32,
    /// The last note's overrides: cutoff, res, attack, decay, sustain,
    /// release, drive (NaN where the patch value played).
    pub last_opts: [f32; 7],
    /// An Ironfish loop's synth, and its held notes' ends as (beat, note).
    iron: Option<Box<Ironfish>>,
    iron_offs: Box<[(f64, u8)]>,
    /// A Morph loop's synth (its notes' ends share `iron_offs`).
    morph: Option<Box<Morph>>,
    /// A Tandem loop's synth (its notes' ends share `iron_offs`).
    tandem: Option<Box<Tandem>>,
    /// A shader loop's instrument; per voice (end beat, serial); the
    /// params last set (their list's address).
    shader: Option<Box<ShaderInst>>,
    shader_voices: Box<[(f64, u32)]>,
    shader_handles: Box<[u32]>,
    shader_params: usize,
    /// A sung loop's choir; its held notes as (end beat, choir serial).
    choir: Option<Box<Choir>>,
    choir_offs: Box<[(f64, u32)]>,
    /// A neural singer's phrases playing (or waiting for their audio).
    phrases: Box<[Option<PhrasePlay>]>,
    /// A native loop's instruments by program, the one playing, its held
    /// notes as (end beat, note, instrument), and a mix buffer.
    natives: Vec<(u8, Box<dyn NativeInstrument>)>,
    native_current: usize,
    native_offs: Box<[(f64, u8, u8)]>,
    native_l: Box<[f32]>,
    native_r: Box<[f32]>,
    /// A MIDI loop's held notes (end beat, note), and the messages of its
    /// last chunk as (frame, bytes).
    midi_offs: Box<[(f64, u8)]>,
    midi_out: Box<[(u16, [u8; 3])]>,
    nmidi: usize,
    midi_channel: u8,
    /// A recipe synth's patch with the settings an editor holds live (its
    /// next notes play it), or none: the authored patch.
    live_recipe: Option<Recipe>,
}

/// Messages a MIDI loop sends in one chunk.
pub const MAX_MIDI_OUT: usize = 256;

/// Held Ironfish notes a loop tracks.
const MAX_IRON_HELD: usize = 64;
/// How long an Ironfish or shader note without a length holds, in beats.
const HELD_DEFAULT_BEATS: f64 = 0.25;

fn mix_seed(key: u64, beat: f64, note: f32) -> u32 {
    let mut h = key ^ beat.to_bits().rotate_left(17) ^ (note.to_bits() as u64).rotate_left(41);
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    (h as u32) | 1
}

/// General MIDI drum numbers onto kit slots, so `play(36)` on a kit hits.
fn gm_drum(note: f32) -> Option<u8> {
    Some(match note.round() as i32 {
        35 | 36 => 0,
        38 | 40 => 1,
        39 => 2,
        42 | 44 => 3,
        46 => 4,
        37 => 5,
        41 | 43 => 6,
        45 | 47 => 7,
        48 | 50 => 8,
        49 | 52 | 55 | 57 => 9,
        51 | 53 | 59 => 10,
        69 | 70 | 82 => 11,
        56 => 12,
        60..=64 => 13,
        _ => return None,
    })
}

impl LoopEngine {
    /// Allocates the pool: off the audio thread.
    pub fn new(key: u64) -> LoopEngine {
        LoopEngine {
            key,
            voices: (0..MAX_LOOP_VOICES).map(|_| Voice::idle()).collect(),
            scratch: vec![0.0; MAX_LOOP_FRAMES].into_boxed_slice(),
            acts: vec![Act { frame: 0, event: None, release: 0 }; MAX_LOOP_ACTS].into_boxed_slice(),
            nacts: 0,
            serial: 1,
            pending: vec![Event { beat: 0.0, loop_key: 0, kind: EventKind::Note, note: 0.0, vel: 0.0, pan: 0.0, dur: 0.0, opts: Default::default() }; MAX_LOOP_PENDING].into_boxed_slice(),
            npending: 0,
            late: 0,
            dropped: 0,
            last_opts: [f32::NAN; 7],
            iron: None,
            iron_offs: Box::new([]),
            morph: None,
            tandem: None,
            shader: None,
            shader_voices: Box::new([]),
            shader_handles: Box::new([]),
            shader_params: 0,
            choir: None,
            choir_offs: Box::new([]),
            phrases: Box::new([]),
            natives: Vec::new(),
            native_current: 0,
            native_offs: Box::new([]),
            native_l: Box::new([]),
            native_r: Box::new([]),
            midi_offs: Box::new([]),
            midi_out: Box::new([]),
            nmidi: 0,
            midi_channel: 0,
            live_recipe: None,
        }
    }

    /// Settings held live are about to be set again: back to the authored
    /// ones first (a shader's params as the revision wrote them; a recipe
    /// synth's patch). No allocation.
    pub fn live_begin(&mut self, inst: &Inst) {
        self.live_recipe = None;
        if let (Inst::Shader { params, .. }, Some(shader)) = (inst, self.shader.as_mut()) {
            for (index, value) in params.iter() {
                shader.set_param(*index as usize, *value);
            }
            self.shader_params = Arc::as_ptr(params) as *const u8 as usize;
        }
    }

    /// One setting held live: a shader's param by name (it sounds at once,
    /// on the notes playing), a recipe synth's layer key on every layer
    /// (`cutoff`, `q`, `a` …) or a recipe key (`volume`): its next notes.
    /// Unknown names are ignored. No allocation.
    pub fn live_set(&mut self, inst: &Inst, name: &str, value: f32) {
        match inst {
            Inst::Shader { .. } => {
                if let Some(shader) = self.shader.as_mut() {
                    shader.set_param_by_name(name, value);
                }
            }
            Inst::Synth { recipe, .. } => {
                let live = self.live_recipe.get_or_insert(*recipe);
                if Recipe::is_recipe_key(name) {
                    let _ = live.set_num(name, value);
                } else {
                    for layer in live.layers.iter_mut().take(live.count) {
                        let _ = layer.set_num(name, value);
                    }
                }
            }
            _ => {}
        }
    }

    /// A loop that plays `inst`, with whatever it needs made here (off the
    /// audio thread): an Ironfish loop's synth, a choir, native instruments
    /// at `rate`. `carried`: the loop's state from the graph playing takes
    /// this one's place (same instrument), so the slow instruments are not
    /// built again (a shader's new program still is: it takes over).
    pub fn for_inst(key: u64, inst: &Inst, rate: f32, carried: bool) -> Result<LoopEngine, String> {
        let mut engine = LoopEngine::new(key);
        if carried && !matches!(inst, Inst::Shader { .. }) {
            return Ok(engine);
        }
        if let Inst::Native { kinds, .. } = inst {
            for (program, kind) in kinds.iter() {
                engine.natives.push((*program, makepad_audio_synth::instruments::build(kind, rate)?));
            }
            engine.native_offs = vec![(f64::INFINITY, 0, 0); 256].into_boxed_slice();
            engine.native_l = vec![0.0; MAX_LOOP_FRAMES].into_boxed_slice();
            engine.native_r = vec![0.0; MAX_LOOP_FRAMES].into_boxed_slice();
        }
        if let Inst::Midi { channel, .. } = inst {
            engine.midi_channel = *channel & 15;
            engine.midi_offs = vec![(f64::INFINITY, 0); 128].into_boxed_slice();
            engine.midi_out = vec![(0, [0; 3]); MAX_MIDI_OUT].into_boxed_slice();
        }
        if let Inst::Ironfish { patch, .. } = inst {
            engine.iron = Some(Box::new(Ironfish::new(rate, *patch)));
            engine.iron_offs = vec![(f64::INFINITY, 0); MAX_IRON_HELD].into_boxed_slice();
        }
        if let Inst::Morph { patch, src, .. } = inst {
            engine.morph = Some(Box::new(Morph::new(rate, patch.clone(), src.clone())));
            engine.iron_offs = vec![(f64::INFINITY, 0); MAX_IRON_HELD].into_boxed_slice();
        }
        if let Inst::Tandem { prog, .. } = inst {
            let mut t = Box::new(Tandem::new(rate));
            t.set_program(prog.clone());
            engine.tandem = Some(t);
            engine.iron_offs = vec![(f64::INFINITY, 0); MAX_IRON_HELD].into_boxed_slice();
        }
        if let Inst::Shader { shader, params, .. } = inst {
            let mut voices = Box::new(ShaderInst::new(shader.clone(), rate));
            for (index, value) in params.iter() {
                voices.set_param(*index as usize, *value);
            }
            engine.shader = Some(voices);
            engine.shader_voices = vec![(f64::INFINITY, 0); SHADER_VOICES].into_boxed_slice();
            engine.shader_handles = vec![0; SHADER_VOICES].into_boxed_slice();
            engine.shader_params = Arc::as_ptr(params) as *const u8 as usize;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Inst::Neural { .. } = inst {
            engine.phrases = (0..MAX_PHRASES).map(|_| None).collect();
        }
        if let Inst::Voice { profiles, singers, style, .. } = inst {
            let seed = (key ^ (key >> 32)) as u32;
            engine.choir = Some(Box::new(Choir::new(profiles, *singers as usize, rate, *style, seed)));
            engine.choir_offs = vec![(f64::INFINITY, 0); MAX_IRON_HELD].into_boxed_slice();
        }
        Ok(engine)
    }

    /// Hot swap of a shader loop: `self` carried the loop's state over,
    /// `fresh` was made for the new program. The new program takes the
    /// sounding voices (their state carried where it matches, else a
    /// crossfade); `fresh` keeps the old program's emptied instance, to be
    /// dropped off the audio thread. No allocation.
    pub fn take_program(&mut self, fresh: &mut LoopEngine) {
        if let (Some(old), Some(new)) = (&mut self.shader, &mut fresh.shader) {
            if !old.shader().ptr_eq(new.shader()) {
                new.swap_from(old);
                std::mem::swap(old, new);
                self.shader_params = 0;
            }
        }
    }

    pub fn voices(&self) -> usize {
        match (&self.iron, &self.shader) {
            (Some(iron), _) => iron.active_voices() as usize,
            (_, Some(shader)) => shader.active_voices(),
            _ if self.choir.is_some() => self.choir_offs.iter().filter(|(off, _)| off.is_finite()).count(),
            _ if !self.phrases.is_empty() => self.phrases.iter().filter(|p| p.is_some()).count(),
            _ if !self.natives.is_empty() => self.native_offs.iter().filter(|(off, ..)| off.is_finite()).count(),
            _ => self.voices.iter().filter(|v| v.active).count(),
        }
    }

    /// Queue an event (in beat order or not).
    pub fn push(&mut self, event: Event) {
        if self.npending >= MAX_LOOP_PENDING {
            self.dropped += 1;
            return;
        }
        let at = self.pending[..self.npending].partition_point(|p| p.beat <= event.beat);
        self.pending.copy_within(at..self.npending, at + 1);
        self.pending[at] = event;
        self.npending += 1;
    }

    /// Take `other`'s waiting events (an instrument switch: the notes go
    /// on, on this engine's instrument). No allocation.
    pub fn take_pending(&mut self, other: &mut LoopEngine) {
        for i in 0..other.npending {
            self.push(other.pending[i]);
        }
        other.npending = 0;
    }

    /// Drop queued events before `beat` (a locate).
    pub fn forget_before(&mut self, beat: f64) {
        let keep = self.pending[..self.npending].iter().filter(|e| e.beat >= beat - 1e-9).count();
        let start = self.npending - keep;
        self.pending.copy_within(start..self.npending, 0);
        self.npending = keep;
    }

    pub fn release_all(&mut self) {
        for i in 0..self.midi_offs.len() {
            let (off, note) = self.midi_offs[i];
            if off.is_finite() {
                self.midi_offs[i] = (f64::INFINITY, 0);
                self.send_midi(0, [0x80 | self.midi_channel, note, 0]);
            }
        }
        for voice in self.voices.iter_mut() {
            voice.release();
        }
        if let Some(iron) = &mut self.iron {
            iron.all_notes_off();
            self.iron_offs.fill((f64::INFINITY, 0));
        }
        for (_, inst) in self.natives.iter_mut() {
            inst.all_notes_off();
        }
        self.native_offs.fill((f64::INFINITY, 0, 0));
        if let Some(choir) = &mut self.choir {
            choir.release_all(0);
            self.choir_offs.fill((f64::INFINITY, 0));
        }
        for play in self.phrases.iter_mut() {
            *play = None;
        }
        if let Some(shader) = &mut self.shader {
            for (voice, (off, _)) in self.shader_voices.iter_mut().enumerate() {
                if off.is_finite() {
                    shader.note_off(voice as u8);
                }
                *off = f64::INFINITY;
            }
        }
    }

    /// A held note let go: it ends at the release's beat, whatever holds it.
    fn release_held(&mut self, event: &Event) {
        let (handle, at) = (event.opts.handle, event.beat);
        let note = event.note.round().clamp(0.0, 127.0) as u8;
        if handle != 0 {
            for voice in self.voices.iter_mut().filter(|v| v.active && !v.released && v.handle == handle) {
                voice.off_beat = voice.off_beat.min(at);
            }
            for (i, h) in self.shader_handles.iter().enumerate() {
                if *h == handle && self.shader_voices[i].0.is_finite() {
                    self.shader_voices[i].0 = self.shader_voices[i].0.min(at);
                }
            }
        }
        for (off, n) in self.iron_offs.iter_mut().chain(self.midi_offs.iter_mut()) {
            if off.is_finite() && *n == note {
                *off = off.min(at);
            }
        }
        for (off, n, _) in self.native_offs.iter_mut() {
            if off.is_finite() && *n == note {
                *off = off.min(at);
            }
        }
    }

    /// Queue the events that start in this chunk as acts; the chunk's end
    /// beat.
    fn take_events(&mut self, frames: usize, t: &Tempo) -> f64 {
        let beat1 = t.end_beat(frames);
        let mut taken = 0;
        while taken < self.npending {
            let event = self.pending[taken];
            if event.beat >= beat1 - 1e-12 {
                break;
            }
            taken += 1;
            if let EventKind::Release = event.kind {
                self.release_held(&event);
                continue;
            }
            let f = (t.frame_of(event.beat) - 1e-6).ceil();
            let frame = if f < 0.0 {
                if t.frame_of(event.beat) < -2.0 {
                    self.late += 1;
                }
                0
            } else {
                (f as usize).min(frames - 1)
            };
            if self.nacts < MAX_LOOP_ACTS {
                self.acts[self.nacts] = Act { frame: frame as u16, event: Some(event), release: 0 };
                self.nacts += 1;
            } else {
                self.dropped += 1;
            }
        }
        self.pending.copy_within(taken..self.npending, 0);
        self.npending -= taken;
        beat1
    }

    /// The MIDI a MIDI loop's last chunk sent, as (frame, bytes), in order.
    pub fn take_midi(&mut self, mut f: impl FnMut(u16, [u8; 3])) {
        for (frame, bytes) in &self.midi_out[..self.nmidi] {
            f(*frame, *bytes);
        }
        self.nmidi = 0;
    }

    fn send_midi(&mut self, frame: usize, bytes: [u8; 3]) {
        if self.nmidi < self.midi_out.len() {
            self.midi_out[self.nmidi] = (frame as u16, bytes);
            self.nmidi += 1;
        } else {
            self.dropped += 1;
        }
    }

    /// A MIDI loop's chunk: its notes as note on / off, program changes,
    /// the pedal as CC 64, a glide's level as CC 7 (a note's glide pitch
    /// has no per-note MIDI).
    fn render_midi(&mut self, channel: u8, frames: usize, t: &Tempo) {
        self.midi_channel = channel & 15;
        if self.midi_offs.is_empty() || !t.moving() {
            return;
        }
        let ch = channel & 15;
        let beat1 = self.take_events(frames, t);
        for (index, (off, _)) in self.midi_offs.iter().enumerate() {
            if *off < beat1 && self.nacts < MAX_LOOP_ACTS {
                let f = (t.frame_of(*off) - 1e-6).ceil().max(0.0) as usize;
                self.acts[self.nacts] = Act { frame: f.min(frames - 1) as u16, event: None, release: index as u8 };
                self.nacts += 1;
            }
        }
        // Offs before ons on a frame, so a repeated note sounds again.
        self.acts[..self.nacts].sort_unstable_by_key(|a| (a.frame, a.event.is_some()));
        for a in 0..self.nacts {
            let act = self.acts[a];
            let frame = act.frame as usize;
            match act.event {
                None => {
                    let (off, note) = self.midi_offs[act.release as usize];
                    if off.is_finite() {
                        self.midi_offs[act.release as usize] = (f64::INFINITY, 0);
                        self.send_midi(frame, [0x80 | ch, note, 0]);
                    }
                }
                Some(event) => match event.kind {
                    EventKind::Program(program) => self.send_midi(frame, [0xc0 | ch, program & 127, 0]),
                    EventKind::Pedal => self.send_midi(frame, [0xb0 | ch, 64, (event.vel.clamp(0.0, 1.0) * 127.0).round() as u8]),
                    EventKind::Glide => {
                        if event.vel.is_finite() {
                            self.send_midi(frame, [0xb0 | ch, 7, (event.vel.clamp(0.0, 1.0) * 127.0).round() as u8]);
                        }
                    }
                    kind => {
                        let note = match kind {
                            EventKind::Hit(_, gm) => gm,
                            _ => event.note.round().clamp(0.0, 127.0) as u8,
                        };
                        // The same note held ends first.
                        for i in 0..self.midi_offs.len() {
                            if self.midi_offs[i].0.is_finite() && self.midi_offs[i].1 == note {
                                self.midi_offs[i] = (f64::INFINITY, 0);
                                self.send_midi(frame, [0x80 | ch, note, 0]);
                            }
                        }
                        let velocity = (event.vel.clamp(0.0, 1.0) * 127.0).round().clamp(1.0, 127.0) as u8;
                        self.send_midi(frame, [0x90 | ch, note, velocity]);
                        let len = if event.dur > 0.0 { event.dur as f64 } else { HELD_DEFAULT_BEATS };
                        match self.midi_offs.iter_mut().find(|(o, _)| !o.is_finite()) {
                            Some(slot) => *slot = (event.beat + len, note),
                            None => self.send_midi(frame, [0x80 | ch, note, 0]),
                        }
                    }
                },
            }
        }
    }

    /// A native loop's chunk: notes on and off on their frames, program
    /// changes between them, every sounding instrument mixed.
    fn render_native(&mut self, gain: f32, l: &mut [f32], r: &mut [f32], frames: usize, t: &Tempo) {
        if self.natives.is_empty() {
            return;
        }
        let mut natives = std::mem::take(&mut self.natives);
        if t.moving() {
            let beat1 = self.take_events(frames, t);
            for (index, (off, ..)) in self.native_offs.iter().enumerate() {
                if *off < beat1 && self.nacts < MAX_LOOP_ACTS {
                    let f = (t.frame_of(*off) - 1e-6).ceil().max(0.0) as usize;
                    self.acts[self.nacts] = Act { frame: f.min(frames - 1) as u16, event: None, release: index as u8 };
                    self.nacts += 1;
                }
            }
        }
        self.acts[..self.nacts].sort_unstable_by_key(|a| (a.frame, a.event.is_some()));
        let mut at = 0usize;
        for a in 0..=self.nacts {
            let end = if a < self.nacts { self.acts[a].frame as usize } else { frames };
            if end > at {
                let n = end - at;
                for (_, inst) in natives.iter_mut() {
                    if !inst.is_active() {
                        continue;
                    }
                    let (bl, br) = (&mut self.native_l[..n], &mut self.native_r[..n]);
                    inst.render(bl, br);
                    for i in 0..n {
                        l[at + i] += bl[i] * gain;
                        r[at + i] += br[i] * gain;
                    }
                }
                at = end;
            }
            if a == self.nacts {
                break;
            }
            let act = self.acts[a];
            match act.event {
                None => {
                    let (off, note, inst) = self.native_offs[act.release as usize];
                    if off.is_finite() {
                        self.native_offs[act.release as usize] = (f64::INFINITY, 0, 0);
                        if let Some((_, i)) = natives.get_mut(inst as usize) {
                            i.note_off(note);
                        }
                    }
                }
                Some(event) => {
                    let note = match event.kind {
                        EventKind::Program(program) => {
                            if let Some(index) = natives.iter().position(|(p, _)| *p == program) {
                                self.native_current = index;
                            }
                            continue;
                        }
                        EventKind::Pedal => {
                            // One foot on every instrument the loop plays.
                            for (_, inst) in natives.iter_mut() {
                                inst.control(Control::Sustain, event.vel.clamp(0.0, 1.0));
                            }
                            continue;
                        }
                        EventKind::Glide => continue,
                        EventKind::Hit(_, gm) => gm,
                        _ => event.note.round().clamp(0.0, 127.0) as u8,
                    };
                    let current = self.native_current.min(natives.len() - 1);
                    let velocity = (event.vel * 127.0).round().clamp(1.0, 127.0) as u8;
                    // The same note held on this instrument ends first.
                    for off in self.native_offs.iter_mut().filter(|(o, n, i)| o.is_finite() && *n == note && *i as usize == current) {
                        natives[current].1.note_off(note);
                        *off = (f64::INFINITY, 0, 0);
                    }
                    natives[current].1.note_on(note, velocity);
                    let len = if event.dur > 0.0 { event.dur as f64 } else { HELD_DEFAULT_BEATS };
                    match self.native_offs.iter_mut().find(|(o, ..)| !o.is_finite()) {
                        Some(slot) => *slot = (event.beat + len, note, current as u8),
                        None => self.dropped += 1,
                    }
                }
            }
        }
        self.natives = natives;
    }

    /// A sung loop's chunk: each syllable queued on its frame, then the
    /// choir renders the chunk.
    fn render_voice(&mut self, l: &mut [f32], r: &mut [f32], frames: usize, rate: f32, t: &Tempo) {
        let Some(mut choir) = self.choir.take() else { return };
        if t.moving() {
            let beat1 = self.take_events(frames, t);
            for (index, (off, _)) in self.choir_offs.iter().enumerate() {
                if *off < beat1 && self.nacts < MAX_LOOP_ACTS {
                    let f = (t.frame_of(*off) - 1e-6).ceil().max(0.0) as usize;
                    self.acts[self.nacts] = Act { frame: f.min(frames - 1) as u16, event: None, release: index as u8 };
                    self.nacts += 1;
                }
            }
        }
        self.acts[..self.nacts].sort_unstable_by_key(|a| (a.frame, a.event.is_some()));
        let seconds_per_beat = if t.moving() { 1.0 / (t.per_frame * rate as f64) } else { 0.5 };
        for a in 0..self.nacts {
            let act = self.acts[a];
            match act.event {
                None => {
                    let (_, serial) = self.choir_offs[act.release as usize];
                    self.choir_offs[act.release as usize] = (f64::INFINITY, 0);
                    choir.release(act.frame as u32, serial);
                }
                Some(event) => {
                    if matches!(event.kind, EventKind::Program(_) | EventKind::Pedal | EventKind::Glide) {
                        continue;
                    }
                    // A played note on a voice sings "ah".
                    let (part, syl) = match event.kind {
                        EventKind::Sung(part, syl) => (part, syl),
                        _ => (0, PhonemeSeq::ipa("ɑ")),
                    };
                    let len = if event.dur > 0.0 { event.dur as f64 } else { 1.0 };
                    let note = SungNote { note: event.note, vel: event.vel.clamp(0.0, 1.0), dur: (len * seconds_per_beat) as f32, syl };
                    let serial = choir.note_on(act.frame as u32, note, part);
                    match self.choir_offs.iter_mut().find(|(off, _)| !off.is_finite()) {
                        Some(slot) => *slot = (event.beat + len, serial),
                        None => {
                            choir.release(act.frame as u32, serial);
                            self.dropped += 1;
                        }
                    }
                }
            }
        }
        choir.render_stereo(&mut l[..frames], &mut r[..frames]);
        self.choir = Some(choir);
    }

    /// A neural singer's chunk: each phrase event starts a play at its
    /// (fractional) frame; every play adds its audio, resampled to `rate`,
    /// at its event's level. A play whose audio is not rendered yet asks for
    /// it each chunk and keeps its place, so it joins where it has reached.
    #[cfg(not(target_arch = "wasm32"))]
    fn render_neural(&mut self, singer: &NeuralSinger, l: &mut [f32], r: &mut [f32], frames: usize, rate: f32, t: &Tempo) {
        let step = makepad_score_voice::neural::RATE as f64 / rate as f64;
        if !t.moving() {
            for play in self.phrases.iter_mut() {
                *play = None;
            }
            self.take_events(frames, t);
            return;
        }
        self.take_events(frames, t);
        for a in 0..self.nacts {
            let Some(event) = self.acts[a].event else { continue };
            let EventKind::Phrase(key) = event.kind else { continue };
            // Where in the phrase this chunk's first frame is (in its samples).
            let pos = -t.frame_of(event.beat) * step;
            // A start on a frame is on it (not a rounding error before it).
            let pos = if (pos - pos.round()).abs() < 1e-6 { pos.round() } else { pos };
            match self.phrases.iter_mut().find(|p| p.is_none()) {
                Some(slot) => *slot = Some(PhrasePlay { key, audio: None, pos, gain: event.vel.clamp(0.0, 4.0) }),
                None => self.dropped += 1,
            }
        }
        for slot in self.phrases.iter_mut() {
            let Some(play) = slot else { continue };
            if play.audio.is_none() {
                play.audio = singer.get(play.key);
            }
            match &play.audio {
                Some(audio) => {
                    let n = audio.len();
                    for i in 0..frames {
                        let p = play.pos + i as f64 * step;
                        if p < 0.0 {
                            continue;
                        }
                        let k = p as usize;
                        if k + 1 >= n {
                            break;
                        }
                        let f = (p - k as f64) as f32;
                        let v = (audio[k] * (1.0 - f) + audio[k + 1] * f) * play.gain;
                        l[i] += v;
                        r[i] += v;
                    }
                    play.pos += frames as f64 * step;
                    if play.pos >= n as f64 {
                        *slot = None;
                    }
                }
                None => {
                    play.pos += frames as f64 * step;
                    // Never arrived (no voice, or far behind): give up after a minute.
                    if play.pos > 60.0 * makepad_score_voice::neural::RATE as f64 {
                        *slot = None;
                    }
                }
            }
        }
    }

    /// A shader loop's chunk: its notes on and off on their frames.
    fn render_shader(&mut self, params: &Arc<[(u16, f32)]>, mono: bool, l: &mut [f32], r: &mut [f32], frames: usize, t: &Tempo) {
        let Some(mut shader) = self.shader.take() else { return };
        let address = Arc::as_ptr(params) as *const u8 as usize;
        if self.shader_params != address {
            for (index, value) in params.iter() {
                shader.set_param(*index as usize, *value);
            }
            self.shader_params = address;
        }
        if t.moving() {
            let beat1 = self.take_events(frames, t);
            for (index, (off, _)) in self.shader_voices.iter().enumerate() {
                if *off < beat1 && self.nacts < MAX_LOOP_ACTS {
                    let f = (t.frame_of(*off) - 1e-6).ceil().max(0.0) as usize;
                    self.acts[self.nacts] = Act { frame: f.min(frames - 1) as u16, event: None, release: index as u8 };
                    self.nacts += 1;
                }
            }
        }
        self.acts[..self.nacts].sort_unstable_by_key(|a| (a.frame, a.event.is_some()));
        let mut t = 0usize;
        for a in 0..=self.nacts {
            let end = if a < self.nacts { self.acts[a].frame as usize } else { frames };
            if end > t {
                shader.render(&mut l[t..end], &mut r[t..end]);
                t = end;
            }
            if a < self.nacts {
                let act = self.acts[a];
                match act.event {
                    None => {
                        let voice = act.release;
                        if self.shader_voices[voice as usize].0.is_finite() {
                            self.shader_voices[voice as usize].0 = f64::INFINITY;
                            shader.note_off(voice);
                        }
                    }
                    Some(event) => {
                        if matches!(event.kind, EventKind::Program(_) | EventKind::Pedal) {
                            continue;
                        }
                        // A glide on a shader voice: a legato to its new
                        // note (the shader slides it).
                        if let EventKind::Glide = event.kind {
                            if event.note.is_finite() && event.opts.handle != 0 {
                                if let Some(v) = self.shader_handles.iter().position(|h| *h == event.opts.handle) {
                                    let vel = if event.vel.is_finite() { event.vel } else { 0.8 };
                                    shader.legato(event.note, vel, v as u8);
                                }
                            }
                            continue;
                        }
                        let note = match event.kind {
                            EventKind::Hit(..) => 60.0 + event.note,
                            _ => event.note,
                        };
                        let len = if event.dur > 0.0 { event.dur as f64 } else { HELD_DEFAULT_BEATS };
                        let serial = self.serial;
                        self.serial = self.serial.wrapping_add(1);
                        let voice = if mono {
                            shader.legato(note, event.vel, 0);
                            0
                        } else {
                            let voice = (0..self.shader_voices.len()).find(|&v| !shader.active(v as u8))
                                .unwrap_or_else(|| (0..self.shader_voices.len()).min_by_key(|&v| self.shader_voices[v].1).unwrap_or(0));
                            shader.note_on(note, event.vel, voice as u8);
                            voice
                        };
                        self.shader_voices[voice] = (event.beat + len, serial);
                        self.shader_handles[voice] = event.opts.handle;
                    }
                }
            }
        }
        self.shader = Some(shader);
    }

    /// A Morph loop's chunk: its notes on and off on their frames (as an
    /// Ironfish loop's), on the pass's tempo.
    #[allow(clippy::too_many_arguments)]
    fn render_morph(&mut self, patch: &Arc<MorphPatch>, src: &Arc<MorphSources>, l: &mut [f32], r: &mut [f32], frames: usize, rate: f32, t: &Tempo) {
        let Some(mut morph) = self.morph.take() else { return };
        if !Arc::ptr_eq(morph.patch(), patch) {
            morph.set_patch(patch.clone(), src.clone());
        }
        if t.per_frame > 0.0 {
            morph.set_tempo((t.per_frame * rate as f64) as f32);
        }
        if t.moving() {
            let beat1 = self.take_events(frames, t);
            for (index, (off, _)) in self.iron_offs.iter().enumerate() {
                if *off < beat1 && self.nacts < MAX_LOOP_ACTS {
                    let f = (t.frame_of(*off) - 1e-6).ceil().max(0.0) as usize;
                    self.acts[self.nacts] = Act { frame: f.min(frames - 1) as u16, event: None, release: index as u8 };
                    self.nacts += 1;
                }
            }
        }
        self.acts[..self.nacts].sort_unstable_by_key(|a| (a.frame, a.event.is_some()));
        let mut at = 0usize;
        for a in 0..=self.nacts {
            let end = if a < self.nacts { self.acts[a].frame as usize } else { frames };
            if end > at {
                morph.render(&mut l[at..end], &mut r[at..end]);
                at = end;
            }
            if a < self.nacts {
                let act = self.acts[a];
                match act.event {
                    None => {
                        let (_, note) = self.iron_offs[act.release as usize];
                        self.iron_offs[act.release as usize] = (f64::INFINITY, 0);
                        morph.note_off(note);
                    }
                    Some(event) => {
                        if matches!(event.kind, EventKind::Program(_) | EventKind::Pedal) {
                            continue;
                        }
                        let note = event.note.round().clamp(0.0, 127.0) as u8;
                        let velocity = (event.vel * 127.0).round().clamp(1.0, 127.0) as u8;
                        for off in self.iron_offs.iter_mut().filter(|(_, held)| *held == note) {
                            if off.0.is_finite() {
                                morph.note_off(note);
                                *off = (f64::INFINITY, 0);
                            }
                        }
                        morph.note_on(note, velocity);
                        let len = if event.dur > 0.0 { event.dur as f64 } else { HELD_DEFAULT_BEATS };
                        match self.iron_offs.iter_mut().find(|(off, _)| !off.is_finite()) {
                            Some(slot) => *slot = (event.beat + len, note),
                            None => {
                                morph.note_off(note);
                                self.dropped += 1;
                            }
                        }
                    }
                }
            }
        }
        self.morph = Some(morph);
    }

    /// A Tandem loop's chunk: its notes on and off on their frames (as a
    /// Morph loop's), on the pass's tempo.
    fn render_tandem(&mut self, prog: &Arc<TandemProgram>, l: &mut [f32], r: &mut [f32], frames: usize, t: &Tempo) {
        let Some(mut synth) = self.tandem.take() else { return };
        if !Arc::ptr_eq(synth.program(), prog) {
            synth.set_program(prog.clone());
        }
        if t.per_frame > 0.0 {
            synth.set_tempo(t.per_frame * synth.sample_rate() as f64 * 60.0);
        }
        if t.moving() {
            let beat1 = self.take_events(frames, t);
            for (index, (off, _)) in self.iron_offs.iter().enumerate() {
                if *off < beat1 && self.nacts < MAX_LOOP_ACTS {
                    let f = (t.frame_of(*off) - 1e-6).ceil().max(0.0) as usize;
                    self.acts[self.nacts] = Act { frame: f.min(frames - 1) as u16, event: None, release: index as u8 };
                    self.nacts += 1;
                }
            }
        }
        self.acts[..self.nacts].sort_unstable_by_key(|a| (a.frame, a.event.is_some()));
        let mut at = 0usize;
        for a in 0..=self.nacts {
            let end = if a < self.nacts { self.acts[a].frame as usize } else { frames };
            if end > at {
                synth.render(&mut l[at..end], &mut r[at..end]);
                at = end;
            }
            if a < self.nacts {
                let act = self.acts[a];
                match act.event {
                    None => {
                        let (_, note) = self.iron_offs[act.release as usize];
                        self.iron_offs[act.release as usize] = (f64::INFINITY, 0);
                        synth.note_off(0, note, 64);
                    }
                    Some(event) => {
                        if matches!(event.kind, EventKind::Program(_) | EventKind::Pedal) {
                            continue;
                        }
                        let note = event.note.round().clamp(0.0, 127.0) as u8;
                        let velocity = (event.vel * 127.0).round().clamp(1.0, 127.0) as u8;
                        for off in self.iron_offs.iter_mut().filter(|(_, held)| *held == note) {
                            if off.0.is_finite() {
                                synth.note_off(0, note, 64);
                                *off = (f64::INFINITY, 0);
                            }
                        }
                        synth.note_on(0, note, velocity);
                        let len = if event.dur > 0.0 { event.dur as f64 } else { HELD_DEFAULT_BEATS };
                        match self.iron_offs.iter_mut().find(|(off, _)| !off.is_finite()) {
                            Some(slot) => *slot = (event.beat + len, note),
                            None => {
                                synth.note_off(0, note, 64);
                                self.dropped += 1;
                            }
                        }
                    }
                }
            }
        }
        self.tandem = Some(synth);
    }

    /// An Ironfish loop's chunk: its notes on and off on their frames.
    fn render_iron(&mut self, patch: &IronfishPatch, l: &mut [f32], r: &mut [f32], frames: usize, t: &Tempo) {
        let Some(mut iron) = self.iron.take() else { return };
        if iron.patch() != patch.sanitise() {
            iron.set_patch(*patch);
        }
        if t.moving() {
            let beat1 = self.take_events(frames, t);
            for (index, (off, _)) in self.iron_offs.iter().enumerate() {
                if *off < beat1 && self.nacts < MAX_LOOP_ACTS {
                    let f = (t.frame_of(*off) - 1e-6).ceil().max(0.0) as usize;
                    self.acts[self.nacts] = Act { frame: f.min(frames - 1) as u16, event: None, release: index as u8 };
                    self.nacts += 1;
                }
            }
        }
        // Note offs first on a shared frame, so a repeated note restarts.
        self.acts[..self.nacts].sort_unstable_by_key(|a| (a.frame, a.event.is_some()));
        let mut t = 0usize;
        for a in 0..=self.nacts {
            let end = if a < self.nacts { self.acts[a].frame as usize } else { frames };
            while t < end {
                let [fl, fr] = iron.next_frame();
                l[t] = fl;
                r[t] = fr;
                t += 1;
            }
            if a < self.nacts {
                let act = self.acts[a];
                match act.event {
                    None => {
                        let (_, note) = self.iron_offs[act.release as usize];
                        self.iron_offs[act.release as usize] = (f64::INFINITY, 0);
                        iron.note_off(note);
                    }
                    Some(event) => {
                        if matches!(event.kind, EventKind::Program(_) | EventKind::Pedal) {
                            continue;
                        }
                        let note = event.note.round().clamp(0.0, 127.0) as u8;
                        let velocity = (event.vel * 127.0).round().clamp(1.0, 127.0) as u8;
                        // A note already held ends where this one starts.
                        for off in self.iron_offs.iter_mut().filter(|(_, held)| *held == note) {
                            if off.0.is_finite() {
                                iron.note_off(note);
                                *off = (f64::INFINITY, 0);
                            }
                        }
                        iron.note_on(note, velocity);
                        let len = if event.dur > 0.0 { event.dur as f64 } else { HELD_DEFAULT_BEATS };
                        match self.iron_offs.iter_mut().find(|(off, _)| !off.is_finite()) {
                            Some(slot) => *slot = (event.beat + len, note),
                            None => {
                                iron.note_off(note);
                                self.dropped += 1;
                            }
                        }
                    }
                }
            }
        }
        self.iron = Some(iron);
    }

    /// Render `frames` frames of `inst` on the pass's tempo `t` (not
    /// moving when stopped: nothing new starts),
    /// OVERWRITING `l`/`r`. The instrument belongs to the caller (a new
    /// revision's patch plays the voices it starts from then on).
    pub fn render(&mut self, inst: &Inst, l: &mut [f32], r: &mut [f32], rate: f32, t: &Tempo) {
        let per_frame = t.per_frame;
        let frames = l.len().min(r.len()).min(MAX_LOOP_FRAMES);
        l[..frames].fill(0.0);
        r[..frames].fill(0.0);
        self.nacts = 0;
        match inst {
            Inst::Ironfish { patch, .. } => return self.render_iron(patch, l, r, frames, t),
            Inst::Morph { patch, src, .. } => return self.render_morph(patch, src, l, r, frames, rate, t),
            Inst::Tandem { prog, .. } => return self.render_tandem(prog, l, r, frames, t),
            Inst::Shader { params, mono, .. } => return self.render_shader(params, *mono, l, r, frames, t),
            Inst::Voice { .. } => return self.render_voice(l, r, frames, rate, t),
            #[cfg(not(target_arch = "wasm32"))]
            Inst::Neural { singer, .. } => return self.render_neural(singer, l, r, frames, rate, t),
            Inst::Native { gain, .. } => return self.render_native(*gain, l, r, frames, t),
            Inst::Midi { channel, .. } => return self.render_midi(*channel, frames, t),
            Inst::Input { .. } => {
                // Its notes play nothing; they are dropped as they come.
                if t.moving() {
                    self.take_events(frames, t);
                }
                return;
            }
            _ => {}
        }
        if t.moving() {
            let beat1 = self.take_events(frames, t);
            let frame_of = |beat: f64| -> f64 { (t.frame_of(beat) - 1e-6).ceil() };
            for (index, voice) in self.voices.iter().enumerate() {
                if voice.active && voice.off_beat.is_finite() && voice.off_beat < beat1 && self.nacts < MAX_LOOP_ACTS {
                    let f = frame_of(voice.off_beat).max(0.0) as usize;
                    self.acts[self.nacts] = Act { frame: f.min(frames - 1) as u16, event: None, release: index as u8 };
                    self.nacts += 1;
                }
            }
        }
        let acts = &mut self.acts[..self.nacts];
        acts.sort_unstable_by_key(|a| a.frame);
        let mut t = 0usize;
        for a in 0..=self.nacts {
            let end = if a < self.nacts { self.acts[a].frame as usize } else { frames };
            if end > t {
                for voice in self.voices.iter_mut().filter(|voice| voice.active) {
                    let sample = match inst {
                        Inst::Sampler { sample, .. } => Some(&**sample),
                        Inst::Drums { slots, .. } => slots.get(voice.slot as usize).and_then(|slot| slot.sample.as_deref()),
                        _ => None,
                    };
                    voice.render(&mut l[t..end], &mut r[t..end], &mut self.scratch, rate, sample);
                }
                t = end;
            }
            if a < self.nacts {
                let act = self.acts[a];
                match act.event {
                    None => self.voices[act.release as usize].release(),
                    Some(event) => {
                        self.start(inst, &event, rate, per_frame);
                        let o = &event.opts;
                        self.last_opts = [o.cutoff, o.res, o.attack, o.decay, o.sustain, o.release, o.drive];
                    }
                }
            }
        }
    }

    fn pick(&self, note: f32, mono: bool) -> usize {
        if mono {
            return 0;
        }
        self.voices.iter().position(|voice| voice.active && !voice.released && voice.note == note && voice.slot == u8::MAX)
            .or_else(|| self.voices.iter().position(|voice| !voice.active))
            .unwrap_or_else(|| (0..self.voices.len()).min_by_key(|&i| self.voices[i].serial).unwrap_or(0))
    }

    fn start(&mut self, inst: &Inst, event: &Event, rate: f32, per_frame: f64) {
        if matches!(event.kind, EventKind::Program(_) | EventKind::Pedal) {
            return;
        }
        if let EventKind::Glide = event.kind {
            // A playing note of this loop moves (its handle names it).
            let frames = if per_frame > 0.0 { (event.dur as f64 / per_frame) as f32 } else { 1.0 };
            let target = event.opts.handle;
            for voice in self.voices.iter_mut().filter(|v| v.active && v.handle == target && target != 0) {
                voice.glide_to(event.note, event.opts.glide_from, event.vel, event.opts.cutoff, frames);
            }
            return;
        }
        let seed = mix_seed(self.key, event.beat, event.note);
        let (pl, pr) = pan_gains(event.pan);
        let serial = self.serial;
        self.serial = self.serial.wrapping_add(1);
        let held = event.dur > 0.0;
        // A looping sample ends when it is let go, so it is always let go.
        let off_beat = if held { event.beat + event.dur as f64 } else if event.opts.looped { event.beat + 4.0 } else { f64::INFINITY };
        let slot = match (inst, event.kind) {
            (Inst::Synth { recipe, mono, .. }, EventKind::Note | EventKind::Sample | EventKind::Sung(..)) => {
                let recipe = with_opts(self.live_recipe.as_ref().unwrap_or(recipe), &event.opts);
                let mono = *mono;
                let index = self.pick(event.note, mono);
                if mono {
                    for voice in self.voices.iter_mut().skip(1) {
                        voice.release();
                    }
                }
                self.voices[index].start_note(&recipe, event.note, held, seed);
                index
            }
            (Inst::Drums { slots, .. }, kind) => {
                let (drum, semis) = match kind {
                    EventKind::Hit(drum, _) => (drum, event.note),
                    _ => match gm_drum(event.note) {
                        Some(drum) => (drum, 0.0),
                        None => return,
                    },
                };
                let Some(entry) = slots.get(drum as usize).cloned() else { return };
                for (closer, open) in CHOKES {
                    if drum as usize == closer {
                        for voice in self.voices.iter_mut().filter(|voice| voice.active && voice.slot as usize == open) {
                            voice.release();
                        }
                    }
                }
                let index = self.pick(f32::NAN, false);
                let pitch = entry.pitch * 2f32.powf(semis / 12.0);
                if let Some(sample) = &entry.sample {
                    let play = SamplePlay { step: pitch as f64 * sample.rate as f64 / rate as f64, from: 0.0, to: sample.len() as f64, looped: false, time_step: f64::NAN, grain: 0.0, sustain: None };
                    self.voices[index].start_sample(play, drum, 0.0, 0.02, 60.0);
                } else if let Some(recipe) = &entry.recipe {
                    let recipe = with_opts(recipe, &event.opts);
                    self.voices[index].start_hit(&recipe, pitch, drum, seed);
                } else {
                    return;
                }
                let voice = &mut self.voices[index];
                voice.gain = event.vel * entry.gain;
                voice.vel = event.vel;
                voice.env = event.opts.env;
                voice.set_moves(&event.opts);
                voice.cutoff_env = event.opts.cutoff_env;
                voice.beats_per_frame = per_frame as f32;
                voice.pan_l = pl;
                voice.pan_r = pr;
                voice.serial = serial;
                voice.handle = event.opts.handle;
                return;
            }
            (Inst::Sampler { sample, root, attack, release, .. }, EventKind::Note | EventKind::Sample | EventKind::Sung(..)) => {
                let o = &event.opts;
                let semis = if event.note.is_finite() { event.note - *root } else { 0.0 };
                let speed = 2f64.powf(semis as f64 / 12.0) * if o.rate.is_finite() && o.rate != 0.0 { o.rate as f64 } else { 1.0 };
                let (from, to) = sample.region(o);
                let dir = speed.signum();
                // Beats to output frames at the tempo it starts at.
                let frames_for = |beats: f32| if per_frame > 0.0 { beats.max(1e-3) as f64 / per_frame } else { f64::NAN };
                let mut play = SamplePlay { step: speed * sample.rate as f64 / rate as f64, from, to, looped: o.looped, time_step: f64::NAN, grain: 0.0, sustain: None };
                // The sample's own loop, when the note plays it forwards
                // from where it starts to past the loop's start.
                if let Some((a, b)) = sample.sustain.filter(|&(a, b)| !o.looped && speed > 0.0 && from <= a as f64 && to >= b as f64) {
                    play.sustain = Some((a as f64, b as f64));
                }
                if o.fit.is_finite() && per_frame > 0.0 {
                    play.step = dir * (to - from) / frames_for(o.fit);
                } else if o.stretch.is_finite() && per_frame > 0.0 {
                    play.time_step = dir * (to - from) / frames_for(o.stretch);
                    play.grain = (0.05 * sample.rate as f64).max(64.0);
                }
                let (attack, release) = (*attack, *release);
                let index = self.pick(event.note, false);
                self.voices[index].start_sample(play, u8::MAX, attack, release, event.note);
                index
            }
            _ => return,
        };
        let voice = &mut self.voices[slot];
        voice.off_beat = off_beat;
        voice.gain = event.vel;
        voice.vel = event.vel;
        voice.env = event.opts.env;
        voice.set_moves(&event.opts);
        voice.cutoff_env = event.opts.cutoff_env;
        voice.beats_per_frame = per_frame as f32;
        voice.pan_l = pl;
        voice.pan_r = pr;
        voice.serial = serial;
        voice.handle = event.opts.handle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_audio_synth::recipe::Recipe;

    fn inst() -> Inst {
        let recipe = Recipe::parse("pluck=0.35 freq=440 a=0.001 d=1.6 s=0 r=0.2 gain=0.45; saw freq=440 detune=6 a=0.001 d=0.16 s=0 r=0.06 gain=0.13 lp=9000>1200 q=1.5").unwrap();
        Inst::Synth { key: 2, recipe, mono: false }
    }

    #[test]
    fn a_note_starts_on_its_frame_whatever_the_chunking() {
        let render = |sizes: &[usize]| {
            let (mut e, inst) = (LoopEngine::new(1), inst());
            e.push(Event { beat: 0.5, loop_key: 1, kind: EventKind::Note, note: 60.0, vel: 1.0, pan: 0.0, dur: 0.25, opts: Default::default() });
            let bpf = 2.0 / 48_000.0; // 120 BPM
            let mut out = Vec::new();
            let (mut frame, mut i) = (0usize, 0usize);
            while frame < 24_000 {
                let n = sizes[i % sizes.len()].min(24_000 - frame);
                let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
                e.render(&inst, &mut l, &mut r, 48_000.0, &Tempo::linear(frame as f64 * bpf, bpf));
                out.extend(l);
                frame += n;
                i += 1;
            }
            out
        };
        let a = render(&[256]);
        assert_eq!(a.iter().position(|x| x.abs() > 1e-7), Some(12_000));
        assert_eq!(a, render(&[100, 256, 37, 1]));
    }

    #[test]
    fn a_neural_phrase_plays_from_its_frame_and_a_late_one_joins_where_it_has_reached() {
        use makepad_score_voice::lyrics::{builtin_pronounce, lower_sing, NoteIn, SingOpts};
        use makepad_score_voice::neural::{self, makepad_ai_sing as sing, NeuralSinger};
        let cantor = sing::Cantor::random(sing::acoustic::AcousticConfig::tiny(), sing::vocoder::VocoderConfig::tiny(), 1);
        let singer = NeuralSinger::new(cantor);
        let phrase = |words: &str| {
            let notes: Vec<NoteIn> = (0..2).map(|i| NoteIn { beat: i as f64, dur: 1.0, pitches: vec![64.0], vel: 0.8 }).collect();
            neural::phrase_of(&lower_sing(&notes, words, &SingOpts::default(), &builtin_pronounce), 120.0, neural::SINGER).unwrap()
        };
        let bpf = 2.0 / 48_000.0; // 120 BPM
        let play = |key: u64, at_frame: usize, arrive_at: Option<(usize, &dyn Fn())>| {
            let inst = Inst::Neural { key: 9, singer: singer.clone() };
            let mut e = LoopEngine::for_inst(9, &inst, 48_000.0, false).unwrap();
            e.push(Event { beat: at_frame as f64 * bpf, loop_key: 9, kind: EventKind::Phrase(key), note: 0.0, vel: 0.5, pan: 0.0, dur: 0.0, opts: Default::default() });
            let mut out = Vec::new();
            let mut frame = 0usize;
            while frame < 96_000 {
                if let Some((at, arrive)) = &arrive_at {
                    if frame == *at {
                        arrive();
                    }
                }
                let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
                e.render(&inst, &mut l, &mut r, 48_000.0, &Tempo::linear(frame as f64 * bpf, bpf));
                assert_eq!(l, r);
                out.extend(l);
                frame += 256;
            }
            out
        };
        // Rendered before it plays: it starts on its frame (100 frames into a pass).
        let (key, score, _) = phrase("la la");
        let audio = singer.render_now(key, &score);
        let out = play(key, 12_100, None);
        assert!(out[..12_100].iter().all(|v| *v == 0.0));
        for k in [0usize, 1, 500, 20_000] {
            assert!((out[12_100 + k] - 0.5 * audio[k]).abs() < 1e-6, "sample {k}");
        }
        // Rendered only after it should have begun: it joins at the place it has reached.
        let (late, late_score, _) = phrase("me me");
        let late_audio = sing::Cantor::random(sing::acoustic::AcousticConfig::tiny(), sing::vocoder::VocoderConfig::tiny(), 1).render(&late_score, &sing::RenderOpts::default()).0;
        let arrive = || {
            singer.render_now(late, &late_score);
        };
        let out = play(late, 12_100, Some((24_064, &arrive)));
        assert_ne!(late, key);
        assert!(out[..24_064].iter().all(|v| *v == 0.0), "silent until it arrives: first sound at {:?}", out.iter().position(|v| *v != 0.0));
        for k in [24_064usize, 30_000, 40_000] {
            assert!((out[k] - 0.5 * neural::LEVEL * late_audio[k - 12_100]).abs() < 1e-6, "frame {k}");
        }
        assert!(Arc::ptr_eq(&audio, &singer.get(key).unwrap()));
    }

    #[test]
    fn a_midi_loop_sends_its_notes_on_their_frames() {
        let inst = Inst::Midi { key: 4, channel: 1, port: 0 };
        let mut e = LoopEngine::for_inst(4, &inst, 48_000.0, false).unwrap();
        let note = |beat: f64, note: f32, dur: f32| Event { beat, loop_key: 4, kind: EventKind::Note, note, vel: 1.0, pan: 0.0, dur, opts: Default::default() };
        e.push(note(0.5, 60.0, 0.25));
        e.push(Event { kind: EventKind::Program(40), ..note(0.25, 0.0, 0.0) });
        e.push(note(1.0, 64.0, 8.0));
        e.push(Event { kind: EventKind::Release, ..note(1.5, 64.0, 0.0) });
        let bpf = 2.0 / 48_000.0;
        let mut sent = Vec::new();
        let mut frame = 0usize;
        while frame < 48_000 {
            let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
            e.render(&inst, &mut l, &mut r, 48_000.0, &Tempo::linear(frame as f64 * bpf, bpf));
            assert!(l.iter().all(|x| *x == 0.0));
            e.take_midi(|f, data| sent.push((frame + f as usize, data)));
            frame += 256;
        }
        assert_eq!(sent, [
            (6_000, [0xc1, 40, 0]),
            (12_000, [0x91, 60, 127]),
            (18_000, [0x81, 60, 0]),
            (24_000, [0x91, 64, 127]),
            // Let go at 1.5 (a key released), not after its 8 beats.
            (36_000, [0x81, 64, 0]),
        ]);
    }

    #[test]
    fn a_released_key_ends_its_note() {
        let voices_after = |release: bool| {
            let recipe = Recipe::parse("saw freq=440 detune=6 a=0.9 d=1.5 s=0.8 r=2.4 gain=0.13 lp=1500>3000 glide=3; sine freq=220 a=1.0 d=1.5 s=0.9 r=2.4 gain=0.13").unwrap();
            let inst = Inst::Synth { key: 1, recipe, mono: false };
            let mut e = LoopEngine::new(1);
            let opts = crate::event::NoteOpts { handle: 0x8000_0000 | 60, ..Default::default() };
            e.push(Event { beat: 0.0, loop_key: 1, kind: EventKind::Note, note: 60.0, vel: 1.0, pan: 0.0, dur: 64.0, opts });
            if release {
                e.push(Event { beat: 0.5, loop_key: 1, kind: EventKind::Release, note: 60.0, vel: 0.0, pan: 0.0, dur: 0.0, opts });
            }
            let bpf = 2.0 / 48_000.0;
            let mut frame = 0usize;
            while frame < 48_000 * 6 {
                let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
                e.render(&inst, &mut l, &mut r, 48_000.0, &Tempo::linear(frame as f64 * bpf, bpf));
                frame += 256;
            }
            e.voices()
        };
        assert_eq!((voices_after(false), voices_after(true)), (1, 0));
    }

    #[test]
    fn a_new_shader_program_takes_over_the_sounding_voices() {
        let program = |code: &str| {
            let shader = crate::shader::compile(code).unwrap_or_else(|e| panic!("{e:?}"));
            Inst::Shader { key: 3, shader, params: Arc::new([]), mono: false }
        };
        let code = crate::shader::library("epiano").unwrap();
        let (a, b) = (program(code), program(&code.replace("0.", "0.0")));
        let bpf = 2.0 / 48_000.0;
        let mut kept = LoopEngine::for_inst(3, &a, 48_000.0, false).unwrap();
        kept.push(Event { beat: 0.0, loop_key: 3, kind: EventKind::Note, note: 60.0, vel: 1.0, pan: 0.0, dur: 4.0, opts: Default::default() });
        let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
        for chunk in 0..8 {
            kept.render(&a, &mut l, &mut r, 48_000.0, &Tempo::linear(chunk as f64 * 256.0 * bpf, bpf));
        }
        assert_eq!(kept.voices(), 1);
        let mut fresh = LoopEngine::for_inst(3, &b, 48_000.0, false).unwrap();
        kept.take_program(&mut fresh);
        let Inst::Shader { shader, .. } = &b else { unreachable!() };
        assert!(kept.shader.as_ref().unwrap().shader().ptr_eq(shader));
        kept.render(&b, &mut l, &mut r, 48_000.0, &Tempo::linear(8.0 * 256.0 * bpf, bpf));
        assert_eq!(kept.voices(), 1, "the note plays on in the new program");
        assert!(l.iter().any(|x| x.abs() > 1e-4));
    }

    fn render_all(e: &mut LoopEngine, inst: &Inst, frames: usize, bpf: f64, sizes: &[usize]) -> Vec<f32> {
        let mut out = Vec::new();
        let (mut frame, mut i) = (0usize, 0usize);
        while frame < frames {
            let n = sizes[i % sizes.len()].min(frames - frame);
            let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
            e.render(inst, &mut l, &mut r, 48_000.0, &Tempo::linear(frame as f64 * bpf, bpf));
            out.extend(l);
            frame += n;
            i += 1;
        }
        out
    }

    #[test]
    fn native_instruments_play_notes_hits_and_program_changes_on_their_frames() {
        let bpf = 2.0 / 48_000.0;
        let note = |beat: f64, kind: EventKind, note: f32| Event { beat, loop_key: 4, kind, note, vel: 0.9, pan: 0.0, dur: 0.5, opts: Default::default() };
        // The modelled drum kit: a pedal hat and a floor tom by GM note.
        let drums = Inst::Native { key: 4, kinds: Arc::new([(0, InstrumentKind::Drums)]), gain: 1.0 };
        let mut e = LoopEngine::for_inst(4, &drums, 48_000.0, false).unwrap();
        e.push(note(0.5, EventKind::Hit(3, 44), 0.0));
        e.push(note(1.0, EventKind::Hit(6, 43), 0.0));
        let out = render_all(&mut e, &drums, 36_000, bpf, &[256]);
        assert_eq!(out.iter().position(|x| x.abs() > 1e-6), Some(12_000));
        assert!(out[24_000..].iter().any(|x| x.abs() > 1e-3));
        // The modelled piano, low register included.
        let piano = Inst::Native { key: 5, kinds: Arc::new([(0, InstrumentKind::Piano { preset: None })]), gain: 1.0 };
        let mut e = LoopEngine::for_inst(5, &piano, 48_000.0, false).unwrap();
        e.push(note(0.0, EventKind::Note, 28.0));
        e.push(note(1.0, EventKind::Note, 60.0));
        let a = render_all(&mut e, &piano, 48_000, bpf, &[256]);
        assert!(a.iter().any(|x| x.abs() > 1e-3) && a.iter().all(|x| x.is_finite()));
        // Program changes pick the instrument the next notes play.
        let two = Inst::Native { key: 6, kinds: Arc::new([(0, InstrumentKind::Drums), (1, InstrumentKind::Piano { preset: None })]), gain: 1.0 };
        let mut e = LoopEngine::for_inst(6, &two, 48_000.0, false).unwrap();
        e.push(note(0.0, EventKind::Program(1), 0.0));
        e.push(note(0.0, EventKind::Note, 60.0));
        render_all(&mut e, &two, 4_800, bpf, &[256]);
        assert_eq!(e.native_current, 1);
    }

    #[test]
    fn the_pedal_holds_a_released_piano_note() {
        let bpf = 2.0 / 48_000.0;
        let ev = |beat: f64, kind: EventKind, vel: f32| Event { beat, loop_key: 7, kind, note: 60.0, vel, pan: 0.0, dur: 0.25, opts: Default::default() };
        let piano = Inst::Native { key: 7, kinds: Arc::new([(0, InstrumentKind::Piano { preset: None })]), gain: 1.0 };
        let tail = |pedal: Option<f32>| {
            let mut e = LoopEngine::for_inst(7, &piano, 48_000.0, false).unwrap();
            if let Some(v) = pedal {
                e.push(ev(0.0, EventKind::Pedal, v));
            }
            e.push(ev(0.0, EventKind::Note, 0.8));
            let out = render_all(&mut e, &piano, 72_000, bpf, &[256]);
            // A second after the key came up (beat 0.25 is frame 6000).
            out[54_000..].iter().map(|x| x * x).sum::<f32>()
        };
        let (damped, half, held) = (tail(None), tail(Some(0.5)), tail(Some(1.0)));
        assert!(held > damped * 4.0, "pedal down {held} vs damped {damped}");
        assert!(half > damped && half.is_finite(), "half pedal {half} vs damped {damped}");
    }

    #[test]
    fn a_tempo_change_inside_a_pass_moves_the_notes_after_it() {
        // 120 BPM, then 60 BPM from beat 1 (frame 24000): beat 1.5 lands a
        // half second (at 60) after it.
        let t = |frame: usize, n: usize| {
            let bpf = 2.0 / 48_000.0;
            let change = 24_000.0;
            let mut tempo = Tempo::linear(frame as f64 * bpf, bpf);
            if (frame as f64) < change && change < (frame + n) as f64 {
                tempo.splits[0] = (1.0, change - frame as f64, 1.0 / 48_000.0);
                tempo.nsplits = 1;
            } else if frame as f64 >= change {
                tempo = Tempo::linear(1.0 + (frame as f64 - change) / 48_000.0, 1.0 / 48_000.0);
            }
            tempo
        };
        let inst = inst();
        for size in [256usize, 100, 1000] {
            let mut e = LoopEngine::new(1);
            e.push(Event { beat: 1.5, loop_key: 1, kind: EventKind::Note, note: 60.0, vel: 1.0, pan: 0.0, dur: 0.25, opts: Default::default() });
            let mut out = Vec::new();
            let mut frame = 0;
            while frame < 60_000 {
                let n = size.min(60_000 - frame);
                let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
                e.render(&inst, &mut l, &mut r, 48_000.0, &t(frame, n));
                out.extend(l);
                frame += n;
            }
            assert_eq!(out.iter().position(|x| x.abs() > 1e-7), Some(48_000), "pass size {size}");
        }
    }
}
