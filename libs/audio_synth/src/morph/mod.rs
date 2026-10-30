//! Morph: a wavetable synth layout over the synth kit ([`crate::kit`]).
//!
//! A voice has three main oscillators (each wavetable, granular, spectral
//! or sample, with unison and two warps), a sub and a noise oscillator, two
//! filters (serial or parallel; each source picks the filter it feeds),
//! four envelopes (the first is the amp), eight LFOs, eight macros and a
//! mod matrix of up to 64 rows. The voices are summed into the effects
//! rack; an arpeggiator can play the held notes on the tempo.
//!
//! Every parameter is in one schema ([`layout`]), which the score, the
//! panels and the AI read. A patch is data ([`MorphPatch`], text form
//! `a_table=formant a_pos=0.3 mod=lfo1,a_pos,0.4 fx=delay,reverb`).
//!
//! Realtime: `note_on`, `note_off` and `render` never allocate or lock.
//! Building a patch's sources (wavetables) is the caller's, off the audio
//! thread.

mod patch;

pub use patch::{MorphPatch, MorphSources};

use crate::kit::arp::{self, Arp, ArpEvent, ArpIx};
use crate::kit::env::{bend, Env, EnvBlock, EnvIx};
use crate::kit::filter::{self, Filter, FilterBlock, FilterIx};
use crate::kit::fx::{self, FxIx, Rack};
use crate::kit::lfo::{self, Lfo, LfoBlock, LfoIx};
use crate::kit::matrix::{self, N_SOURCES};
use crate::kit::osc::{self, midi_hz, pan_gains, NoiseIx, NoiseOsc, Osc, OscBlock, OscIx, Sub, SubIx};
use crate::kit::schema::{Builder, Schema, Unit::{self, Cents, Percent, Secs, Semis}, Values};
use crate::dsp::{DcBlock, Rng};
use std::sync::{Arc, OnceLock};

/// The wavetables an oscillator can name (`a_table: @formant`).
pub const TABLES: &[&str] = &["basic", "pwm", "harmonic", "formant", "fm", "sync", "organ", "bell", "vocal", "digital", "noisy", "growl", "user", "import"];
pub const GLIDE_MODES: &[&str] = &["always", "legato"];
pub const MAX_VOICES: usize = 32;
/// Frames a control block lasts (modulation, envelopes, LFOs move per block).
pub const BLOCK: usize = 32;

/// Where Morph's parameters sit.
#[derive(Clone, Debug, Default)]
pub struct Ix {
    pub osc: [OscIx; 3],
    pub sub: SubIx,
    pub noise: NoiseIx,
    pub filter: [FilterIx; 2],
    pub filter_route: usize,
    pub env: [EnvIx; 4],
    pub lfo: [LfoIx; 8],
    pub macros: [usize; 8],
    pub poly: usize,
    pub mono: usize,
    pub legato: usize,
    pub glide: usize,
    pub glide_mode: usize,
    pub glide_scaled: usize,
    pub glide_curve: usize,
    pub bend_range: usize,
    pub pitch: usize,
    pub velocity: usize,
    pub level: usize,
    pub rand_pan: usize,
    pub rand_detune: usize,
    pub rand_cutoff: usize,
    pub rand_env: usize,
    pub tuning: usize,
    pub tuning_root: usize,
    pub arp: ArpIx,
    pub fx: FxIx,
}

pub struct Layout {
    pub schema: Schema,
    pub ix: Ix,
}

fn build() -> Layout {
    let mut b = Builder::new();
    let osc = [
        osc::declare(&mut b, "a", TABLES, true, 1),
        osc::declare(&mut b, "b", TABLES, false, 0),
        osc::declare(&mut b, "c", TABLES, false, 0),
    ];
    let sub = osc::declare_sub(&mut b);
    let noise = osc::declare_noise(&mut b);
    let filter = [filter::declare(&mut b, 1, true, 4000.0), filter::declare(&mut b, 2, false, 1000.0)];
    b.group("filters", "serial: f1 then f2; parallel: side by side");
    let filter_route = b.choice("filter_route".into(), "route", filter::ROUTES, 0, "");
    let env = std::array::from_fn(|e| crate::kit::env::declare(&mut b, e + 1, e == 0));
    let lfo = std::array::from_fn(|l| lfo::declare(&mut b, l + 1));
    b.group("macros", "eight macro knobs (sources m1..m8): give each a job with mod rows; macro_names names them");
    let macros = std::array::from_fn(|m| b.num(format!("m{}", m + 1), &format!("macro {}", m + 1), 0.0, 1.0, 0.0, Percent, ""));
    b.group("voice", "voicing: polyphony, mono and legato, glide, bend range, global pitch, velocity, level, per-note randomness");
    let poly = b.int("poly".into(), "poly", 1.0, MAX_VOICES as f32, 16.0, "");
    let mono = b.flag("mono".into(), "mono", false, "");
    let legato = b.flag("legato".into(), "legato", false, "mono: overlapping notes do not restart the envelopes");
    let glide = b.log("glide".into(), "glide", 0.0001, 4.0, 0.0001, Secs, "");
    let glide_mode = b.choice("glide_mode".into(), "glide mode", GLIDE_MODES, 0, "legato: only overlapping notes glide");
    let glide_scaled = b.flag("glide_scaled".into(), "scaled", false, "glide time is per octave");
    let glide_curve = b.num("glide_curve".into(), "glide curve", -1.0, 1.0, 0.0, Unit::None, "- fast then settles, + slow then fast");
    let bend_range = b.int("bend_range".into(), "bend", 0.0, 48.0, 2.0, "");
    let pitch = b.num("pitch".into(), "pitch", -48.0, 48.0, 0.0, Semis, "every oscillator's pitch (vibrato, risers: mod it)");
    let velocity = b.num("velocity".into(), "velocity", 0.0, 1.0, 0.6, Percent, "how much velocity sets the level");
    let level = b.num("level".into(), "level", 0.0, 2.0, 0.7, Unit::None, "");
    let rand_pan = b.num("rand_pan".into(), "rand pan", 0.0, 1.0, 0.0, Percent, "each note a random pan");
    let rand_detune = b.num("rand_detune".into(), "rand detune", 0.0, 50.0, 0.0, Cents, "each note a random detune (analog drift)");
    let rand_cutoff = b.num("rand_cutoff".into(), "rand cutoff", 0.0, 1.0, 0.0, Percent, "each note moves the filters a random amount");
    let tuning = b.choice("tuning".into(), "tuning", tuning_names(), 0, "the scale the keys play (12tet standard; edoN equal steps; just pythagorean meantone harmonics bohlen_pierce); tuning_scale: a scale of its own");
    let tuning_root = b.int("tuning_root".into(), "tuning root", 0.0, 127.0, 60.0, "the key that plays the scale's first degree (A4 stays 440 Hz)");
    let rand_env = b.num("rand_env".into(), "rand env", 0.0, 1.0, 0.0, Percent, "each note scales the envelope times a random amount");
    let arp = arp::declare(&mut b);
    let fx = fx::declare(&mut b);
    Layout {
        schema: b.finish(),
        ix: Ix {
            osc, sub, noise, filter, filter_route, env, lfo, macros, poly, mono, legato, glide, glide_mode, glide_scaled, glide_curve, bend_range,
            pitch, velocity, level, rand_pan, rand_detune, rand_cutoff, rand_env, tuning, tuning_root, arp, fx,
        },
    }
}

/// The tuning library's names (the `tuning` choice).
fn tuning_names() -> &'static [&'static str] {
    static N: OnceLock<Vec<&'static str>> = OnceLock::new();
    N.get_or_init(|| crate::kit::tuning::library().iter().map(|(n, ..)| *n).collect())
}

/// Morph's schema and parameter places (built on first use).
pub fn layout() -> &'static Layout {
    static LAYOUT: OnceLock<Layout> = OnceLock::new();
    LAYOUT.get_or_init(build)
}

/// What a score writes besides the parameters, for the AI.
pub const STRUCTURE: &str = "Morph{preset: @name, any parameter by key, mods: [[@src @dst amount ...] ...], fx: [@delay @reverb ...] (the rack in order, each effect once; an effect's parameters are its group fx_<kind>), lfo1_points: [[x y curve] ...] (lfo shape @drawn), a_frames: [[partial amplitudes] ...] (table @user: one list a frame, partial 1 first), a_src: asset(id, rev) (modes @sample and @granular, table @import), macro_names: [\"bright\" ...]}. ~ marks what a mod row may move.";

/// The built-in patches: (name, what it is, patch text). Our own sounds.
pub const PRESETS: &[(&str, &str, &str)] = &[
    ("init", "one plain wavetable voice", ""),
    ("wide_saw", "a stacked, detuned saw lead/pad", "a_pos=0.66 a_unison=7 a_detune=22 f1_cutoff=7000 env1_release=0.4 fx=reverb reverb_mix=0.18"),
    ("reese", "two detuned saws an octave apart, a slow ladder sweep: a bass", "a_pos=0.66 a_unison=4 a_detune=14 b_on=true b_pos=0.66 b_oct=-1 b_unison=2 b_detune=9 sub_on=true sub_level=0.4 f1_type=ladder f1_cutoff=420 f1_res=0.35 lfo1_mode=free lfo1_rate=0.25 mod=lfo1,f1_cutoff,0.18 env1_release=0.12"),
    ("vowel_pad", "a formant table walked by a smooth random LFO", "a_table=formant a_unison=3 a_detune=10 lfo1_shape=smooth lfo1_rate=0.3 mod=lfo1,a_pos,0.5 env1_attack=0.9 env1_release=2.2 f1_cutoff=5000 fx=reverb reverb_mix=0.4 reverb_decay_time=2.32"),
    ("grain_cloud", "a granular cloud over the vocal table", "a_mode=granular a_table=vocal a_grain=140 a_density=45 a_spray=0.35 a_grain_pitch=0.15 a_scan=0.04 a_unison=3 a_detune=12 env1_attack=1.2 env1_release=3 f1_cutoff=6000 fx=delay,reverb delay_mix=0.2 reverb_mix=0.35"),
    ("glass_bell", "spectral bell: stretched partials whose top closes", "a_mode=spectral a_table=bell a_stretch=0.06 a_tilt=-2 env1_attack=0.002 env1_decay=2.8 env1_sustain=0 env1_release=2.2 env2_decay=1.6 mod=env2,a_cut_hi,0.7 a_cut_hi=24 f1_on=false fx=reverb reverb_mix=0.3"),
    ("wobble", "a growl table and the filter moved on eighths", "a_table=growl a_unison=2 a_detune=8 sub_on=true f1_type=lp24 f1_cutoff=900 f1_res=0.4 lfo1_sync=n8 lfo1_mode=free mod=lfo1,a_pos,0.6 mod=lfo1,f1_cutoff,0.35 mono=true glide=0.03"),
    ("chaos_drone", "a noisy table steered by a Lorenz attractor", "a_table=noisy a_unison=4 a_detune=18 lfo1_shape=lorenz lfo1_rate=0.4 mod=lfo1,a_pos,0.5 mod=lfo1y,f1_cutoff,0.3 f1_type=lp24 f1_cutoff=1800 env1_attack=2 env1_release=4 fx=reverb reverb_mix=0.45 reverb_decay_time=3"),
    ("pluck", "a bright harmonic pluck", "a_table=harmonic a_pos=0.85 env1_decay=0.45 env1_sustain=0 env1_release=0.3 f1_type=lp12 f1_cutoff=700 env2_decay=0.25 mod=env2,f1_cutoff,0.55 fx=delay delay_mix=0.18"),
    ("arp_keys", "a pulse arpeggio over the held chord", "arp_on=true arp_pattern=updown arp_rate=n16 arp_octaves=2 a_table=pwm a_pos=0.35 env1_decay=0.22 env1_sustain=0.15 env1_release=0.12 f1_cutoff=3200 fx=delay delay_left=-1.415 delay_right=-1.415 delay_mix=0.25"),
    ("fm_metal", "a fifth-up oscillator frequency-modulating the first, closing", "b_on=true b_semi=19 b_level=0 a_warp1=fm a_warp1_amt=0.35 a_warp_src=b env2_decay=0.9 mod=env2,a_warp1_amt,0.5 env1_decay=1.4 env1_sustain=0.2 f1_on=false"),
    ("sync_lead", "a hard-sync lead, the sync swept by an envelope", "a_warp1=sync a_warp1_amt=0.2 env2_decay=0.6 mod=env2,a_warp1_amt,0.5 a_unison=2 a_detune=6 mono=true legato=true glide=0.07 f1_cutoff=6000 fx=delay delay_mix=0.15"),
    ("orbit_pad", "two tables moved by one circular path (x and y)", "a_table=organ b_on=true b_table=vocal b_level=0.6 lfo1_shape=circle lfo1_rate=0.15 mod=lfo1,a_pos,0.5 mod=lfo1y,b_pos,0.5 env1_attack=0.7 env1_release=2.5 fx=reverb reverb_mix=0.4"),
];

/// A voice of the engine.
struct Voice {
    active: bool,
    gate: bool,
    note: u8,
    vel: f32,
    release_vel: f32,
    serial: u32,
    pitch: f32,
    glide_from: f32,
    glide_to: f32,
    glide_t: f32,
    glide_len: f32,
    rands: [f32; 6],
    alt: f32,
    pressure: f32,
    slide: f32,
    osc: [Osc; 3],
    sub: Sub,
    noise: NoiseOsc,
    filter: [Filter; 2],
    env: [Env; 4],
    lfo: [Lfo; 8],
    vals: Box<[f32]>,
    amp: f32,
    dc: [DcBlock; 2],
    rng: Rng,
}

impl Voice {
    fn new(seed: u32, nparams: usize) -> Voice {
        Voice {
            active: false,
            gate: false,
            note: 60,
            vel: 0.0,
            release_vel: 0.0,
            serial: 0,
            pitch: 60.0,
            glide_from: 60.0,
            glide_to: 60.0,
            glide_t: 1.0,
            glide_len: 0.0,
            rands: [0.0; 6],
            alt: 1.0,
            pressure: 0.0,
            slide: 0.0,
            osc: [Osc::new(seed * 3 + 1), Osc::new(seed * 3 + 2), Osc::new(seed * 3 + 3)],
            sub: Sub::default(),
            noise: NoiseOsc::new(seed.wrapping_mul(97) + 5),
            filter: [Filter::default(), Filter::default()],
            env: [Env::default(); 4],
            lfo: std::array::from_fn(|i| Lfo::new(seed * 11 + i as u32 + 1)),
            vals: vec![0.0; nparams].into_boxed_slice(),
            amp: 0.0,
            dc: [DcBlock::default(); 2],
            rng: Rng::new(seed.wrapping_mul(0x9e37_79b9) | 1),
        }
    }
}

/// The Morph engine.
pub struct Morph {
    rate: f32,
    patch: Arc<MorphPatch>,
    src: Arc<MorphSources>,
    voices: Box<[Voice]>,
    lfo: [Lfo; 8],
    rack: Box<Rack>,
    arp: Arp,
    arp_events: [ArpEvent; 64],
    arp_was_on: bool,
    /// Beats a second (the tempo), the beat and time clocks.
    bps: f32,
    beats: f64,
    secs: f64,
    mod_wheel: f32,
    bend: f32,
    aftertouch: f32,
    held: [(u8, u8); 32],
    nheld: usize,
    serial: u32,
    alt: f32,
    last_pitch: f32,
    rng: Rng,
    buf_l: [f32; BLOCK],
    buf_r: [f32; BLOCK],
    acc: Box<[f32]>,
    out_pos: usize,
}

impl Morph {
    pub fn new(rate: f32, patch: Arc<MorphPatch>, src: Arc<MorphSources>) -> Morph {
        let n = layout().schema.params.len();
        let voices: Box<[Voice]> = (0..MAX_VOICES).map(|i| Voice::new(i as u32 + 1, n)).collect();
        let mut rack = Box::new(Rack::new(rate));
        rack.set_chain(&patch.fx, &patch.values[..], &layout().ix.fx);
        Morph {
            rate,
            patch,
            src,
            voices,
            lfo: std::array::from_fn(|i| Lfo::new(1000 + i as u32)),
            rack,
            arp: Arp::new(),
            arp_events: [ArpEvent::Off(0); 64],
            arp_was_on: false,
            bps: 2.0,
            beats: 0.0,
            secs: 0.0,
            mod_wheel: 0.0,
            bend: 0.0,
            aftertouch: 0.0,
            held: [(0, 0); 32],
            nheld: 0,
            serial: 0,
            alt: 1.0,
            last_pitch: -1.0,
            rng: Rng::new(0x2545_f491),
            buf_l: [0.0; BLOCK],
            buf_r: [0.0; BLOCK],
            acc: vec![0.0; matrix::MAX_ROWS].into_boxed_slice(),
            out_pos: BLOCK,
        }
    }

    pub fn rate(&self) -> f32 {
        self.rate
    }

    pub fn patch(&self) -> &Arc<MorphPatch> {
        &self.patch
    }

    /// A new patch (and its sources) from the next block; sounding voices
    /// carry on with it.
    pub fn set_patch(&mut self, patch: Arc<MorphPatch>, src: Arc<MorphSources>) {
        self.rack.set_chain(&patch.fx, &patch.values[..], &layout().ix.fx);
        self.patch = patch;
        self.src = src;
        let base = &self.patch.values;
        for v in self.voices.iter_mut().filter(|v| v.active) {
            v.vals.copy_from_slice(base);
        }
    }

    /// The tempo, in beats a second (synced LFOs, the delay, the arp).
    pub fn set_tempo(&mut self, beats_per_sec: f32) {
        if beats_per_sec.is_finite() && beats_per_sec > 0.0 {
            self.bps = beats_per_sec;
        }
    }

    pub fn set_mod_wheel(&mut self, v: f32) {
        self.mod_wheel = v.clamp(0.0, 1.0);
    }

    /// Pitch bend -1..1 (times `bend_range`).
    pub fn set_bend(&mut self, v: f32) {
        self.bend = v.clamp(-1.0, 1.0);
    }

    pub fn set_aftertouch(&mut self, v: f32) {
        self.aftertouch = v.clamp(0.0, 1.0);
    }

    /// Per-note expression: pressure and slide 0..1 for the voice playing `note`.
    pub fn set_expression(&mut self, note: u8, pressure: Option<f32>, slide: Option<f32>) {
        for v in self.voices.iter_mut().filter(|v| v.active && v.note == note) {
            if let Some(p) = pressure {
                v.pressure = p.clamp(0.0, 1.0);
            }
            if let Some(s) = slide {
                v.slide = s.clamp(0.0, 1.0);
            }
        }
    }

    /// Voices sounding now.
    pub fn sounding(&self) -> usize {
        self.voices.iter().filter(|v| v.active).count()
    }

    pub fn note_on(&mut self, note: u8, vel: u8) {
        if vel == 0 {
            return self.note_off(note);
        }
        if self.patch.values[layout().ix.arp.on] > 0.5 {
            self.arp.press(note, vel);
            return;
        }
        self.voice_on(note, vel);
    }

    pub fn note_off(&mut self, note: u8) {
        self.arp.release(note);
        if self.patch.values[layout().ix.arp.on] > 0.5 && self.arp_was_on {
            return;
        }
        self.voice_off(note, 64);
    }

    pub fn all_notes_off(&mut self) {
        for v in self.voices.iter_mut() {
            v.gate = false;
            for e in v.env.iter_mut() {
                e.gate_off();
            }
        }
        self.arp.clear();
        self.nheld = 0;
    }

    fn voice_on(&mut self, note: u8, vel: u8) {
        let ix = &layout().ix;
        let base = &self.patch.values;
        let mono = base[ix.mono] > 0.5;
        let legato = base[ix.legato] > 0.5;
        // The held stack (mono returns to the last held note).
        if let Some(i) = self.held[..self.nheld].iter().position(|h| h.0 == note) {
            self.held.copy_within(i + 1..self.nheld, i);
            self.nheld -= 1;
        }
        if self.nheld < self.held.len() {
            self.held[self.nheld] = (note, vel);
            self.nheld += 1;
        }
        let overlapping = self.voices.iter().any(|v| v.active && v.gate);
        let glide_time = base[ix.glide];
        let glides = glide_time > 0.001 && self.last_pitch >= 0.0 && (base[ix.glide_mode] < 0.5 || overlapping);
        self.serial = self.serial.wrapping_add(1);
        self.alt = -self.alt;
        // The tuned note the key plays; an unmapped key is silent.
        let target = match &self.src.tuning {
            Some(t) => match t.key_note(note as i32) {
                Some(n) => n,
                None => return,
            },
            None => note as f32,
        };
        if mono {
            if let Some(v) = self.voices.iter_mut().find(|v| v.active) {
                if glides {
                    v.glide_from = v.pitch;
                    v.glide_to = target;
                    v.glide_t = 0.0;
                    v.glide_len = if base[ix.glide_scaled] > 0.5 { glide_time * ((target - v.pitch).abs() / 12.0) } else { glide_time };
                } else {
                    v.pitch = target;
                    v.glide_to = target;
                    v.glide_t = 1.0;
                }
                v.note = note;
                v.vel = vel as f32 / 127.0;
                v.gate = true;
                v.serial = self.serial;
                let retrigger = !(legato && overlapping);
                if retrigger {
                    Self::start_voice(v, &self.patch, &self.src, self.alt, false);
                }
                for e in v.env.iter_mut() {
                    e.gate_on(!retrigger);
                }
                self.last_pitch = target;
                return;
            }
        }
        let poly = if mono { 1 } else { base[ix.poly].clamp(1.0, MAX_VOICES as f32) as usize };
        let active = self.voices.iter().filter(|v| v.active).count();
        let slot = if active < poly {
            self.voices.iter().position(|v| !v.active)
        } else {
            None
        }
        .or_else(|| {
            // Steal: the oldest released voice, else the oldest.
            let released = self.voices.iter().enumerate().filter(|(_, v)| v.active && !v.gate).min_by_key(|(_, v)| v.serial).map(|(i, _)| i);
            released.or_else(|| self.voices.iter().enumerate().filter(|(_, v)| v.active).min_by_key(|(_, v)| v.serial).map(|(i, _)| i))
        });
        let Some(i) = slot else { return };
        let v = &mut self.voices[i];
        v.note = note;
        v.vel = vel as f32 / 127.0;
        v.gate = true;
        v.active = true;
        v.serial = self.serial;
        if glides {
            v.glide_from = if self.last_pitch >= 0.0 { self.last_pitch } else { target };
            v.glide_to = target;
            v.glide_t = 0.0;
            v.glide_len = if base[ix.glide_scaled] > 0.5 { glide_time * ((target - v.glide_from).abs() / 12.0) } else { glide_time };
            v.pitch = v.glide_from;
        } else {
            v.pitch = target;
            v.glide_to = target;
            v.glide_t = 1.0;
        }
        Self::start_voice(v, &self.patch, &self.src, self.alt, true);
        for e in v.env.iter_mut() {
            e.gate_on(false);
        }
        self.last_pitch = target;
    }

    fn start_voice(v: &mut Voice, patch: &MorphPatch, src: &MorphSources, alt: f32, fresh: bool) {
        let ix = &layout().ix;
        v.vals.copy_from_slice(&patch.values);
        v.alt = alt;
        for r in v.rands.iter_mut() {
            *r = v.rng.unit();
        }
        v.pressure = 0.0;
        v.slide = 0.0;
        let hz = midi_hz(v.pitch);
        for o in 0..3 {
            let blk = OscBlock::read(&*v.vals, &ix.osc[o], hz, 0.0);
            v.osc[o].start(&blk, &src.osc[o], v.rng.next_u32());
        }
        v.sub.start();
        for l in 0..8 {
            let b = LfoBlock::read(&*v.vals, &ix.lfo[l]);
            v.lfo[l].trigger(&b);
        }
        if fresh {
            for f in v.filter.iter_mut() {
                f.reset();
            }
            v.amp = 0.0;
            v.dc = [DcBlock::default(); 2];
            for e in v.env.iter_mut() {
                e.kill();
            }
        }
    }

    fn voice_off(&mut self, note: u8, vel: u8) {
        let ix = &layout().ix;
        if let Some(i) = self.held[..self.nheld].iter().position(|h| h.0 == note) {
            self.held.copy_within(i + 1..self.nheld, i);
            self.nheld -= 1;
        }
        let mono = self.patch.values[ix.mono] > 0.5;
        if mono && self.nheld > 0 {
            // Back to the last note still held.
            let (n, vel) = self.held[self.nheld - 1];
            let playing_it = self.voices.iter().any(|v| v.active && v.gate && v.note == note);
            if playing_it {
                self.voice_on(n, vel);
            }
            return;
        }
        for v in self.voices.iter_mut().filter(|v| v.active && v.gate && v.note == note) {
            v.gate = false;
            v.release_vel = vel as f32 / 127.0;
            for e in v.env.iter_mut() {
                e.gate_off();
            }
        }
    }

    /// One stereo frame (blocks are made as needed).
    #[inline]
    pub fn next_frame(&mut self) -> [f32; 2] {
        if self.out_pos >= BLOCK {
            self.block();
            self.out_pos = 0;
        }
        let i = self.out_pos;
        self.out_pos += 1;
        [self.buf_l[i], self.buf_r[i]]
    }

    /// Render, OVERWRITING both slices.
    pub fn render(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let [x, y] = self.next_frame();
            *a = x;
            *b = y;
        }
    }

    fn block(&mut self) {
        let lay = layout();
        let ix = &lay.ix;
        let dt = BLOCK as f32 / self.rate;
        let patch = self.patch.clone();
        let src = self.src.clone();
        let base: &[f32] = &patch.values;
        // The arp plays the held notes.
        let arp_on = base[ix.arp.on] > 0.5;
        if arp_on {
            let n = self.arp.advance(base, &ix.arp, self.bps * dt, &mut self.arp_events);
            for k in 0..n {
                match self.arp_events[k] {
                    ArpEvent::On(note, vel) => self.voice_on(note, vel),
                    ArpEvent::Off(note) => self.voice_off(note, 64),
                }
            }
        } else if self.arp_was_on {
            for k in 0..self.voices.len() {
                if self.voices[k].gate {
                    let n = self.voices[k].note;
                    self.voice_off(n, 64);
                }
            }
        }
        self.arp_was_on = arp_on;
        // Shared (mono and free) LFOs on the clock.
        for l in 0..8 {
            let b = LfoBlock::read(base, &ix.lfo[l]);
            let clock = Self::lfo_clock(&b, self.secs, self.beats);
            self.lfo[l].advance(&b, &patch.points[l], dt, self.bps, clock);
        }
        self.buf_l = [0.0; BLOCK];
        self.buf_r = [0.0; BLOCK];
        let poly = base[ix.poly].max(1.0);
        let sounding = self.voices.iter().filter(|v| v.active).count() as f32;
        let mut noise = self.rng.bipolar();
        let lfo = self.lfo;
        for vi in 0..self.voices.len() {
            if !self.voices[vi].active {
                continue;
            }
            noise = -noise * 0.5 + self.rng.bipolar() * 0.5;
            let globals = Globals { mod_wheel: self.mod_wheel, bend: self.bend, aftertouch: self.aftertouch, noise, voices: (sounding / poly).min(1.0), bps: self.bps, secs: self.secs, beats: self.beats };
            Self::render_voice(&mut self.voices[vi], &patch, &src, &lfo, &globals, &mut self.acc, self.rate, &mut self.buf_l, &mut self.buf_r);
        }
        // The rack, then the level.
        self.rack.process(&patch.fx, base, &ix.fx, self.bps, &mut self.buf_l, &mut self.buf_r);
        let level = base[ix.level];
        for i in 0..BLOCK {
            self.buf_l[i] *= level;
            self.buf_r[i] *= level;
        }
        self.secs += dt as f64;
        self.beats += (self.bps * dt) as f64;
    }

    fn lfo_clock(b: &LfoBlock, secs: f64, beats: f64) -> f64 {
        if b.sync_beats > 0.0 { beats / b.sync_beats as f64 } else { secs * b.rate as f64 }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_voice(v: &mut Voice, patch: &MorphPatch, src: &MorphSources, glfo: &[Lfo; 8], g: &Globals, acc: &mut [f32], rate: f32, out_l: &mut [f32; BLOCK], out_r: &mut [f32; BLOCK]) {
        let lay = layout();
        let ix = &lay.ix;
        let dt = BLOCK as f32 / rate;
        let base: &[f32] = &patch.values;
        // Envelopes and LFOs move first (on last block's modulation).
        let env_scale = 1.0 + (v.rands[5] * 2.0 - 1.0) * base[ix.rand_env] * 0.5;
        let amp_from = v.amp;
        let mut envs = [0.0f32; 4];
        for e in 0..4 {
            let blk = EnvBlock::read(&*v.vals, &ix.env[e], env_scale);
            envs[e] = v.env[e].advance(&blk, dt);
        }
        let mut lfos = [[0.0f32; 2]; 8];
        let mut lfo_bip = [true; 8];
        for l in 0..8 {
            let b = LfoBlock::read(&*v.vals, &ix.lfo[l]);
            lfo_bip[l] = !b.uni;
            if b.mono {
                lfos[l] = glfo[l].out();
            } else {
                let clock = Self::lfo_clock(&b, g.secs, g.beats);
                v.lfo[l].advance(&b, &patch.points[l], dt, g.bps, clock);
                lfos[l] = v.lfo[l].out();
            }
        }
        // Sources.
        let mut s = [0.0f32; N_SOURCES];
        let mut bip = [false; N_SOURCES];
        for e in 0..4 {
            s[matrix::SRC_ENV1 + e] = envs[e];
        }
        for l in 0..8 {
            s[matrix::SRC_LFO1 + l] = lfos[l][0];
            s[matrix::SRC_LFO1Y + l] = lfos[l][1];
            bip[matrix::SRC_LFO1 + l] = lfo_bip[l];
            bip[matrix::SRC_LFO1Y + l] = lfo_bip[l];
        }
        s[matrix::SRC_VEL] = v.vel;
        s[matrix::SRC_NOTE] = ((v.note as f32 - 60.0) / 60.0).clamp(-1.0, 1.0);
        bip[matrix::SRC_NOTE] = true;
        s[matrix::SRC_RAND] = v.rands[0];
        s[matrix::SRC_RAND2] = v.rands[1];
        s[matrix::SRC_ALT] = v.alt;
        bip[matrix::SRC_ALT] = true;
        s[matrix::SRC_MOD] = g.mod_wheel;
        s[matrix::SRC_BEND] = g.bend;
        bip[matrix::SRC_BEND] = true;
        s[matrix::SRC_AFTERTOUCH] = g.aftertouch;
        s[matrix::SRC_PRESSURE] = v.pressure;
        s[matrix::SRC_SLIDE] = v.slide;
        for m in 0..8 {
            s[matrix::SRC_M1 + m] = base[ix.macros[m]];
        }
        s[matrix::SRC_CONST] = 1.0;
        s[matrix::SRC_NOISE] = g.noise;
        bip[matrix::SRC_NOISE] = true;
        s[matrix::SRC_GATE] = if v.gate { 1.0 } else { 0.0 };
        s[matrix::SRC_RELEASE_VEL] = v.release_vel;
        s[matrix::SRC_VOICES] = g.voices;
        patch.matrix.apply(&lay.schema, base, &s, &bip, acc, &mut v.vals);
        let vals: &[f32] = &v.vals;
        // Glide.
        if v.glide_t < 1.0 {
            v.glide_t = (v.glide_t + dt / v.glide_len.max(1e-4)).min(1.0);
            let t = bend(v.glide_t, vals[ix.glide_curve]);
            v.pitch = v.glide_from + (v.glide_to - v.glide_from) * t;
        }
        let detune = (v.rands[2] * 2.0 - 1.0) * vals[ix.rand_detune] / 100.0;
        let note_hz = midi_hz(v.pitch + detune);
        let pitch_semis = g.bend * vals[ix.bend_range] + vals[ix.pitch];
        let mut blk = [OscBlock::default(); 3];
        for o in 0..3 {
            blk[o] = OscBlock::read(vals, &ix.osc[o], note_hz, pitch_semis);
            if blk[o].on {
                v.osc[o].prepare(&blk[o], &src.osc[o], rate, BLOCK);
            }
        }
        let sub_on = vals.on(ix.sub.on);
        let sub_shape = vals.index(ix.sub.shape) as u8;
        let sub_inc = note_hz * ((vals[ix.sub.oct] * 12.0 + pitch_semis) / 12.0).exp2() / rate;
        let sub_level = vals[ix.sub.level];
        let (sub_l, sub_r) = pan_gains(vals[ix.sub.pan]);
        let sub_out = vals.index(ix.sub.out);
        let noise_on = vals.on(ix.noise.on);
        let noise_color = vals.index(ix.noise.color) as u8;
        let noise_stereo = vals[ix.noise.stereo];
        // Digital noise steps two octaves over the note, followed `noise_pitch` of the way from c4.
        let noise_inc = 4.0 * 261.625_58 * (note_hz * (pitch_semis / 12.0).exp2() / 261.625_58).powf(vals[ix.noise.pitch]) / rate;
        let noise_level = vals[ix.noise.level];
        let (noise_pl, noise_pr) = pan_gains(vals[ix.noise.pan]);
        let noise_out = vals.index(ix.noise.out);
        let cutoff_scale = (1.0 + (v.rands[3] * 2.0 - 1.0) * vals[ix.rand_cutoff]).max(0.05);
        let note_ratio = note_hz / 261.625_58;
        let mut fb = [FilterBlock::default(); 2];
        for f in 0..2 {
            fb[f] = FilterBlock::read(vals, &ix.filter[f], note_ratio, cutoff_scale);
            if fb[f].on {
                v.filter[f].prepare(&fb[f], rate);
            }
        }
        let parallel = vals[ix.filter_route] > 0.5;
        let vel_gain = 1.0 - vals[ix.velocity] * (1.0 - v.vel);
        let (vpl, vpr) = pan_gains((v.rands[4] * 2.0 - 1.0) * vals[ix.rand_pan]);
        let (vpl, vpr) = (vpl * std::f32::consts::SQRT_2, vpr * std::f32::consts::SQRT_2);
        let amp_to = envs[0];
        v.amp = amp_to;
        let damp = (amp_to - amp_from) / BLOCK as f32;
        let mut amp = amp_from;
        let mut lasts = [v.osc[0].last, v.osc[1].last, v.osc[2].last, 0.0, 0.0];
        let gain = 0.5 * vel_gain;
        for i in 0..BLOCK {
            let mut f1 = [0.0f32; 2];
            let mut f2 = [0.0f32; 2];
            let mut direct = [0.0f32; 2];
            let route = |out: usize, l: f32, r: f32, f1: &mut [f32; 2], f2: &mut [f32; 2], direct: &mut [f32; 2]| match out {
                0 => {
                    f1[0] += l;
                    f1[1] += r;
                }
                1 => {
                    f2[0] += l;
                    f2[1] += r;
                }
                2 => {
                    f1[0] += l;
                    f1[1] += r;
                    f2[0] += l;
                    f2[1] += r;
                }
                _ => {
                    direct[0] += l;
                    direct[1] += r;
                }
            };
            for o in 0..3 {
                let b = &blk[o];
                if !b.on {
                    continue;
                }
                let m = lasts[b.warp_src as usize % 5];
                let (l, r) = v.osc[o].tick(b, &src.osc[o], m, rate);
                lasts[o] = v.osc[o].last;
                let (pl, pr) = pan_gains(b.pan);
                let lv = b.level * std::f32::consts::SQRT_2;
                route(b.out as usize, l * pl * lv, r * pr * lv, &mut f1, &mut f2, &mut direct);
            }
            if sub_on {
                let y = v.sub.tick(sub_shape, sub_inc) * sub_level;
                lasts[3] = y;
                route(sub_out, y * sub_l, y * sub_r, &mut f1, &mut f2, &mut direct);
            }
            if noise_on {
                let (a, b) = v.noise.tick(noise_color, noise_stereo, noise_inc);
                lasts[4] = (a + b) * 0.5;
                route(noise_out, a * noise_level * noise_pl, b * noise_level * noise_pr, &mut f1, &mut f2, &mut direct);
            }
            let y1 = if fb[0].on { v.filter[0].tick(f1) } else { f1 };
            let mixed = if parallel {
                let y2 = if fb[1].on { v.filter[1].tick(f2) } else { f2 };
                [y1[0] + y2[0], y1[1] + y2[1]]
            } else {
                let x2 = [f2[0] + y1[0], f2[1] + y1[1]];
                if fb[1].on { v.filter[1].tick(x2) } else { x2 }
            };
            amp += damp;
            let a = amp * gain;
            out_l[i] += v.dc[0].process((mixed[0] + direct[0]) * a * vpl);
            out_r[i] += v.dc[1].process((mixed[1] + direct[1]) * a * vpr);
        }
        if !v.env[0].active() {
            v.active = false;
            v.gate = false;
        }
    }
}

struct Globals {
    mod_wheel: f32,
    bend: f32,
    aftertouch: f32,
    noise: f32,
    voices: f32,
    bps: f32,
    secs: f64,
    beats: f64,
}

impl crate::instrument::Instrument for Morph {
    fn name(&self) -> &str {
        "morph"
    }

    fn sample_rate(&self) -> f32 {
        self.rate
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
        Morph::note_on(self, note, velocity);
    }

    fn note_off(&mut self, note: u8) {
        Morph::note_off(self, note);
    }

    fn control(&mut self, control: crate::instrument::Control, value: f32) {
        use crate::instrument::Control;
        match control {
            Control::Modulation => self.set_mod_wheel(value),
            Control::PitchBend => {
                let range = self.patch.values[layout().ix.bend_range].max(1.0);
                self.set_bend(value / range);
            }
            _ => {}
        }
    }

    fn all_notes_off(&mut self) {
        Morph::all_notes_off(self);
    }

    fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        Morph::render(self, out_l, out_r);
    }

    fn is_active(&self) -> bool {
        self.voices.iter().any(|v| v.active)
    }
}
