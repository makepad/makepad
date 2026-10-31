//! A note stream on the audio thread: what a keyboard plays (MIDI in)
//! through a score's stream nodes (`Chord`, `Transpose`, `Velocity`, `Arp`)
//! into a loop's instrument, each message on its own beat of the pass.
//!
//! A pass hands the stream the messages that fall in it, already placed on
//! their beats ([`Stream::run`]); the stream passes them stage by stage and
//! hands the loop's engine its notes (held until let go) and controllers
//! (bend, the mod wheel, pressure, per-note slide). The sustain pedal holds
//! keys at the source, so it holds on every instrument and an arp; an arp
//! runs on the pass's beats (on the score's clock, or on a free clock at
//! its tempo while the transport is stopped).
//!
//! Nothing here allocates: lists and arp state are made with the stream.

use crate::event::{Event, EventKind, NoteOpts};
use makepad_audio_synth::kit::arp::{Arp, ArpEvent, ArpSettings};

/// The most stages a stream passes through.
pub const MAX_STAGES: usize = 8;
/// The most messages one pass carries through a stage.
pub const MAX_MSGS: usize = 256;
/// The most tones a `Chord` stage adds to a key.
pub const MAX_TONES: usize = 6;
/// How long a key's note holds unless it is let go, in beats.
pub const HELD_BEATS: f32 = 1.0e7;

/// A continuous controller.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ctl {
    /// Pitch bend, -1..1 (times the bend range).
    Bend,
    /// The mod wheel (CC 1), 0..1.
    Mod,
    /// Channel pressure, 0..1.
    Pressure,
    /// A key's own pressure (polyphonic aftertouch), 0..1.
    KeyPressure(u8),
    /// Slide (CC 74, an MPE key's timbre), 0..1.
    Slide,
    /// A key's own slide (MPE: its channel's CC 74), 0..1.
    KeySlide(u8),
    /// The sustain pedal (CC 64), 0..1: held at the source.
    Sustain,
    /// Every key let go (CC 123 / 120).
    AllOff,
}

/// One message on its beat. `ch` is the MIDI channel (0..15).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Msg {
    On { beat: f64, ch: u8, note: u8, vel: f32 },
    Off { beat: f64, ch: u8, note: u8 },
    Ctl { beat: f64, ch: u8, ctl: Ctl, value: f32 },
}

impl Msg {
    pub fn beat(&self) -> f64 {
        match self {
            Msg::On { beat, .. } | Msg::Off { beat, .. } | Msg::Ctl { beat, .. } => *beat,
        }
    }

    /// A MIDI channel message (status, data, data) at `beat`; None for what
    /// a stream does not take (program changes, system messages).
    pub fn from_midi(data: [u8; 3], beat: f64) -> Option<Msg> {
        let (status, ch, a, b) = (data[0] & 0xf0, data[0] & 15, data[1] & 127, data[2] & 127);
        Some(match status {
            0x90 if b > 0 => Msg::On { beat, ch, note: a, vel: b as f32 / 127.0 },
            0x80 | 0x90 => Msg::Off { beat, ch, note: a },
            0xa0 => Msg::Ctl { beat, ch, ctl: Ctl::KeyPressure(a), value: b as f32 / 127.0 },
            0xd0 => Msg::Ctl { beat, ch, ctl: Ctl::Pressure, value: a as f32 / 127.0 },
            0xe0 => Msg::Ctl { beat, ch, ctl: Ctl::Bend, value: ((((b as u16) << 7) | a as u16) as f32 - 8192.0) / 8192.0 },
            0xb0 => match a {
                1 => Msg::Ctl { beat, ch, ctl: Ctl::Mod, value: b as f32 / 127.0 },
                64 => Msg::Ctl { beat, ch, ctl: Ctl::Sustain, value: b as f32 / 127.0 },
                74 => Msg::Ctl { beat, ch, ctl: Ctl::Slide, value: b as f32 / 127.0 },
                120 | 123 => Msg::Ctl { beat, ch, ctl: Ctl::AllOff, value: 0.0 },
                _ => return None,
            },
            _ => return None,
        })
    }
}

/// How a `Velocity` stage bends a key's velocity: as it is, softer keys
/// louder (`soft`), harder (`hard`), or every key at one level (`fixed`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VelCurve {
    Linear,
    Soft,
    Hard,
    Fixed(f32),
}

impl VelCurve {
    pub const NAMES: [&'static str; 4] = ["linear", "soft", "hard", "fixed"];

    pub fn apply(&self, v: f32, min: f32, max: f32) -> f32 {
        let v = v.clamp(0.0, 1.0);
        let shaped = match self {
            VelCurve::Linear => v,
            VelCurve::Soft => v.sqrt(),
            VelCurve::Hard => v * v,
            VelCurve::Fixed(level) => return level.clamp(0.0, 1.0),
        };
        (min + (max - min) * shaped).clamp(0.0, 1.0)
    }
}

/// One stage of a stream, as the score's node sets it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StageDef {
    /// Every note this many semitones up (down when negative).
    Transpose(i8),
    /// Each key plays these semitones above it too.
    Chord { tones: [i8; MAX_TONES], n: u8 },
    Velocity { curve: VelCurve, min: f32, max: f32 },
    /// An arpeggiator over the keys held; `latch` keeps a chord playing
    /// after its keys are let go (the next chord replaces it); `sync` puts
    /// its steps on the clock's grid.
    Arp { settings: ArpSettings, latch: bool, sync: bool },
}

/// What a stream is: its stages in order, the bend range (semitones a full
/// bend moves) and whether channels are per key (MPE: a channel's bend,
/// pressure and slide move only its key).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamDef {
    pub stages: [Option<StageDef>; MAX_STAGES],
    pub bend_range: f32,
    pub mpe: bool,
}

impl Default for StreamDef {
    fn default() -> Self {
        StreamDef { stages: [None; MAX_STAGES], bend_range: 2.0, mpe: false }
    }
}

/// Numbers each stage takes in a stream's encoding.
pub const STAGE_WORDS: usize = 12;

impl StreamDef {
    pub fn has_arp(&self) -> bool {
        self.stages.iter().flatten().any(|s| matches!(s, StageDef::Arp { .. }))
    }

    /// The stream as a list of numbers (a `Loop` node's `stream` setting):
    /// the bend range, MPE, then [`STAGE_WORDS`] per stage, its kind first.
    pub fn encode(&self) -> Vec<f64> {
        let mut out = vec![self.bend_range as f64, self.mpe as u8 as f64];
        for stage in self.stages.iter().flatten() {
            let mut w = [0.0f64; STAGE_WORDS];
            match *stage {
                StageDef::Transpose(semis) => (w[0], w[1]) = (1.0, semis as f64),
                StageDef::Chord { tones, n } => {
                    (w[0], w[1]) = (2.0, n as f64);
                    for (k, t) in tones.iter().enumerate() {
                        w[2 + k] = *t as f64;
                    }
                }
                StageDef::Velocity { curve, min, max } => {
                    let (index, level) = match curve {
                        VelCurve::Linear => (0.0, 0.0),
                        VelCurve::Soft => (1.0, 0.0),
                        VelCurve::Hard => (2.0, 0.0),
                        VelCurve::Fixed(level) => (3.0, level as f64),
                    };
                    w[..5].copy_from_slice(&[3.0, index, level, min as f64, max as f64]);
                }
                StageDef::Arp { settings: a, latch, sync } => {
                    w[..11].copy_from_slice(&[
                        4.0, a.pattern as f64, a.step, a.octaves as f64, a.gate as f64, a.swing as f64, a.repeats as f64, a.chance as f64,
                        a.transpose as f64, latch as u8 as f64, sync as u8 as f64,
                    ]);
                }
            }
            out.extend_from_slice(&w);
        }
        out
    }

    /// A stream from its encoding ([`StreamDef::encode`]); None for an
    /// empty list (the loop plays no stream).
    pub fn decode(words: &[f64]) -> Option<StreamDef> {
        if words.len() < 2 {
            return None;
        }
        let mut def = StreamDef { bend_range: (words[0] as f32).clamp(0.0, 48.0), mpe: words[1] > 0.5, ..Default::default() };
        for (k, w) in words[2..].chunks_exact(STAGE_WORDS).take(MAX_STAGES).enumerate() {
            let int = |x: f64, lo: i32, hi: i32| (x.round() as i32).clamp(lo, hi);
            def.stages[k] = match w[0] as u8 {
                1 => Some(StageDef::Transpose(int(w[1], -48, 48) as i8)),
                2 => {
                    let mut tones = [0i8; MAX_TONES];
                    for (t, x) in tones.iter_mut().zip(&w[2..2 + MAX_TONES]) {
                        *t = int(*x, -48, 48) as i8;
                    }
                    Some(StageDef::Chord { tones, n: int(w[1], 0, MAX_TONES as i32) as u8 })
                }
                3 => Some(StageDef::Velocity {
                    curve: match w[1] as u8 {
                        1 => VelCurve::Soft,
                        2 => VelCurve::Hard,
                        3 => VelCurve::Fixed(w[2] as f32),
                        _ => VelCurve::Linear,
                    },
                    min: w[3] as f32,
                    max: w[4] as f32,
                }),
                4 => Some(StageDef::Arp {
                    settings: ArpSettings {
                        pattern: int(w[1], 0, 63) as usize,
                        step: w[2],
                        octaves: int(w[3], 1, 4) as usize,
                        gate: w[4] as f32,
                        swing: w[5] as f32,
                        repeats: int(w[6], 1, 4) as usize,
                        chance: w[7] as f32,
                        transpose: int(w[8], -24, 24),
                    },
                    latch: w[9] > 0.5,
                    sync: w[10] > 0.5,
                }),
                _ => None,
            };
        }
        Some(def)
    }
}

/// A key's handle on the instrument: per channel and note.
pub fn handle(ch: u8, note: u8) -> u32 {
    0x8000_0000 | ((ch as u32 & 15) << 7) | note as u32
}

/// The channel a stream note's handle names.
pub fn handle_channel(handle: u32) -> Option<u8> {
    (handle & 0x8000_0000 != 0).then_some(((handle >> 7) & 15) as u8)
}

/// An arp stage's state.
struct ArpStage {
    arp: Arp,
    /// Keys down now (latch keeps the arp's own after they come up).
    down: [u64; 2],
    /// The beat the arp has played to.
    cursor: f64,
    events: [(f64, ArpEvent); 64],
    /// The channel its notes go out on (the last key's).
    ch: u8,
}

fn bit(set: &[u64; 2], note: u8) -> bool {
    set[(note >> 6) as usize & 1] & (1 << (note & 63)) != 0
}

fn set_bit(set: &mut [u64; 2], note: u8, on: bool) {
    let (word, mask) = ((note >> 6) as usize & 1, 1u64 << (note & 63));
    if on {
        set[word] |= mask;
    } else {
        set[word] &= !mask;
    }
}

fn count(set: &[u64; 2]) -> u32 {
    set[0].count_ones() + set[1].count_ones()
}

/// A stream's state: the definition, the keys held at the source and
/// under the pedal, each arp stage's state, what sounds at the output, and
/// two message lists a pass ping-pongs between.
pub struct Stream {
    pub def: StreamDef,
    /// Keys down at the source (per channel) and the ones the pedal holds.
    down: [[u64; 2]; 16],
    pedalled: [[u64; 2]; 16],
    pedal: [bool; 16],
    arps: [Option<Box<ArpStage>>; MAX_STAGES],
    /// Notes sounding at the output, per channel, with how many keys hold
    /// each.
    sounding: [[u8; 128]; 16],
    a: Box<[Msg]>,
    b: Box<[Msg]>,
    na: usize,
    /// Messages a full list dropped.
    pub dropped: u32,
    /// The last notes started at the output (beat, note, velocity 0..127),
    /// newest last.
    recent: [(f64, u8, u8); 8],
    nrecent: usize,
    started: u64,
}

impl Stream {
    /// Made off the audio thread.
    pub fn new(def: StreamDef) -> Stream {
        let mut arps: [Option<Box<ArpStage>>; MAX_STAGES] = Default::default();
        for (k, stage) in def.stages.iter().enumerate() {
            if let Some(StageDef::Arp { .. }) = stage {
                arps[k] = Some(Box::new(ArpStage { arp: Arp::new(), down: [0; 2], cursor: f64::NAN, events: [(0.0, ArpEvent::Off(0)); 64], ch: 0 }));
            }
        }
        let empty = Msg::Off { beat: 0.0, ch: 0, note: 0 };
        Stream {
            def, down: [[0; 2]; 16], pedalled: [[0; 2]; 16], pedal: [false; 16], arps, sounding: [[0; 128]; 16],
            a: vec![empty; MAX_MSGS].into_boxed_slice(), b: vec![empty; MAX_MSGS].into_boxed_slice(), na: 0, dropped: 0,
            recent: [(0.0, 0, 0); 8], nrecent: 0, started: 0,
        }
    }

    /// Carry what plays over from `old` (a new revision of the same loop):
    /// the keys held, the pedal, what sounds and the arps' chords where
    /// this revision has an arp at the same stage. No allocation.
    pub fn take_state(&mut self, old: &mut Stream) {
        self.down = old.down;
        self.pedalled = old.pedalled;
        self.pedal = old.pedal;
        self.sounding = old.sounding;
        self.recent = old.recent;
        self.nrecent = old.nrecent;
        self.started = old.started;
        for k in 0..MAX_STAGES {
            if self.arps[k].is_some() && old.arps[k].is_some() {
                std::mem::swap(&mut self.arps[k], &mut old.arps[k]);
            }
        }
    }

    /// Queue a message for this pass (in beat order).
    pub fn push(&mut self, msg: Msg) {
        if self.na >= MAX_MSGS {
            self.dropped += 1;
            return;
        }
        let at = self.a[..self.na].partition_point(|m| m.beat() <= msg.beat());
        self.a.copy_within(at..self.na, at + 1);
        self.a[at] = msg;
        self.na += 1;
    }

    /// Whether anything sounds or will (keys held, a latched arp).
    pub fn active(&self) -> bool {
        self.na > 0 || self.sounding.iter().any(|ch| ch.iter().any(|n| *n > 0)) || self.arps.iter().flatten().any(|a| a.arp.held() > 0)
    }

    /// The keys held at the source (any channel, the pedal's too).
    pub fn held(&self) -> [u64; 2] {
        let mut out = [0u64; 2];
        for ch in 0..16 {
            out[0] |= self.down[ch][0] | self.pedalled[ch][0];
            out[1] |= self.down[ch][1] | self.pedalled[ch][1];
        }
        out
    }

    /// The notes sounding at the output (any channel).
    pub fn sounding(&self) -> [u64; 2] {
        let mut out = [0u64; 2];
        for ch in self.sounding.iter() {
            for (note, n) in ch.iter().enumerate() {
                if *n > 0 {
                    set_bit(&mut out, note as u8, true);
                }
            }
        }
        out
    }

    /// The last notes started (beat, note, velocity), newest last, and how
    /// many ever were.
    pub fn recent(&self) -> (&[(f64, u8, u8)], u64) {
        (&self.recent[..self.nrecent], self.started)
    }

    /// The first arp's pattern as built for its chord, and its step.
    pub fn arp_steps(&self) -> Option<(&[u8], usize)> {
        self.arps.iter().flatten().next().map(|a| a.arp.steps())
    }

    fn out(list: &mut [Msg], n: &mut usize, dropped: &mut u32, msg: Msg) {
        if *n < list.len() {
            list[*n] = msg;
            *n += 1;
        } else {
            *dropped += 1;
        }
    }

    /// One pass over beats `b0..b1` (`b1 > b0`): the queued messages through
    /// every stage; each note and controller to `note` and `ctl` in beat
    /// order. Notes as events of loop `loop_key` (held until let go).
    pub fn run(&mut self, b0: f64, b1: f64, loop_key: u64, pan: f32, note: &mut dyn FnMut(Event), ctl: &mut dyn FnMut(u8, Ctl, f32)) {
        // The source: the pedal holds keys let go under it.
        let (mut nb, mut dropped) = (0usize, 0u32);
        for i in 0..self.na {
            let msg = self.a[i];
            match msg {
                Msg::On { beat, ch, note, .. } => {
                    let c = ch as usize & 15;
                    // A key that still sounds (pedalled, or no note-off came)
                    // starts again.
                    if bit(&self.down[c], note) || bit(&self.pedalled[c], note) {
                        Self::out(&mut self.b, &mut nb, &mut dropped, Msg::Off { beat, ch, note });
                    }
                    set_bit(&mut self.down[c], note, true);
                    set_bit(&mut self.pedalled[c], note, false);
                    Self::out(&mut self.b, &mut nb, &mut dropped, msg);
                }
                Msg::Off { ch, note, .. } => {
                    let c = ch as usize & 15;
                    set_bit(&mut self.down[c], note, false);
                    if self.pedal[c] {
                        set_bit(&mut self.pedalled[c], note, true);
                    } else {
                        Self::out(&mut self.b, &mut nb, &mut dropped, msg);
                    }
                }
                Msg::Ctl { beat, ch, ctl: Ctl::Sustain, value } => {
                    let c = ch as usize & 15;
                    let down = value >= 0.5;
                    if self.pedal[c] && !down {
                        for note in 0..128u8 {
                            if bit(&self.pedalled[c], note) {
                                Self::out(&mut self.b, &mut nb, &mut dropped, Msg::Off { beat, ch, note });
                            }
                        }
                        self.pedalled[c] = [0; 2];
                    }
                    self.pedal[c] = down;
                }
                Msg::Ctl { beat, ch, ctl: Ctl::AllOff, .. } => {
                    let c = ch as usize & 15;
                    for note in 0..128u8 {
                        if bit(&self.down[c], note) || bit(&self.pedalled[c], note) {
                            Self::out(&mut self.b, &mut nb, &mut dropped, Msg::Off { beat, ch, note });
                        }
                    }
                    self.down[c] = [0; 2];
                    self.pedalled[c] = [0; 2];
                    Self::out(&mut self.b, &mut nb, &mut dropped, msg);
                }
                Msg::Ctl { .. } => Self::out(&mut self.b, &mut nb, &mut dropped, msg),
            }
        }
        self.na = 0;
        std::mem::swap(&mut self.a, &mut self.b);
        let mut na = nb;
        // Each stage, from list a into list b.
        for k in 0..MAX_STAGES {
            let Some(stage) = self.def.stages[k] else { continue };
            let mut nb = 0usize;
            match stage {
                StageDef::Transpose(semis) => {
                    for i in 0..na {
                        let shifted = match self.a[i] {
                            Msg::On { beat, ch, note, vel } => (note as i32 + semis as i32).try_into().ok().filter(|n: &u8| *n < 128).map(|note| Msg::On { beat, ch, note, vel }),
                            Msg::Off { beat, ch, note } => (note as i32 + semis as i32).try_into().ok().filter(|n: &u8| *n < 128).map(|note| Msg::Off { beat, ch, note }),
                            Msg::Ctl { beat, ch, ctl: Ctl::KeyPressure(note), value } => {
                                (note as i32 + semis as i32).try_into().ok().filter(|n: &u8| *n < 128).map(|note| Msg::Ctl { beat, ch, ctl: Ctl::KeyPressure(note), value })
                            }
                            other => Some(other),
                        };
                        if let Some(msg) = shifted {
                            Self::out(&mut self.b, &mut nb, &mut dropped, msg);
                        }
                    }
                }
                StageDef::Chord { tones, n } => {
                    for i in 0..na {
                        let msg = self.a[i];
                        Self::out(&mut self.b, &mut nb, &mut dropped, msg);
                        for tone in tones.iter().take(n as usize) {
                            let up = |note: u8| (note as i32 + *tone as i32).try_into().ok().filter(|n: &u8| *n < 128);
                            let more = match msg {
                                Msg::On { beat, ch, note, vel } => up(note).map(|note| Msg::On { beat, ch, note, vel }),
                                Msg::Off { beat, ch, note } => up(note).map(|note| Msg::Off { beat, ch, note }),
                                _ => None,
                            };
                            if let Some(more) = more {
                                Self::out(&mut self.b, &mut nb, &mut dropped, more);
                            }
                        }
                    }
                }
                StageDef::Velocity { curve, min, max } => {
                    for i in 0..na {
                        let msg = match self.a[i] {
                            Msg::On { beat, ch, note, vel } => Msg::On { beat, ch, note, vel: curve.apply(vel, min, max) },
                            other => other,
                        };
                        Self::out(&mut self.b, &mut nb, &mut dropped, msg);
                    }
                }
                StageDef::Arp { settings, latch, sync } => {
                    let Some(st) = self.arps[k].as_deref_mut() else { continue };
                    let step = settings.step.max(1.0 / 64.0);
                    // A jump on the clock (a locate, a loop, the transport
                    // starting): the arp goes on from the new beat.
                    if !(st.cursor - b0).abs().lt(&1e-6) {
                        st.cursor = b0;
                        if sync && st.arp.held() > 0 {
                            st.arp.align(&settings, b0.rem_euclid(step), (b0 / step).floor().max(0.0) as u64);
                        }
                    }
                    let advance = |st: &mut ArpStage, to: f64, list: &mut [Msg], nb: &mut usize, dropped: &mut u32| {
                        let beats = (to - st.cursor).max(0.0);
                        let n = st.arp.advance(&settings, beats, &mut st.events);
                        for e in &st.events[..n] {
                            let beat = (st.cursor + e.0).min(to);
                            let msg = match e.1 {
                                ArpEvent::On(note, vel) => Msg::On { beat, ch: st.ch, note, vel: vel as f32 / 127.0 },
                                ArpEvent::Off(note) => Msg::Off { beat, ch: st.ch, note },
                            };
                            Self::out(list, nb, dropped, msg);
                        }
                        st.cursor = to;
                    };
                    for i in 0..na {
                        let msg = self.a[i];
                        advance(st, msg.beat().clamp(b0, b1), &mut self.b, &mut nb, &mut dropped);
                        match msg {
                            Msg::On { beat, ch, note, vel } => {
                                if latch && count(&st.down) == 0 {
                                    st.arp.clear();
                                }
                                let fresh = st.arp.held() == 0;
                                st.arp.press(note, (vel * 127.0).round().clamp(1.0, 127.0) as u8);
                                st.ch = ch;
                                if fresh && sync {
                                    st.arp.align(&settings, beat.rem_euclid(step), (beat / step).floor().max(0.0) as u64);
                                }
                                set_bit(&mut st.down, note, true);
                            }
                            Msg::Off { note, .. } => {
                                set_bit(&mut st.down, note, false);
                                if !latch {
                                    st.arp.release(note);
                                }
                            }
                            Msg::Ctl { ctl: Ctl::AllOff, .. } => {
                                st.down = [0; 2];
                                st.arp.clear();
                                Self::out(&mut self.b, &mut nb, &mut dropped, msg);
                            }
                            // Pressure on a key reaches the arp's notes as the channel's.
                            Msg::Ctl { beat, ch, ctl: Ctl::KeyPressure(_), value } => Self::out(&mut self.b, &mut nb, &mut dropped, Msg::Ctl { beat, ch, ctl: Ctl::Pressure, value }),
                            Msg::Ctl { .. } => Self::out(&mut self.b, &mut nb, &mut dropped, msg),
                        }
                    }
                    advance(st, b1, &mut self.b, &mut nb, &mut dropped);
                    // An arp's notes come out in time order with what passed
                    // (stable: a step's stop stays before the next start).
                    for i in 1..nb {
                        let mut j = i;
                        while j > 0 && self.b[j - 1].beat() > self.b[j].beat() {
                            self.b.swap(j - 1, j);
                            j -= 1;
                        }
                    }
                }
            }
            std::mem::swap(&mut self.a, &mut self.b);
            na = nb;
        }
        self.dropped += dropped;
        // The output: a note sounds while any key holds it.
        for i in 0..na {
            match self.a[i] {
                Msg::On { beat, ch, note: key, vel } => {
                    let c = ch as usize & 15;
                    self.sounding[c][key as usize] = self.sounding[c][key as usize].saturating_add(1);
                    let opts = NoteOpts { handle: handle(ch, key), ..Default::default() };
                    note(Event { beat, loop_key, kind: EventKind::Note, note: key as f32, vel, pan, dur: HELD_BEATS, opts });
                    if self.nrecent == self.recent.len() {
                        self.recent.copy_within(1.., 0);
                        self.nrecent -= 1;
                    }
                    self.recent[self.nrecent] = (beat, key, (vel * 127.0).round().clamp(1.0, 127.0) as u8);
                    self.nrecent += 1;
                    self.started += 1;
                }
                Msg::Off { beat, ch, note: key } => {
                    let c = ch as usize & 15;
                    let held = &mut self.sounding[c][key as usize];
                    if *held > 0 {
                        *held -= 1;
                        if *held == 0 {
                            let opts = NoteOpts { handle: handle(ch, key), ..Default::default() };
                            note(Event { beat, loop_key, kind: EventKind::Release, note: key as f32, vel: 0.0, pan, dur: 0.0, opts });
                        }
                    }
                }
                Msg::Ctl { ch, ctl: Ctl::AllOff, beat, .. } => {
                    let c = ch as usize & 15;
                    for key in 0..128u8 {
                        if self.sounding[c][key as usize] > 0 {
                            self.sounding[c][key as usize] = 0;
                            let opts = NoteOpts { handle: handle(ch, key), ..Default::default() };
                            note(Event { beat, loop_key, kind: EventKind::Release, note: key as f32, vel: 0.0, pan, dur: 0.0, opts });
                        }
                    }
                }
                // MPE: a channel's pressure and slide are its keys'.
                Msg::Ctl { ch, ctl: c @ (Ctl::Pressure | Ctl::Slide), value, .. } if self.def.mpe => {
                    for key in 0..128u8 {
                        if self.sounding[ch as usize & 15][key as usize] > 0 {
                            ctl(ch, if c == Ctl::Pressure { Ctl::KeyPressure(key) } else { Ctl::KeySlide(key) }, value);
                        }
                    }
                }
                Msg::Ctl { ch, ctl: c, value, .. } => ctl(ch, c, value),
            }
        }
    }

    /// Everything let go at once (the loop stops): what sounds ends, the
    /// pedal and the arps' chords go.
    pub fn release_all(&mut self) {
        self.na = 0;
        self.down = [[0; 2]; 16];
        self.pedalled = [[0; 2]; 16];
        self.pedal = [false; 16];
        self.sounding = [[0; 128]; 16];
        for st in self.arps.iter_mut().flatten() {
            st.arp.clear();
            st.down = [0; 2];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(stream: &mut Stream, b0: f64, b1: f64) -> (Vec<(f64, EventKind, f32, f32)>, Vec<(u8, Ctl, f32)>) {
        let (mut notes, mut ctls) = (Vec::new(), Vec::new());
        stream.run(b0, b1, 1, 0.0, &mut |e| notes.push((e.beat, e.kind, e.note, e.vel)), &mut |ch, c, v| ctls.push((ch, c, v)));
        (notes, ctls)
    }

    fn with(stages: &[StageDef]) -> StreamDef {
        let mut def = StreamDef::default();
        for (slot, s) in def.stages.iter_mut().zip(stages) {
            *slot = Some(*s);
        }
        def
    }

    #[test]
    fn chords_transpose_and_velocity_shape_each_key_and_round_trip_their_encoding() {
        let def = with(&[
            StageDef::Chord { tones: [7, 12, 0, 0, 0, 0], n: 2 },
            StageDef::Transpose(-12),
            StageDef::Velocity { curve: VelCurve::Fixed(0.5), min: 0.0, max: 1.0 },
        ]);
        assert_eq!(StreamDef::decode(&def.encode()), Some(def));
        let mut s = Stream::new(def);
        s.push(Msg::On { beat: 1.0, ch: 0, note: 60, vel: 1.0 });
        let (notes, _) = run(&mut s, 1.0, 1.1);
        assert_eq!(notes, [(1.0, EventKind::Note, 48.0, 0.5), (1.0, EventKind::Note, 55.0, 0.5), (1.0, EventKind::Note, 60.0, 0.5)]);
        s.push(Msg::Off { beat: 1.2, ch: 0, note: 60 });
        let (notes, _) = run(&mut s, 1.1, 1.3);
        assert_eq!(notes.iter().map(|n| (n.1, n.2)).collect::<Vec<_>>(), [(EventKind::Release, 48.0), (EventKind::Release, 55.0), (EventKind::Release, 60.0)]);
        assert!(!s.active());
    }

    #[test]
    fn a_latched_arp_plays_on_until_the_next_chord_replaces_it() {
        let settings = ArpSettings { step: 0.5, gate: 0.5, ..Default::default() };
        let mut s = Stream::new(with(&[StageDef::Arp { settings, latch: true, sync: true }]));
        s.push(Msg::On { beat: 0.0, ch: 0, note: 60, vel: 0.8 });
        s.push(Msg::On { beat: 0.0, ch: 0, note: 64, vel: 0.8 });
        s.push(Msg::Off { beat: 0.1, ch: 0, note: 60 });
        s.push(Msg::Off { beat: 0.1, ch: 0, note: 64 });
        let (notes, _) = run(&mut s, 0.0, 2.0);
        let starts: Vec<(f64, f32)> = notes.iter().filter(|n| n.1 == EventKind::Note).map(|n| ((n.0 * 1e9).round() / 1e9, n.2)).collect();
        assert_eq!(starts, [(0.0, 60.0), (0.5, 64.0), (1.0, 60.0), (1.5, 64.0)]);
        // A new chord: the latched one goes.
        s.push(Msg::On { beat: 2.0, ch: 0, note: 67, vel: 0.8 });
        let (notes, _) = run(&mut s, 2.0, 3.0);
        let starts: Vec<f32> = notes.iter().filter(|n| n.1 == EventKind::Note).map(|n| n.2).collect();
        assert_eq!(starts, [67.0, 67.0]);
    }

    #[test]
    fn mpe_pressure_goes_to_the_key_on_its_channel_and_bend_keeps_its_channel() {
        let mut s = Stream::new(StreamDef { mpe: true, ..Default::default() });
        s.push(Msg::On { beat: 0.0, ch: 2, note: 60, vel: 0.8 });
        s.push(Msg::On { beat: 0.0, ch: 3, note: 64, vel: 0.8 });
        s.push(Msg::from_midi([0xd3, 100, 0], 0.01).unwrap());
        s.push(Msg::from_midi([0xe2, 0, 0x60], 0.02).unwrap());
        let (_, ctls) = run(&mut s, 0.0, 0.1);
        assert_eq!(ctls.len(), 2);
        assert_eq!((ctls[0].0, ctls[0].1), (3, Ctl::KeyPressure(64)));
        assert_eq!((ctls[1].0, ctls[1].1), (2, Ctl::Bend));
        assert!((ctls[1].2 - 0.5).abs() < 1e-3);
    }
}
