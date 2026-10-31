//! The LFO (spec section 5): ten shapes (sine, triangle, square, saw,
//! noise, sample & hold, envelope, step sequencer, MSEG, formula), its own
//! DAHDSR envelope, the trigger modes (free-run from song position, key
//! trigger, random), tempo-synced / deactivated rates, unipolar and
//! amplitude, the three outputs and the EG retrigger requests.
//!
//! One `process_block` per block: the envelope and the phase advance first,
//! wrap events (noise / S&H draws, sequencer steps) happen in the block the
//! phase crosses a cycle, then the outputs are evaluated.

use super::formula::FORMULA_OUTPUTS;
use super::mseg::{Mseg, MsegRun};
use super::stepseq::StepSeq;
use super::time::{at_min, lfo_hz, lfo_phase_inc, stage_rate, LFO_RELEASE_FOREVER};
use super::ModCtx;
use crate::dsp::Rng;
use std::f32::consts::TAU;
use std::sync::OnceLock;

/// The reference fills the noise history at attack with fresh samples each
/// multiplied by the current phase (so phase 0 starts the noise at 0); mirrored.
const QUIRK_NOISE_HISTORY_TIMES_PHASE: bool = true;
/// A unipolar step sequencer clips negative steps to 0 instead of rescaling; mirrored.
const QUIRK_STEPSEQ_UNIPOLAR_CLIPS: bool = true;
/// With a deactivated rate and key trigger, a bipolar triangle's phase scrub
/// is offset by a quarter cycle like at attack; mirrored.
const QUIRK_SCRUB_TRIANGLE_QUARTER: bool = true;
/// The "Raw Waveform" output of the Envelope shape is the curve times the
/// envelope once more (E(e) e); mirrored.
const QUIRK_ENVELOPE_RAW_TIMES_ENV: bool = true;
/// A step sequencer nudges a stopped phase of exactly 0 to 0.001 so the
/// first step shows instead of the one before; mirrored.
const QUIRK_STEPSEQ_ZERO_PHASE_NUDGE: f64 = 0.001;
/// Noise and S&H are reseeded only on the first attack of a free-running LFO.
const QUIRK_FREERUN_RESEED_FIRST_ONLY: bool = true;

/// Shape (interop ids 0..9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LfoShape {
    #[default]
    Sine,
    Triangle,
    Square,
    Saw,
    Noise,
    SampleHold,
    Envelope,
    StepSeq,
    Mseg,
    Formula,
}

impl LfoShape {
    pub const NAMES: [&'static str; 10] = ["Sine", "Triangle", "Square", "Sawtooth", "Noise", "Sample & Hold", "Envelope", "Step Seq", "MSEG", "Formula"];

    pub fn from_id(i: i32) -> LfoShape {
        match i {
            1 => LfoShape::Triangle,
            2 => LfoShape::Square,
            3 => LfoShape::Saw,
            4 => LfoShape::Noise,
            5 => LfoShape::SampleHold,
            6 => LfoShape::Envelope,
            7 => LfoShape::StepSeq,
            8 => LfoShape::Mseg,
            9 => LfoShape::Formula,
            _ => LfoShape::Sine,
        }
    }

    pub fn id(self) -> i32 {
        self as i32
    }

    /// Deform types the shape offers (0 = deform has one fixed meaning).
    pub fn deform_types(self) -> i32 {
        match self {
            LfoShape::Sine | LfoShape::Triangle | LfoShape::Saw | LfoShape::Envelope => 3,
            LfoShape::SampleHold | LfoShape::StepSeq => 2,
            _ => 0,
        }
    }
}

/// Trigger modes (`trigmode`).
pub const TRIG_FREERUN: i32 = 0;
pub const TRIG_KEY: i32 = 1;
pub const TRIG_RANDOM: i32 = 2;

/// Raw (already modulated) values of one LFO's parameters for this block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LfoParams {
    pub shape: LfoShape,
    /// log2 Hz.
    pub rate: f32,
    pub rate_sync: bool,
    pub rate_deactivated: bool,
    /// Start phase (step seq: shuffle).
    pub phase: f32,
    pub phase_extend: bool,
    pub magnitude: f32,
    pub magnitude_extend: bool,
    pub deform: f32,
    /// 0, 1, 2 = deform type 1, 2, 3.
    pub deform_type: i32,
    pub trigmode: i32,
    /// `trigmode`'s deform flag: a scene LFO re-attacks on every new note
    /// (the scene decides when to attack; stored here for it).
    pub retrigger_every_note: bool,
    pub unipolar: bool,
    pub delay: f32,
    pub delay_sync: bool,
    /// Deactivated delay bypasses the envelope (e = 1).
    pub delay_deactivated: bool,
    pub attack: f32,
    pub attack_sync: bool,
    pub hold: f32,
    pub hold_sync: bool,
    pub decay: f32,
    pub decay_sync: bool,
    pub sustain: f32,
    pub release: f32,
    pub release_sync: bool,
    /// The release parameter's stored (unmodulated) value: at its maximum
    /// the envelope never releases ("Forever").
    pub release_unmodulated: f32,
    /// `<extralfo extraAmplitude>`: outputs 1 and 2 scaled by amplitude.
    pub extra_amplitude: bool,
}

impl Default for LfoParams {
    /// The init patch's LFO.
    fn default() -> Self {
        LfoParams {
            shape: LfoShape::Sine,
            rate: 0.0,
            rate_sync: false,
            rate_deactivated: false,
            phase: 0.0,
            phase_extend: false,
            magnitude: 1.0,
            magnitude_extend: false,
            deform: 0.0,
            deform_type: 0,
            trigmode: TRIG_KEY,
            retrigger_every_note: false,
            unipolar: false,
            delay: -8.0,
            delay_sync: false,
            delay_deactivated: false,
            attack: -8.0,
            attack_sync: false,
            hold: -8.0,
            hold_sync: false,
            decay: 0.0,
            decay_sync: false,
            sustain: 1.0,
            release: LFO_RELEASE_FOREVER,
            release_sync: false,
            release_unmodulated: LFO_RELEASE_FOREVER,
            extra_amplitude: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EnvStage {
    Delay,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
    Stopped,
}

/// The LFO's DAHDSR (spec 5.4).
#[derive(Clone, Copy, Debug)]
struct LfoEnv {
    stage: EnvStage,
    q: f32,
    e: f32,
    e_rel: f32,
}

impl LfoEnv {
    fn new() -> LfoEnv {
        LfoEnv { stage: EnvStage::Stopped, q: 0.0, e: 0.0, e_rel: 0.0 }
    }

    /// Note start: skip delay, then attack, then hold when each is at its
    /// minimum (only here: a stage reached later always runs, at least its
    /// minimum length).
    fn attack(&mut self, p: &LfoParams) {
        self.e = 0.0;
        let mut st = EnvStage::Delay;
        if at_min(p.delay) {
            st = EnvStage::Attack;
        }
        if st == EnvStage::Attack && at_min(p.attack) {
            self.e = 1.0;
            st = EnvStage::Hold;
        }
        if st == EnvStage::Hold && at_min(p.hold) {
            st = EnvStage::Decay;
        }
        self.enter(st);
    }

    fn enter(&mut self, st: EnvStage) {
        self.q = 0.0;
        self.stage = st;
    }

    fn release(&mut self, p: &LfoParams) {
        if p.release_unmodulated < LFO_RELEASE_FOREVER && !matches!(self.stage, EnvStage::Release | EnvStage::Stopped) {
            self.e_rel = self.e;
            self.stage = EnvStage::Release;
            self.q = 0.0;
        }
    }

    /// The envelope entered its release (and possibly finished it).
    fn in_release(&self) -> bool {
        matches!(self.stage, EnvStage::Release | EnvStage::Stopped)
    }

    fn process(&mut self, p: &LfoParams, ctx: &ModCtx) -> f32 {
        if p.delay_deactivated {
            self.e = 1.0;
            return 1.0;
        }
        let rate = match self.stage {
            EnvStage::Delay => stage_rate(p.delay, p.delay_sync, ctx),
            EnvStage::Attack => stage_rate(p.attack, p.attack_sync, ctx),
            EnvStage::Hold => stage_rate(p.hold, p.hold_sync, ctx),
            EnvStage::Decay => stage_rate(p.decay, p.decay_sync, ctx),
            EnvStage::Release => stage_rate(p.release, p.release_sync, ctx),
            EnvStage::Sustain | EnvStage::Stopped => 0.0,
        };
        self.q += rate;
        if self.q > 1.0 {
            match self.stage {
                EnvStage::Delay => self.enter(EnvStage::Attack),
                EnvStage::Attack => self.enter(EnvStage::Hold),
                EnvStage::Hold => self.enter(EnvStage::Decay),
                EnvStage::Decay => self.enter(EnvStage::Sustain),
                EnvStage::Release => self.enter(EnvStage::Stopped),
                _ => {}
            }
        }
        let s = p.sustain;
        let q = self.q;
        self.e = match self.stage {
            EnvStage::Delay => 0.0,
            EnvStage::Attack => q,
            EnvStage::Hold => 1.0,
            EnvStage::Decay => (1.0 - q) + q * s,
            EnvStage::Sustain => s,
            EnvStage::Release => (1.0 - q) * self.e_rel,
            EnvStage::Stopped => 0.0,
        };
        self.e
    }
}

/// The correlated noise generator (Noise, S&H, Envelope type 3).
#[derive(Clone, Copy, Debug, Default)]
struct CorrNoise {
    l1: f32,
    l2: f32,
}

impl CorrNoise {
    fn next(&mut self, c: f32, rng: &mut Rng) -> f32 {
        let c = c.clamp(-1.0, 1.0);
        let mut k = 0.8 * c.abs();
        k = 2.0 * k - k * k;
        let cs = if c < 0.0 { -k } else { k };
        let m = 1.0 / (1.0 - k).sqrt();
        let r = rng.bipolar();
        self.l2 = r * (1.0 - k) - cs * self.l2;
        self.l1 = self.l2 * (1.0 - k) - cs * self.l1;
        self.l1 * m
    }
}

fn sine_table() -> &'static [f32; 1024] {
    static T: OnceLock<[f32; 1024]> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0.0f32; 1024];
        for (i, v) in t.iter_mut().enumerate() {
            *v = (std::f64::consts::PI * (i as f64 - 512.0) / 512.0).sin() as f32;
        }
        t
    })
}

/// The 1024-entry sine table at index 256 v + 512 (wrapping), interpolated.
#[inline]
fn table_sine(v: f32) -> f32 {
    let t = sine_table();
    let x = 256.0 * v + 512.0;
    let f = x.floor();
    let a = x - f;
    let i0 = (f as i64).rem_euclid(1024) as usize;
    let i1 = (i0 + 1) & 1023;
    (1.0 - a) * t[i0] + a * t[i1]
}

#[inline]
fn bend1(x: f32, d: f32) -> f32 {
    let a = 0.5 * d.clamp(-3.0, 3.0);
    let x = x - a * x * x + a;
    x - a * x * x + a
}

#[inline]
fn bend2(x: f32, d: f32) -> f32 {
    x + 4.5 * d * (TAU * x).sin() / TAU
}

#[inline]
fn bend3(x: f32, d: f32) -> f32 {
    let y = x + 0.25;
    let y = y + 4.5 * d * (TAU * y).sin() / TAU;
    y - 0.25 - d / TAU
}

/// The three deform treatments of the sine / triangle / saw shapes.
fn deform_basic(base: f32, d: f32, ty: i32) -> f32 {
    match ty {
        1 => {
            let w = bend2(base, d);
            if d < -1.0 / 4.5 { w / (1.0 - (d + 1.0 / 4.5) / 1.6) } else { w }
        }
        2 => bend3(base, d) / (1.0 + 0.5 * d.abs()) - 0.06 * d,
        _ => bend1(base, d),
    }
}

/// 4-point cubic between h2 and h1 at mu, with (y0..y3) = (h3, h2, h1, h0).
#[inline]
fn cubic(h: &[f32; 4], mu: f32) -> f32 {
    let (y0, y1, y2, y3) = (h[3], h[2], h[1], h[0]);
    let a0 = y3 - y2 - y0 + y1;
    let a1 = y0 - y1 - a0;
    let a2 = y2 - y0;
    let a3 = y1;
    ((a0 * mu + a1) * mu + a2) * mu + a3
}

/// The stepped / smoothed rule shared by S&H type 2 and step-seq type 1:
/// from h2 to h1 at fraction `ph`.
fn smoothed_steps(h: &[f32; 4], ph: f32, d: f32) -> f32 {
    if d > 0.5 {
        let lin = h[2] + ph * (h[1] - h[2]);
        (2.0 - 2.0 * d) * lin + (2.0 * d - 1.0) * cubic(h, ph)
    } else if d > -0.0001 {
        let g = (ph / (2.0 * d + 1e-5)).clamp(0.0, 1.0);
        (1.0 - g) * h[2] + g * h[1]
    } else if d > -0.5 {
        let g = ((1.0 - ph) / (-2.0 * d + 1e-5)).clamp(0.0, 1.0);
        g * h[1]
    } else {
        let g = (ph / (2.0 + 2.0 * d + 1e-5)).clamp(0.0, 1.0);
        (1.0 - g) * h[1]
    }
}

/// Step-seq type 2: interpolation centred on the step midpoints.
fn centred_steps(h: &[f32; 4], ph: f32, d: f32) -> f32 {
    if d > 0.5 {
        let lin = if ph > 0.5 { h[1] + (ph - 0.5) * (h[0] - h[1]) } else { h[2] + (ph + 0.5) * (h[1] - h[2]) };
        let mu = ph;
        let b = 0.5 * (h[0] * mu * mu + h[1] * (-2.0 * mu * mu + 2.0 * mu + 1.0) + h[2] * (mu * mu - 2.0 * mu + 1.0));
        (2.0 - 2.0 * d) * lin + (2.0 * d - 1.0) * b
    } else if d >= 0.0 {
        if ph > 0.5 {
            let g = (0.5 - (ph - 1.0) / (2.0 * d + 1e-5)).clamp(0.0, 1.0);
            (1.0 - g) * h[0] + g * h[1]
        } else {
            let g = (0.5 - ph / (2.0 * d + 1e-5)).clamp(0.0, 1.0);
            (1.0 - g) * h[1] + g * h[2]
        }
    } else {
        smoothed_steps(h, ph, d)
    }
}

/// One LFO instance (voice or scene).
#[derive(Clone, Debug)]
pub struct Lfo {
    is_scene: bool,
    rng: Rng,
    phase: f64,
    cycle: i64,
    step: i32,
    scrub_step: i32,
    /// h0 (newest / look-ahead) .. h3.
    hist: [f32; 4],
    held: f32,
    shuffle_odd: bool,
    noise: CorrNoise,
    env_noise: CorrNoise,
    env: LfoEnv,
    mseg: MsegRun,
    released: bool,
    release_pending: bool,
    attacked_once: bool,
    out: [f32; 3],
    formula_out: [f32; FORMULA_OUTPUTS],
    req_aeg: bool,
    req_feg: bool,
    /// This block's LFO envelope value.
    last_env: f32,
}

impl Lfo {
    pub fn new(seed: u32, is_scene: bool) -> Lfo {
        Lfo {
            is_scene,
            rng: Rng::new(seed.wrapping_mul(0x9e37_79b9) ^ 0x5bd1_e995),
            phase: 0.0,
            cycle: 0,
            step: 0,
            scrub_step: -1,
            hist: [0.0; 4],
            held: 0.0,
            shuffle_odd: false,
            noise: CorrNoise::default(),
            env_noise: CorrNoise::default(),
            env: LfoEnv::new(),
            mseg: MsegRun::new(seed ^ 0x2545_f491),
            released: false,
            release_pending: false,
            attacked_once: false,
            out: [0.0; 3],
            formula_out: [0.0; FORMULA_OUTPUTS],
            req_aeg: false,
            req_feg: false,
            last_env: 0.0,
        }
    }

    pub fn is_scene(&self) -> bool {
        self.is_scene
    }

    /// The start phase the shape uses (wrapped into [0, 1)).
    fn start_phase(p: &LfoParams) -> f64 {
        let raw = match p.shape {
            LfoShape::StepSeq => 0.0,
            LfoShape::Mseg | LfoShape::Formula | LfoShape::Envelope => p.phase,
            _ => p.phase.clamp(0.0, 1.0 - 1.0 / 360.0),
        };
        (raw as f64).rem_euclid(1.0)
    }

    fn shape_offset(p: &LfoParams) -> f64 {
        match p.shape {
            LfoShape::Triangle if !p.unipolar => 0.25,
            LfoShape::Sine if p.unipolar => 0.75,
            _ => 0.0,
        }
    }

    /// Note start (voice) / scene attack: envelope, trigger-mode phase,
    /// shape state.
    pub fn attack(&mut self, p: &LfoParams, ctx: &ModCtx, seq: &StepSeq, mseg: &Mseg) {
        self.released = false;
        self.release_pending = false;
        self.req_aeg = false;
        self.req_feg = false;
        self.env.attack(p);
        let start = Self::start_phase(p);
        let scale = if p.shape == LfoShape::Mseg { mseg.phase_scale() as f64 } else { 1.0 };
        let total = match p.trigmode {
            TRIG_RANDOM => {
                self.step = if seq.loop_end > 0 { (self.rng.next_u32() % (seq.loop_end as u32 + 1)) as i32 } else { 0 };
                self.rng.unit() as f64
            }
            TRIG_FREERUN => {
                let t = ctx.song_pos_beats / (2.0 * ctx.tsr as f64);
                let total = start * scale + t * lfo_hz(p.rate, p.rate_sync, p.rate_deactivated, ctx.tsr);
                let cycle = total.floor() as i64;
                self.step = seq.loop_start + cycle.rem_euclid(seq.loop_len() as i64) as i32;
                total
            }
            _ => {
                self.step = 0;
                start * scale
            }
        };
        let total = total + Self::shape_offset(p);
        self.phase = total.rem_euclid(1.0);
        self.cycle = total.floor() as i64;
        let reseed = !(QUIRK_FREERUN_RESEED_FIRST_ONLY && p.trigmode == TRIG_FREERUN && self.attacked_once);
        match p.shape {
            LfoShape::Noise if reseed => {
                self.noise = CorrNoise::default();
                let c = p.deform;
                let mul = if QUIRK_NOISE_HISTORY_TIMES_PHASE { self.phase as f32 } else { 1.0 };
                // Six draws: the first three all land in h3 (two are overwritten), then h2, h1, h0.
                for k in [3, 3, 3, 2, 1, 0] {
                    self.hist[k] = self.noise.next(c, &mut self.rng) * mul;
                }
                self.phase = 0.0;
            }
            LfoShape::SampleHold if reseed => {
                if p.deform_type == 1 {
                    self.noise = CorrNoise::default();
                    for k in (0..4).rev() {
                        self.hist[k] = self.noise.next(0.0, &mut self.rng);
                    }
                    // A fifth draw goes to the held value, which the smoothed type never reads.
                    self.held = self.noise.next(0.0, &mut self.rng);
                } else {
                    for _ in 0..3 {
                        self.held = self.noise.next(p.deform, &mut self.rng);
                    }
                }
            }
            LfoShape::StepSeq => {
                self.load_steps(seq, self.step);
                self.step = seq.next_step(self.step);
                self.hist[0] = seq.value(self.step);
                self.shuffle_odd = !self.shuffle_odd;
                self.scrub_step = -1;
            }
            LfoShape::Mseg => self.mseg.reset(),
            _ => {}
        }
        self.attacked_once = true;
    }

    /// h1 = s[step], h2 = s[step-1], h3 = s[step-2].
    fn load_steps(&mut self, seq: &StepSeq, step: i32) {
        self.hist[1] = seq.value(step);
        self.hist[2] = seq.value(step - 1);
        self.hist[3] = seq.value(step - 2);
    }

    /// Restart only the envelope (mono non-legato retrigger of a voice LFO
    /// that is not key-triggered).
    pub fn restart_envelope(&mut self, p: &LfoParams) {
        self.released = false;
        self.release_pending = false;
        self.env.attack(p);
    }

    /// Note-off (voice) / last voice of the scene released. Takes effect at
    /// the next block (the envelope needs that block's parameters).
    pub fn release(&mut self) {
        if !self.released {
            self.released = true;
            self.release_pending = true;
        }
    }

    pub fn released(&self) -> bool {
        self.released
    }

    fn fire_step(&mut self, seq: &StepSeq, step: i32) {
        if self.is_scene {
            return;
        }
        let (both, feg, aeg) = seq.triggers(step);
        self.req_aeg |= both || aeg;
        self.req_feg |= both || feg;
    }

    fn on_wrap(&mut self, p: &LfoParams, seq: &StepSeq) {
        match p.shape {
            LfoShape::Noise => {
                self.hist = [self.noise.next(p.deform, &mut self.rng), self.hist[0], self.hist[1], self.hist[2]];
            }
            LfoShape::SampleHold => {
                if p.deform_type == 1 {
                    self.hist = [self.noise.next(0.0, &mut self.rng), self.hist[0], self.hist[1], self.hist[2]];
                } else {
                    self.held = self.noise.next(p.deform, &mut self.rng);
                }
            }
            LfoShape::StepSeq => {
                self.fire_step(seq, self.step);
                self.step = seq.next_step(self.step);
                self.hist = [seq.value(self.step), self.hist[0], self.hist[1], self.hist[2]];
                self.shuffle_odd = !self.shuffle_odd;
            }
            _ => {}
        }
    }

    /// One block.
    pub fn process_block(&mut self, p: &LfoParams, ctx: &ModCtx, seq: &StepSeq, mseg: &Mseg) {
        self.req_aeg = false;
        self.req_feg = false;
        if self.release_pending {
            self.release_pending = false;
            self.env.release(p);
        }
        let e = self.env.process(p, ctx);
        self.last_env = e;
        let f = lfo_phase_inc(p.rate, p.rate_sync, p.rate_deactivated, ctx) as f64;
        let stepseq = p.shape == LfoShape::StepSeq;
        if f == 0.0 && p.trigmode == TRIG_KEY {
            let mut ph = if stepseq { p.phase as f64 } else { Self::start_phase(p) };
            if QUIRK_SCRUB_TRIANGLE_QUARTER && p.shape == LfoShape::Triangle && !p.unipolar {
                ph += 0.25;
            }
            self.phase = ph.rem_euclid(1.0);
        } else {
            let ratemult = if stepseq {
                let raw = if p.phase_extend { 2.0 * p.phase - 1.0 } else { p.phase };
                let s = raw.clamp(-1.99, 1.99);
                if self.shuffle_odd { 1.0 / (1.0 - s / 2.0) } else { 1.0 / (1.0 + s / 2.0) }
            } else {
                1.0
            };
            self.phase += f * ratemult as f64;
        }
        let mut wraps = 0i64;
        if self.phase >= 2.0 {
            let n = self.phase.floor();
            self.phase -= n;
            wraps = n as i64;
        } else if self.phase >= 1.0 {
            self.phase -= 1.0;
            wraps = 1;
        } else if self.phase < 0.0 {
            let n = self.phase.floor();
            self.phase -= n;
            wraps = n as i64;
        }
        self.cycle += wraps;
        if stepseq && f == 0.0 && self.phase == 0.0 {
            self.phase = QUIRK_STEPSEQ_ZERO_PHASE_NUDGE;
        }
        for _ in 0..wraps.clamp(0, 16) {
            self.on_wrap(p, seq);
        }

        let d = p.deform;
        let ph = self.phase as f32;
        let w = match p.shape {
            LfoShape::Sine => deform_basic(table_sine(2.0 - 4.0 * ph), d, p.deform_type),
            LfoShape::Triangle => {
                let base = -1.0 + 4.0 * if ph <= 0.5 { ph } else { 1.0 - ph };
                deform_basic(base, d, p.deform_type)
            }
            LfoShape::Saw => deform_basic(1.0 - 2.0 * ph, d, p.deform_type),
            LfoShape::Square => {
                if ph <= 0.5 + 0.5 * d { 1.0 } else { -1.0 }
            }
            LfoShape::Noise => cubic(&self.hist, ph),
            LfoShape::SampleHold => {
                if p.deform_type == 1 { smoothed_steps(&self.hist, ph, d) } else { self.held }
            }
            LfoShape::StepSeq => {
                let mut frac = ph;
                if f == 0.0 {
                    let p16 = 16.0 * ph;
                    let mut idx = (p16.floor() as i32) & 15;
                    if seq.loop_end >= seq.loop_start {
                        while idx > seq.loop_end {
                            idx -= seq.loop_len();
                        }
                    }
                    frac = p16.fract();
                    self.load_steps(seq, idx);
                    self.hist[0] = seq.value(seq.next_step(idx));
                    if idx != self.scrub_step {
                        if self.scrub_step >= 0 {
                            self.fire_step(seq, idx);
                        }
                        self.scrub_step = idx;
                    }
                }
                if p.deform_type == 1 { centred_steps(&self.hist, frac, d) } else { smoothed_steps(&self.hist, frac, d) }
            }
            LfoShape::Mseg => {
                // "Released": the envelope entered release, or release is at
                // Forever and the note was released.
                let forever = p.release_unmodulated >= LFO_RELEASE_FOREVER;
                let rel = self.env.in_release() || (forever && self.released);
                let v = mseg.eval(&mut self.mseg, self.cycle as f64 + self.phase, d, rel);
                if !self.is_scene {
                    self.req_aeg |= self.mseg.retrigger_aeg;
                    self.req_feg |= self.mseg.retrigger_feg;
                }
                v
            }
            LfoShape::Envelope | LfoShape::Formula => 0.0,
        };

        let a = p.magnitude.clamp(-3.0, 3.0);
        let xa = if p.extra_amplitude { a } else { 1.0 };
        match p.shape {
            LfoShape::Envelope => {
                let curve = match p.deform_type {
                    1 => {
                        if d > 0.0 { e.powf(1.0 + 9.0 * d) } else { e.powf(1.0 / (1.0 + 4.0 * d.abs())) }
                    }
                    2 => {
                        if d < 0.0 {
                            e + 0.2 * self.env_noise.next(1.0 - d.abs(), &mut self.rng)
                        } else {
                            let k = 24.0 * d;
                            if k > 1.0 { (e * k).round() / k } else { e }
                        }
                    }
                    _ => e * ((1.0 - d) + d * e),
                };
                let raw = if QUIRK_ENVELOPE_RAW_TIMES_ENV { curve * e } else { curve };
                self.out = [a * curve, raw * xa, e * xa];
            }
            LfoShape::Formula => {
                // The script's runner writes the outputs after this block
                // (`set_formula_outputs`); without one (no script loaded) they stay 0.
                self.formula_out = [0.0; FORMULA_OUTPUTS];
                self.out = [0.0; 3];
            }
            _ => {
                let wu = if !p.unipolar {
                    w
                } else if stepseq && QUIRK_STEPSEQ_UNIPOLAR_CLIPS {
                    w.max(0.0)
                } else {
                    0.5 + 0.5 * w
                };
                self.out = [e * a * wu, wu * xa, e * xa];
            }
        }
    }

    /// (main, raw waveform, EG only) = mod sources index 0, 1, 2.
    #[inline]
    pub fn outputs(&self) -> [f32; 3] {
        self.out
    }

    /// This block's LFO envelope value (what a formula's outputs are scaled by).
    pub fn env_level(&self) -> f32 {
        self.last_env
    }

    /// A formula evaluator's outputs for this block (after `process_block`):
    /// the 16 sources as final values, and its EG retrigger requests (voice
    /// LFOs only).
    pub fn set_formula_outputs(&mut self, out: &[f32; FORMULA_OUTPUTS], aeg: bool, feg: bool) {
        self.formula_out = *out;
        self.out = [out[0], out[1], out[2]];
        if !self.is_scene {
            self.req_aeg |= aeg;
            self.req_feg |= feg;
        }
    }

    /// The formula shape's 16 outputs.
    pub fn formula_outputs(&self) -> &[f32; FORMULA_OUTPUTS] {
        &self.formula_out
    }

    /// This block's (amp EG, filter EG) retrigger requests; cleared on read.
    pub fn retrigger_requests(&mut self) -> (bool, bool) {
        let r = (self.req_aeg, self.req_feg);
        self.req_aeg = false;
        self.req_feg = false;
        r
    }

    /// Phase within the cycle and the cycle count.
    pub fn phase(&self) -> (f64, i64) {
        (self.phase, self.cycle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 48000.0;

    fn run(p: &LfoParams, seq: &StepSeq, blocks: usize) -> Vec<[f32; 3]> {
        let ctx = ModCtx::new(FS);
        let mseg = Mseg::default_voice();
        let mut l = Lfo::new(3, false);
        l.attack(p, &ctx, seq, &mseg);
        (0..blocks)
            .map(|_| {
                l.process_block(p, &ctx, seq, &mseg);
                l.outputs()
            })
            .collect()
    }

    /// Block indices where the main output rises through `level`.
    fn rising(v: &[[f32; 3]], level: f32) -> Vec<usize> {
        (1..v.len()).filter(|&i| v[i - 1][0] < level && v[i][0] >= level).collect()
    }

    fn check_spacing(c: &[usize], period: usize, what: &str) {
        assert!(c.len() >= 3, "{what}: {c:?}");
        for w in c.windows(2) {
            assert!((w[1] as i64 - w[0] as i64 - period as i64).abs() <= 1, "{what}: {c:?}");
        }
    }

    #[test]
    fn periods_at_1_and_4_hz() {
        let seq = StepSeq::default();
        for (rate, period) in [(0.0f32, 1500usize), (2.0, 375)] {
            for shape in [LfoShape::Sine, LfoShape::Triangle, LfoShape::Saw, LfoShape::Square] {
                for dt in 0..3 {
                    let p = LfoParams { shape, rate, deform_type: dt, ..LfoParams::default() };
                    let v = run(&p, &seq, period * 5);
                    let c = if shape == LfoShape::Saw {
                        (1..v.len()).filter(|&i| v[i][0] - v[i - 1][0] > 1.0).collect()
                    } else {
                        rising(&v, 0.0)
                    };
                    check_spacing(&c, period, &format!("{shape:?} {rate} {dt}"));
                }
            }
            // Tempo-synced: rate x tsr.
            let ctx = ModCtx::new(FS).with_bpm(240.0);
            let p = LfoParams { rate, rate_sync: true, ..LfoParams::default() };
            let mut l = Lfo::new(1, false);
            let m = Mseg::default_voice();
            l.attack(&p, &ctx, &seq, &m);
            let v: Vec<_> = (0..period * 3)
                .map(|_| {
                    l.process_block(&p, &ctx, &seq, &m);
                    l.outputs()
                })
                .collect();
            check_spacing(&rising(&v, 0.0), period / 2, "synced");
        }
    }

    #[test]
    fn sine_starts_at_phase() {
        let seq = StepSeq::default();
        let p = LfoParams { phase: 0.25, ..LfoParams::default() };
        let v = run(&p, &seq, 1);
        assert!((v[0][0] - 1.0).abs() < 1e-3);
        // Bipolar triangle starts at 0 moving up; unipolar sine at 0.
        let v = run(&LfoParams { shape: LfoShape::Triangle, ..LfoParams::default() }, &seq, 2);
        assert!(v[0][0].abs() < 0.01 && v[1][0] > v[0][0]);
        let v = run(&LfoParams { unipolar: true, ..LfoParams::default() }, &seq, 1);
        assert!(v[0][0] < 1e-3);
    }

    #[test]
    fn unipolar_range() {
        let seq = StepSeq::default();
        for shape in [LfoShape::Sine, LfoShape::Triangle, LfoShape::Saw, LfoShape::Square, LfoShape::SampleHold] {
            let p = LfoParams { shape, rate: 3.0, unipolar: true, ..LfoParams::default() };
            let v = run(&p, &seq, 4000);
            let lo = v.iter().fold(f32::MAX, |a, x| a.min(x[0]));
            let hi = v.iter().fold(f32::MIN, |a, x| a.max(x[0]));
            assert!(lo >= -1e-6 && hi <= 1.0 + 1e-6, "{shape:?} {lo} {hi}");
            assert!(hi - lo > 0.5, "{shape:?} {lo} {hi}");
        }
    }

    #[test]
    fn sample_hold_holds_per_cycle() {
        let seq = StepSeq::default();
        let p = LfoParams { shape: LfoShape::SampleHold, rate: 2.0, ..LfoParams::default() };
        let v = run(&p, &seq, 375 * 8);
        let changes: Vec<usize> = (1..v.len()).filter(|&i| v[i][0] != v[i - 1][0]).collect();
        assert!(changes.len() >= 7);
        check_spacing(&changes, 375, "s&h");
        assert!(v[0][0] != 0.0);
    }

    #[test]
    fn noise_is_continuous() {
        let seq = StepSeq::default();
        let p = LfoParams { shape: LfoShape::Noise, rate: 0.0, phase: 0.5, ..LfoParams::default() };
        let v = run(&p, &seq, 1500 * 10);
        let max_d = (1..v.len()).map(|i| (v[i][0] - v[i - 1][0]).abs()).fold(0.0, f32::max);
        let span = v.iter().fold(0.0f32, |a, x| a.max(x[0].abs()));
        assert!(max_d < 0.02 && span > 0.1, "{max_d} {span}");
    }

    #[test]
    fn step_sequencer_steps_and_triggers() {
        let mut seq = StepSeq::default();
        for i in 0..16 {
            seq.steps[i] = i as f32 / 16.0;
        }
        seq.trigmask = 1 << 3 | 1 << (32 + 5);
        let p = LfoParams { shape: LfoShape::StepSeq, rate: 2.0, ..LfoParams::default() };
        let ctx = ModCtx::new(FS);
        let m = Mseg::default_voice();
        let mut l = Lfo::new(1, false);
        l.attack(&p, &ctx, &seq, &m);
        let mut trig = Vec::new();
        let mut vals = Vec::new();
        for b in 0..375 * 18 {
            l.process_block(&p, &ctx, &seq, &m);
            vals.push(l.outputs()[0]);
            let r = l.retrigger_requests();
            if r.0 || r.1 {
                trig.push((b, r));
            }
        }
        for k in 0..18 {
            let mid = k * 375 + 187;
            assert_eq!(vals[mid], (k % 16) as f32 / 16.0, "step {k}");
        }
        for k in 1..17 {
            let at = (1..vals.len()).find(|&i| vals[i] == (k % 16) as f32 / 16.0 && vals[i - 1] != vals[i] && i + 10 > k * 375).unwrap();
            assert!((at as i64 - (k * 375) as i64).abs() <= 1, "step {k} at {at}");
        }
        assert_eq!(trig.len(), 2, "{trig:?}");
        assert!((trig[0].0 as i64 - 3 * 375).abs() <= 1 && trig[0].1 == (true, true));
        assert!((trig[1].0 as i64 - 5 * 375).abs() <= 1 && trig[1].1 == (true, false));
        // Loop markers and unipolar clipping.
        seq.loop_start = 2;
        seq.loop_end = 3;
        seq.steps[3] = -0.5;
        let p = LfoParams { unipolar: true, ..p };
        let v = run(&p, &seq, 375 * 6);
        assert_eq!(v[187][0], 0.0);
        assert_eq!(v[375 + 187][0], 1.0 / 16.0);
        assert_eq!(v[2 * 375 + 187][0], 2.0 / 16.0);
        assert_eq!(v[3 * 375 + 187][0], 0.0);
        assert_eq!(v[4 * 375 + 187][0], 2.0 / 16.0);
        // Scene LFOs raise no trigger requests.
        let mut s = Lfo::new(1, true);
        seq.trigmask = u64::MAX;
        s.attack(&p, &ctx, &seq, &m);
        for _ in 0..1000 {
            s.process_block(&p, &ctx, &seq, &m);
            assert_eq!(s.retrigger_requests(), (false, false));
        }
    }

    #[test]
    fn deactivated_rate_scrubs() {
        let seq = StepSeq::default();
        let ctx = ModCtx::new(FS);
        let m = Mseg::default_voice();
        let mut l = Lfo::new(1, false);
        let mut p = LfoParams { shape: LfoShape::Saw, rate_deactivated: true, ..LfoParams::default() };
        l.attack(&p, &ctx, &seq, &m);
        for ph in [0.1f32, 0.4, 0.7] {
            p.phase = ph;
            l.process_block(&p, &ctx, &seq, &m);
            assert!((l.outputs()[0] - (1.0 - 2.0 * ph)).abs() < 1e-5);
        }
    }

    #[test]
    fn envelope_and_outputs() {
        let seq = StepSeq::default();
        let ctx = ModCtx::new(FS);
        let m = Mseg::default_voice();
        let p = LfoParams {
            shape: LfoShape::Envelope,
            attack: -1.0,
            decay: -2.0,
            sustain: 0.4,
            release: -2.0,
            release_unmodulated: -2.0,
            magnitude: 0.5,
            ..LfoParams::default()
        };
        let mut l = Lfo::new(1, false);
        l.attack(&p, &ctx, &seq, &m);
        let mut peak_at = 0;
        for b in 1..3000 {
            l.process_block(&p, &ctx, &seq, &m);
            if l.outputs()[2] >= 0.999 && peak_at == 0 {
                peak_at = b;
            }
        }
        assert!((peak_at as i64 - 750).abs() <= 2, "{peak_at}");
        assert!((l.outputs()[0] - 0.5 * 0.4).abs() < 1e-5);
        assert!((l.outputs()[2] - 0.4).abs() < 1e-5);
        l.release();
        let mut n = 0;
        while l.outputs()[2] > 0.0 {
            l.process_block(&p, &ctx, &seq, &m);
            n += 1;
        }
        assert!((n as i64 - 376).abs() <= 2, "{n}");
        // Release Forever: no release.
        let p2 = LfoParams { release_unmodulated: LFO_RELEASE_FOREVER, ..p };
        let mut l = Lfo::new(1, false);
        l.attack(&p2, &ctx, &seq, &m);
        for _ in 0..3000 {
            l.process_block(&p2, &ctx, &seq, &m);
        }
        l.release();
        for _ in 0..3000 {
            l.process_block(&p2, &ctx, &seq, &m);
        }
        assert!((l.outputs()[2] - 0.4).abs() < 1e-5);
        // Delay deactivated bypasses the envelope.
        let p3 = LfoParams { shape: LfoShape::Sine, phase: 0.25, delay_deactivated: true, attack: 2.0, ..LfoParams::default() };
        let v = run(&p3, &seq, 1);
        assert!((v[0][0] - 1.0).abs() < 1e-3 && v[0][2] == 1.0);
        // Delay holds the output at 0.
        let p4 = LfoParams { delay: -2.0, ..p3 };
        let p4 = LfoParams { delay_deactivated: false, ..p4 };
        let v = run(&p4, &seq, 400);
        assert_eq!(v[300][0], 0.0);
        assert!(v[399][2] > 0.0);
    }

    #[test]
    fn mseg_shape_and_formula() {
        let seq = StepSeq::default();
        let ctx = ModCtx::new(FS);
        let m = Mseg::default_scene();
        let p = LfoParams { shape: LfoShape::Mseg, ..LfoParams::default() };
        let mut l = Lfo::new(1, true);
        l.attack(&p, &ctx, &seq, &m);
        let mut v = Vec::new();
        for _ in 0..1500 {
            l.process_block(&p, &ctx, &seq, &m);
            v.push(l.outputs()[0]);
        }
        assert!((v[374] - 1.0).abs() < 2e-3 && (v[1124] + 1.0).abs() < 2e-3, "{} {}", v[374], v[1124]);
        let p = LfoParams { shape: LfoShape::Formula, ..LfoParams::default() };
        let v = run(&p, &seq, 10);
        assert!(v.iter().all(|o| *o == [0.0; 3]), "no runner: silent");
    }

    #[test]
    fn freerun_follows_song_position() {
        let seq = StepSeq::default();
        let m = Mseg::default_voice();
        let mut ctx = ModCtx::new(FS);
        ctx.song_pos_beats = 0.5; // 0.25 s at 120 BPM: a quarter cycle at 1 Hz
        let p = LfoParams { trigmode: TRIG_FREERUN, ..LfoParams::default() };
        let mut l = Lfo::new(1, false);
        l.attack(&p, &ctx, &seq, &m);
        assert!((l.phase().0 - 0.25).abs() < 1e-9);
        let p = LfoParams { trigmode: TRIG_RANDOM, ..p };
        l.attack(&p, &ctx, &seq, &m);
        assert!((0.0..1.0).contains(&l.phase().0));
    }
    /// The attack draws of the noise shapes (MODULATION 5.5): Noise makes six draws (the first
    /// three into h3), each times the start phase; smoothed S&H four fresh draws at correlation 0
    /// into h3..h0 plus a fifth into the held value; the next wrap draws one more.
    #[test]
    fn noise_attack_draws() {
        let seq = StepSeq::default();
        let ctx = ModCtx::new(FS);
        let m = Mseg::default_voice();
        let seed = 7u32;
        let fresh = || Rng::new(seed.wrapping_mul(0x9e37_79b9) ^ 0x5bd1_e995);
        let p = LfoParams { shape: LfoShape::Noise, phase: 0.5, deform: 0.3, ..LfoParams::default() };
        let mut l = Lfo::new(seed, false);
        l.attack(&p, &ctx, &seq, &m);
        let (mut rng, mut n) = (fresh(), CorrNoise::default());
        let d: Vec<f32> = (0..6).map(|_| n.next(0.3, &mut rng) * 0.5).collect();
        assert_eq!(l.hist, [d[5], d[4], d[3], d[2]]);
        assert_eq!(l.phase, 0.0);
        let p = LfoParams { shape: LfoShape::SampleHold, deform_type: 1, deform: 0.8, ..LfoParams::default() };
        let mut l = Lfo::new(seed, false);
        l.attack(&p, &ctx, &seq, &m);
        let (mut rng, mut n) = (fresh(), CorrNoise::default());
        let d: Vec<f32> = (0..6).map(|_| n.next(0.0, &mut rng)).collect();
        assert_eq!(l.hist, [d[3], d[2], d[1], d[0]]);
        assert_eq!(l.held, d[4]);
        l.on_wrap(&p, &seq);
        assert_eq!(l.hist, [d[5], d[3], d[2], d[1]]);
    }

    /// A hold at its minimum is skipped only at the note start: after an attack it runs its
    /// minimum length (2^-8 s, ~6 blocks) at full level before the decay.
    #[test]
    fn minimum_hold_runs_after_an_attack() {
        let seq = StepSeq::default();
        let p = LfoParams { shape: LfoShape::Envelope, deform: 0.0, attack: -5.0, decay: -2.0, sustain: 0.0, ..LfoParams::default() };
        let v = run(&p, &seq, 200);
        let full = v.iter().filter(|o| o[2] >= 0.9999).count();
        assert!((5..=7).contains(&full), "{full} blocks at full level");
        let p = LfoParams { attack: -8.0, ..p };
        let v = run(&p, &seq, 200);
        assert!(v[0][2] < 1.0, "attack and hold skipped at the start: decaying from the first block");
    }

    /// Triangle and Saw take Sine's three deform treatments (Q12).
    #[test]
    fn triangle_saw_deform_treatments() {
        let seq = StepSeq::default();
        let ctx = ModCtx::new(FS);
        let m = Mseg::default_voice();
        for shape in [LfoShape::Triangle, LfoShape::Saw] {
            for (dt, d) in [(0, 0.6f32), (1, -0.5), (1, 0.4), (2, -0.7), (2, 0.9)] {
                let p = LfoParams { shape, deform_type: dt, deform: d, rate: 1.0, ..LfoParams::default() };
                let mut l = Lfo::new(1, false);
                l.attack(&p, &ctx, &seq, &m);
                for _ in 0..37 {
                    l.process_block(&p, &ctx, &seq, &m);
                }
                let ph = l.phase().0 as f32;
                let base = match shape {
                    LfoShape::Triangle => -1.0 + 4.0 * if ph <= 0.5 { ph } else { 1.0 - ph },
                    _ => 1.0 - 2.0 * ph,
                };
                let w = match dt {
                    0 => bend1(base, d),
                    1 if d < -1.0 / 4.5 => bend2(base, d) / (1.0 - (d + 1.0 / 4.5) / 1.6),
                    1 => bend2(base, d),
                    _ => bend3(base, d) / (1.0 + 0.5 * d.abs()) - 0.06 * d,
                };
                assert!((l.outputs()[1] - w).abs() < 1e-5, "{shape:?} {dt} {d}: {:?} vs {w}", l.outputs());
            }
        }
    }

    #[test]
    fn shuffle_first_step_is_shorter_and_toggle_survives_attack() {
        let mut seq = StepSeq::default();
        for i in 0..16 {
            seq.steps[i] = i as f32 / 16.0;
        }
        // 4 Hz steps = 375 blocks; shuffle 0.5: odd steps x 1/(1 - 0.25), even x 1/(1 + 0.25).
        let p = LfoParams { shape: LfoShape::StepSeq, rate: 2.0, phase: 0.5, ..LfoParams::default() };
        let ctx = ModCtx::new(FS);
        let m = Mseg::default_voice();
        let mut l = Lfo::new(1, false);
        let changes = |l: &mut Lfo, n: usize| {
            let mut out = Vec::new();
            let mut prev = f32::NAN;
            for b in 0..n {
                l.process_block(&p, &ctx, &seq, &m);
                let v = l.outputs()[0];
                if b > 0 && v != prev {
                    out.push(b);
                }
                prev = v;
            }
            out
        };
        l.attack(&p, &ctx, &seq, &m);
        let c = changes(&mut l, 1600);
        let short = (375.0f32 * 0.75).round() as i64;
        let long = (375.0f32 * 1.25).round() as i64;
        assert!((c[0] as i64 - short).abs() <= 1, "first step after a fresh attack is the shorter: {c:?}");
        assert!((c[1] as i64 - c[0] as i64 - long).abs() <= 1, "{c:?}");
        assert!((c[2] as i64 - c[1] as i64 - short).abs() <= 1, "{c:?}");
        // Four wraps since the attack (281, 750, 1031, 1500): the toggle is on;
        // a re-attack flips it off (no reset), so the first step is now the longer one.
        l.attack(&p, &ctx, &seq, &m);
        let c = changes(&mut l, 1000);
        assert!((c[0] as i64 - long).abs() <= 1, "re-attack continues the alternation: {c:?}");
    }
}
