//! Shared conventions of the hybrid family (the Tandem layout and the
//! modules it brings into the kit: `flt`, `shaper`, `oscs`, `fxs`, `mods`):
//! block sizes, the pitch / dB / envelope-rate maths, the cubic amplitude
//! law, the pan law, one-pole lags, slot values with their per-value flags
//! (tempo sync, extended range, absolute, deactivated, deform) and slot
//! definitions (what a type-dependent parameter slot means), the generic
//! smoothed biquad and the polyphase half-band 2x up/down sampler.
//!
//! Units: "note vs A440" means f = 440 * 2^(n/12); "log2 seconds" means
//! t = 2^v s; "log2 Hz" means f = 2^v Hz; "amplitude" a has gain a^3.

use super::schema::{Curve, Unit};
use std::f64::consts::PI;
use std::sync::OnceLock;

/// Host-rate block (samples at fs).
pub const BLOCK: usize = 32;
/// Oversampled block (samples at 2 fs) a voice renders per call.
pub const BLOCK_OS: usize = 64;
/// Voices oversample by this factor.
pub const OVERSAMPLE: usize = 2;

struct Tables {
    pitch: [f32; 512],
    db: [f32; 512],
    two_to_the: [f32; 1001],
    glide_log: [f32; 512],
    glide_exp: [f32; 512],
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = Tables { pitch: [0.0; 512], db: [0.0; 512], two_to_the: [0.0; 1001], glide_log: [0.0; 512], glide_exp: [0.0; 512] };
        for i in 0..512 {
            t.pitch[i] = 2f64.powf((i as f64 - 256.0) / 12.0) as f32;
            t.db[i] = 10f64.powf((i as f64 - 384.0) / 20.0) as f32;
            t.glide_log[i] = ((1.0 + 10.0 * i as f64 / 512.0).log2() / 11f64.log2()) as f32;
        }
        for i in 0..512 {
            t.glide_exp[511 - i] = 1.0 - t.glide_log[i];
        }
        for j in 0..1001 {
            t.two_to_the[j] = 2f64.powf(j as f64 / 12000.0) as f32;
        }
        t
    })
}

/// 2^(n/12) by the reference's tables (n clamped so n + 256 is in [1e-4, 512)).
pub fn note_to_pitch(n: f32) -> f32 {
    let t = tables();
    let x = (n + 256.0).clamp(1e-4, 511.9999);
    let e = x.floor() as usize;
    let a = (x - e as f32) * 1000.0;
    let b = (a.floor() as usize).min(999);
    let f = a - b as f32;
    t.pitch[e] * ((1.0 - f) * t.two_to_the[b] + f * t.two_to_the[b + 1])
}

/// Hz of a note relative to A440 (0 = 440 Hz).
#[inline]
pub fn note_hz(n: f32) -> f32 {
    440.0 * note_to_pitch(n)
}

/// Hz of a MIDI note (69 = A440).
#[inline]
pub fn midi_hz(m: f32) -> f32 {
    440.0 * note_to_pitch(m - 69.0)
}

/// 10^(d/20) by the reference's table (-384..+127 dB, interpolated).
pub fn db_to_linear(d: f32) -> f32 {
    let t = tables();
    let x = (d + 384.0).clamp(0.0, 510.9999);
    let e = x.floor() as usize;
    let a = x - e as f32;
    (1.0 - a) * t.db[e] + a * t.db[e + 1]
}

/// Linear gain of an amplitude value: a^3 (negative is silence).
#[inline]
pub fn amp_to_linear(a: f32) -> f32 {
    let a = a.max(0.0);
    a * a * a
}

/// The amplitude value of a linear gain.
#[inline]
pub fn linear_to_amp(g: f32) -> f32 {
    g.clamp(1e-10, 1.0).cbrt()
}

/// Displayed dB of an amplitude value.
pub fn amp_to_db(a: f32) -> f32 {
    18.0 * a.max(1e-10).log2()
}

/// Per-block phase increment of a stage lasting 2^x s, for a block of
/// `block_secs` (index interpolation of the reference's 1/16-octave table;
/// extrapolates past the edges like it).
pub fn envelope_rate_linear(x: f32, block_secs: f32) -> f32 {
    let q = 16.0 * x + 256.0;
    let e = q.floor().clamp(0.0, 510.0);
    let a = q - e;
    let t0 = block_secs * (-(e - 256.0) / 16.0).exp2();
    let t1 = block_secs * (-(e + 1.0 - 256.0) / 16.0).exp2();
    (1.0 - a) * t0 + a * t1
}

/// Per-block one-pole coefficient reaching -60 dB in 2^x s.
pub fn envelope_rate_onepole(x: f32, block_secs: f32) -> f32 {
    let k = 1.0 / envelope_rate_linear(x, block_secs).max(1e-12);
    1.0 - ((1e-3f32).ln() / k).exp()
}

/// The portamento curves (p in 0..1).
pub fn glide_log(p: f32) -> f32 {
    interp512(&tables().glide_log, p)
}
pub fn glide_exp(p: f32) -> f32 {
    interp512(&tables().glide_exp, p)
}
fn interp512(t: &[f32; 512], p: f32) -> f32 {
    let x = (p.clamp(0.0, 1.0)) * 511.0;
    let e = (x.floor() as usize).min(510);
    let a = x - e as f32;
    (1.0 - a) * t[e] + a * t[e + 1]
}

/// The pan law: gains (L, R) for a position x (clamped to -2..2); 0 gives
/// (1, 1), +1 gives (0, 1.5). Not constant power; past +-1 it inverts.
#[inline]
pub fn megapan(x: f32) -> (f32, f32) {
    let x = x.clamp(-2.0, 2.0);
    (1.0 - 0.75 * x - 0.25 * x * x, 1.0 + 0.75 * x - 0.25 * x * x)
}

/// Snap a tempo-synced log2 value to straight / dotted / triplet.
pub fn temposync_quantize(x: f32) -> f32 {
    let n = x.floor();
    let b = (x - n).exp2();
    let f = if b > 1.41 {
        1.5f32.log2()
    } else if b > 1.167 {
        (4.0f32 / 3.0).log2()
    } else {
        0.0
    };
    n + f
}

/// The cubic soft clip of the filter block: x - 4/27 x^3, +-1 past 1.5.
#[inline]
pub fn softclip(x: f32) -> f32 {
    let x = x.clamp(-1.5, 1.5);
    x - (4.0 / 27.0) * x * x * x
}

/// A one-pole lag: `v += rate * (target - v)` per `process`.
#[derive(Clone, Copy, Debug)]
pub struct Lag {
    pub v: f32,
    pub target: f32,
    pub rate: f32,
    first: bool,
}

impl Lag {
    pub fn new(rate: f32) -> Lag {
        Lag { v: 0.0, target: 0.0, rate, first: true }
    }
    /// The first target snaps.
    pub fn set(&mut self, t: f32) {
        self.target = t;
        if self.first {
            self.v = t;
            self.first = false;
        }
    }
    pub fn snap(&mut self, t: f32) {
        self.target = t;
        self.v = t;
        self.first = false;
    }
    #[inline]
    pub fn process(&mut self) -> f32 {
        self.v += self.rate * (self.target - self.v);
        self.v
    }
}

impl Default for Lag {
    fn default() -> Self {
        Lag::new(0.004)
    }
}

/// A value moving linearly to a new target across one block.
#[derive(Clone, Copy, Debug, Default)]
pub struct BlockLine {
    pub v: f32,
    pub dv: f32,
    started: bool,
}

impl BlockLine {
    /// Ramp from the current value to `t` over `n` samples (the first
    /// call jumps).
    pub fn set(&mut self, t: f32, n: usize) {
        if !self.started {
            self.v = t;
            self.dv = 0.0;
            self.started = true;
        } else {
            self.dv = (t - self.v) / n as f32;
        }
    }
    pub fn snap(&mut self, t: f32) {
        self.v = t;
        self.dv = 0.0;
        self.started = true;
    }
    /// The value for this sample, then step.
    #[inline]
    pub fn next(&mut self) -> f32 {
        let v = self.v;
        self.v += self.dv;
        v
    }
}

/// One parameter value as a patch stores it: the raw value in native units
/// plus its flags. Integer kinds keep their integer in `v`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SlotValue {
    pub v: f32,
    pub int: bool,
    pub temposync: bool,
    pub extend: bool,
    pub absolute: bool,
    pub deactivated: bool,
    pub deform: i32,
}

impl SlotValue {
    pub const fn float(v: f32) -> SlotValue {
        SlotValue { v, int: false, temposync: false, extend: false, absolute: false, deactivated: false, deform: 0 }
    }
    pub const fn int(i: i32) -> SlotValue {
        SlotValue { v: i as f32, int: true, temposync: false, extend: false, absolute: false, deactivated: false, deform: 0 }
    }
    #[inline]
    pub fn i(&self) -> i32 {
        self.v as i32
    }
    #[inline]
    pub fn on(&self) -> bool {
        self.v > 0.5
    }
}

/// What a slot's value means (drives display, the AI line and the schema).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotUnit {
    None,
    /// 0..1 shown x100.
    Percent,
    /// Semitones (pitch offsets, EG amounts).
    Semis,
    /// Note vs A440: f = 440 * 2^(v/12).
    NoteA440,
    /// MIDI note: f = 440 * 2^((v-69)/12).
    MidiNote,
    /// t = 2^v s (tempo-syncable times).
    Log2Secs,
    /// f = 2^v Hz (rates).
    Log2Hz,
    Db,
    /// a^3 gain.
    Amp,
    Hz,
    Secs,
    Ms,
    Cents,
}

impl SlotUnit {
    /// The schema's curve and unit for this slot.
    pub fn schema(self, int: bool, choices: bool) -> (Curve, Unit) {
        if choices {
            return (Curve::Choice, Unit::None);
        }
        if int {
            return (Curve::Int, Unit::None);
        }
        let u = match self {
            SlotUnit::Percent => Unit::Percent,
            SlotUnit::Semis | SlotUnit::NoteA440 | SlotUnit::MidiNote => Unit::Semis,
            SlotUnit::Db => Unit::Db,
            SlotUnit::Hz => Unit::Hz,
            SlotUnit::Secs => Unit::Secs,
            SlotUnit::Ms => Unit::Ms,
            SlotUnit::Cents => Unit::Cents,
            _ => Unit::None,
        };
        (Curve::Linear, u)
    }

    /// The short note the AI line carries for units the schema cannot say.
    pub fn about(self) -> &'static str {
        match self {
            SlotUnit::NoteA440 => "note vs A440 (0 = 440 Hz, 12 = an octave up)",
            SlotUnit::MidiNote => "MIDI note (69 = A440)",
            SlotUnit::Log2Secs => "log2 seconds (0 = 1 s, -1 = 0.5 s)",
            SlotUnit::Log2Hz => "log2 Hz (0 = 1 Hz, 3 = 8 Hz)",
            SlotUnit::Amp => "amplitude (gain = v^3)",
            _ => "",
        }
    }
}

/// What a type-dependent slot (an oscillator's param0..6, an effect's
/// p0..p11) means for one type. `name` empty = unused.
#[derive(Clone, Copy, Debug)]
pub struct SlotDef {
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub int: bool,
    pub unit: SlotUnit,
    pub can_extend: bool,
    pub can_deactivate: bool,
    pub can_temposync: bool,
    pub can_absolute: bool,
    /// Names of the deform (variant) options, empty = none.
    pub deform: &'static [&'static str],
    /// For int slots: the names of the values.
    pub choices: &'static [&'static str],
    pub about: &'static str,
}

impl SlotDef {
    pub const UNUSED: SlotDef = SlotDef {
        name: "",
        min: 0.0,
        max: 0.0,
        default: 0.0,
        int: true,
        unit: SlotUnit::None,
        can_extend: false,
        can_deactivate: false,
        can_temposync: false,
        can_absolute: false,
        deform: &[],
        choices: &[],
        about: "",
    };

    pub const fn float(name: &'static str, min: f32, max: f32, default: f32, unit: SlotUnit) -> SlotDef {
        SlotDef { name, min, max, default, int: false, unit, ..SlotDef::UNUSED }
    }

    pub const fn int(name: &'static str, min: f32, max: f32, default: f32) -> SlotDef {
        SlotDef { name, min, max, default, int: true, unit: SlotUnit::None, ..SlotDef::UNUSED }
    }

    pub const fn choice(name: &'static str, choices: &'static [&'static str], default: f32) -> SlotDef {
        SlotDef { name, min: 0.0, max: (choices.len() as f32) - 1.0, default, int: true, unit: SlotUnit::None, choices, ..SlotDef::UNUSED }
    }

    pub fn used(&self) -> bool {
        !self.name.is_empty()
    }

    /// The slot's default value with default flags.
    pub fn default_value(&self) -> SlotValue {
        if self.int { SlotValue::int(self.default as i32) } else { SlotValue::float(self.default) }
    }
}

/// The generic stereo biquad: transposed direct form II in f64, each of
/// the five normalised coefficients smoothed per sample (v = 0.996 v +
/// 0.004 t), snapped on the first set.
#[derive(Clone, Copy, Debug, Default)]
pub struct Biquad {
    v: [f64; 5],
    t: [f64; 5],
    r: [[f64; 2]; 2],
    first_set: bool,
    set_once: bool,
}

impl Biquad {
    pub fn new() -> Biquad {
        Biquad::default()
    }

    /// Clear state; the next coefficient set snaps.
    pub fn suspend(&mut self) {
        self.r = [[0.0; 2]; 2];
        self.set_once = false;
    }

    pub fn set_coefs(&mut self, a0: f64, a1: f64, a2: f64, b0: f64, b1: f64, b2: f64) {
        let ia = 1.0 / a0;
        self.t = [b0 * ia, b1 * ia, b2 * ia, a1 * ia, a2 * ia];
        if !self.set_once {
            self.v = self.t;
            self.set_once = true;
        }
        self.first_set = true;
    }

    pub fn instantize(&mut self) {
        self.v = self.t;
    }

    #[inline]
    fn smooth(&mut self) {
        for k in 0..5 {
            self.v[k] = 0.996 * self.v[k] + 0.004 * self.t[k];
        }
    }

    #[inline]
    fn run(&mut self, ch: usize, u: f64) -> f64 {
        let [b0, b1, b2, a1, a2] = self.v;
        let r = &mut self.r[ch];
        let y = b0 * u + r[0];
        r[0] = b1 * u - a1 * y + r[1];
        r[1] = b2 * u - a2 * y;
        y
    }

    /// One sample per channel, coefficients smoothed per sample.
    #[inline]
    pub fn tick(&mut self, l: f32, r: f32) -> (f32, f32) {
        self.smooth();
        (self.run(0, l as f64) as f32, self.run(1, r as f64) as f32)
    }

    #[inline]
    pub fn tick_mono(&mut self, x: f32) -> f32 {
        self.smooth();
        self.run(0, x as f64) as f32
    }

    /// A stereo block in place (per-sample smoothing), states flushed after.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for i in 0..l.len().min(r.len()) {
            let (a, b) = self.tick(l[i], r[i]);
            l[i] = a;
            r[i] = b;
        }
        self.flush();
    }

    /// A mono block in place.
    pub fn process_mono(&mut self, x: &mut [f32]) {
        for s in x.iter_mut() {
            *s = self.tick_mono(*s);
        }
        self.flush();
    }

    /// The "slow lag" variant: coefficients advance once per block.
    pub fn process_slowlag(&mut self, l: &mut [f32], r: &mut [f32]) {
        self.smooth();
        for i in 0..l.len().min(r.len()) {
            l[i] = self.run(0, l[i] as f64) as f32;
            r[i] = self.run(1, r[i] as f64) as f32;
        }
        self.flush();
    }

    fn flush(&mut self) {
        for ch in &mut self.r {
            for s in ch.iter_mut() {
                if s.abs() < 1e-30 {
                    *s = 0.0;
                }
            }
        }
    }

    /// Gain at `omega` (rad/sample) of the target coefficients.
    pub fn response(&self, omega: f64) -> f64 {
        let [b0, b1, b2, a1, a2] = self.t;
        let (c1, s1, c2, s2) = (omega.cos(), omega.sin(), (2.0 * omega).cos(), (2.0 * omega).sin());
        let nr = b0 + b1 * c1 + b2 * c2;
        let ni = -(b1 * s1 + b2 * s2);
        let dr = 1.0 + a1 * c1 + a2 * c2;
        let di = -(a1 * s1 + a2 * s2);
        ((nr * nr + ni * ni) / (dr * dr + di * di)).sqrt()
    }

    // ---- recipes (omega in rad/sample, c = cos, s = sin, alpha = s/2Q) ----

    pub fn lp(&mut self, omega: f64, q: f64) {
        if omega > PI {
            return self.set_coefs(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        }
        let (s, c) = omega.sin_cos();
        let al = s / (2.0 * q);
        self.set_coefs(1.0 + al, -2.0 * c, 1.0 - al, (1.0 - c) * 0.5, 1.0 - c, (1.0 - c) * 0.5);
    }

    pub fn lp2b(&mut self, omega: f64, q: f64) {
        if omega > PI {
            return self.set_coefs(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        }
        let (s, c) = omega.sin_cos();
        let al = s / (2.0 * q);
        let w2 = omega * omega;
        let p2 = PI * PI;
        let den = w2 * w2 + p2 * p2 + w2 * p2 * (1.0 / q - 2.0);
        let g1 = ((w2 * w2 / den).sqrt() * 0.5).min(1.0);
        let a = 2.0 * g1.sqrt() * (2.0 - g1).sqrt();
        let b0 = (1.0 - c + g1 * (1.0 + c) + a * s) * 0.5;
        let b1 = 1.0 - c - g1 * (1.0 + c);
        let b2 = (1.0 - c + g1 * (1.0 + c) - a * s) * 0.5;
        self.set_coefs(1.0 + al, -2.0 * c, 1.0 - al, b0, b1, b2);
    }

    pub fn hp(&mut self, omega: f64, q: f64) {
        if omega > PI {
            return self.set_coefs(1.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        }
        let (s, c) = omega.sin_cos();
        let al = s / (2.0 * q);
        self.set_coefs(1.0 + al, -2.0 * c, 1.0 - al, (1.0 + c) * 0.5, -(1.0 + c), (1.0 + c) * 0.5);
    }

    pub fn bp(&mut self, omega: f64, q: f64) {
        let (s, c) = omega.sin_cos();
        let al = s / (2.0 * q);
        self.set_coefs(1.0 + al, -2.0 * c, 1.0 - al, al, 0.0, -al);
    }

    pub fn bp2a(&mut self, omega: f64, bw: f64) {
        self.bp(omega, 1.0 / (0.02 + 30.0 * bw * bw));
    }

    pub fn pka(&mut self, omega: f64, r: f64) {
        let r = r.clamp(0.0, 1.0);
        let q = 0.1 + 10.0 * r * r;
        let (s, c) = omega.sin_cos();
        let al = s / (2.0 * q);
        self.set_coefs(1.0 + al, -2.0 * c, 1.0 - al, q * al, 0.0, -q * al);
    }

    pub fn notch(&mut self, omega: f64, r: f64) {
        if omega > PI {
            return self.set_coefs(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        }
        let r = r.clamp(0.0, 1.0);
        let q = 1.0 / (0.02 + 30.0 * r * r);
        let (s, c) = omega.sin_cos();
        let al = s / (2.0 * q);
        self.set_coefs(1.0 + al, -2.0 * c, 1.0 - al, 1.0, -2.0 * c, 1.0);
    }

    pub fn lp_bw(&mut self, omega: f64, bw: f64) {
        self.lp(omega, 1.0 / bw);
    }

    pub fn hp_bw(&mut self, omega: f64, bw: f64) {
        self.hp(omega, 1.0 / bw);
    }

    /// Despite its name in the reference, the band-pass recipe.
    pub fn lphp_morph(&mut self, omega: f64, q: f64, _morph: f64) {
        self.bp(omega, q);
    }

    pub fn apf(&mut self, omega: f64, q: f64) {
        if !(0.0..=PI).contains(&omega) {
            return self.set_coefs(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        }
        let (s, c) = omega.sin_cos();
        let al = s / (2.0 * q);
        self.set_coefs(1.0 + al, -2.0 * c, 1.0 - al, 1.0 - al, -2.0 * c, 1.0 + al);
    }

    pub fn peak_eq(&mut self, omega: f64, bw: f64, gain_db: f64) {
        let g = 10f64.powf(gain_db / 20.0);
        let gb = 10f64.powf(gain_db / 40.0);
        self.orfanidis_eq(omega, bw, g, gb, 1.0);
    }

    pub fn orfanidis_eq(&mut self, omega: f64, bw: f64, g: f64, gb: f64, g0: f64) {
        if (g - g0).abs() <= 1e-5 {
            return self.set_coefs(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        }
        let bw = bw.max(1e-4);
        let mut g = g;
        let mut w0 = omega;
        let dww = 2.0 * w0 * ((2f64.ln() / 2.0) * bw).sinh();
        let f = (g * g - gb * gb).abs();
        let mut g00 = (g * g - g0 * g0).abs();
        let mut f00 = (gb * gb - g0 * g0).abs();
        let p2 = PI * PI;
        let num = g0 * g0 * (w0 * w0 - p2).powi(2) + g * g * f00 * p2 * dww * dww / f;
        let den = (w0 * w0 - p2).powi(2) + f00 * p2 * dww * dww / f;
        let g1 = (num / den).sqrt();
        if omega > PI {
            g = 0.9999 * g1;
            w0 = PI - 1e-5;
            g00 = (g * g - g0 * g0).abs();
            f00 = (gb * gb - g0 * g0).abs();
        }
        let g01 = (g * g - g0 * g1).abs();
        let g11 = (g * g - g1 * g1).abs();
        let f01 = (gb * gb - g0 * g1).abs();
        let f11 = (gb * gb - g1 * g1).abs();
        let t2 = (w0 / 2.0).tan().powi(2);
        let w2 = (g11 / g00).sqrt() * t2;
        let w_lo = w0 * (2f32.powf((-bw / 2.0) as f32)) as f64;
        let w_hi = 2.0 * ((f00 / f11).sqrt() * (g11 / g00).sqrt() * t2 / (w_lo / 2.0).tan()).atan();
        let dw = (w_hi - w_lo).abs();
        let d_w = (1.0 + (f00 / f11).sqrt() * w2) * (dw / 2.0).tan();
        let c = f11 * d_w * d_w - 2.0 * w2 * (f01 - (f00 * f11).sqrt());
        let d = 2.0 * w2 * (g01 - (g00 * g11).sqrt());
        let a = ((c + d) / f).sqrt();
        let b = ((g * g * c + gb * gb * d) / f).sqrt();
        self.set_coefs(1.0 + w2 + a, -2.0 * (1.0 - w2), 1.0 + w2 - a, g1 + g0 * w2 + b, -2.0 * (g1 - g0 * w2), g1 - b + g0 * w2);
    }
}

/// omega for a pitch in octaves vs A440 at `fs` (rad/sample).
#[inline]
pub fn calc_omega(octaves: f32, fs: f32) -> f64 {
    2.0 * PI * 440.0 * note_to_pitch(12.0 * octaves) as f64 / fs as f64
}

#[inline]
pub fn calc_omega_hz(hz: f32, fs: f32) -> f64 {
    2.0 * PI * hz as f64 / fs as f64
}

/// Resonance to the Q of the effects' biquads.
#[inline]
pub fn calc_v1_q(r: f32) -> f64 {
    1.0 / (1.02 - r.clamp(0.0, 1.0) as f64)
}

const HB_STEEP: [(&[f64], &[f64]); 6] = [
    (&[0.23647102099689224], &[0.7145421497126001]),
    (&[0.12073211751675449, 0.6632020224193995], &[0.3903621872345006, 0.890786832653497]),
    (&[0.1271414136264853, 0.6528245886369117, 0.9176942834328115], &[0.40056789819445626, 0.8204163891923343, 0.9763114515836773]),
    (
        &[0.07711507983241622, 0.4820706250610472, 0.7968204713315797, 0.9412514277740471],
        &[0.2659685265210946, 0.6651041532634957, 0.8841015085506159, 0.9820054141886075],
    ),
    (
        &[0.051457617441190984, 0.35978656070567017, 0.6725475931034693, 0.8590884928249939, 0.9540209867860787],
        &[0.18621906251989334, 0.529951372847964, 0.7810257527489514, 0.9141815687605308, 0.985475023014907],
    ),
    (
        &[0.036681502163648017, 0.2746317593794541, 0.56109896978791948, 0.769741833862266, 0.8922608180038789, 0.962094548378084],
        &[0.13654762463195771, 0.42313861743656667, 0.6775400499741616, 0.839889624849638, 0.9315419599631839, 0.9878163707328971],
    ),
];
const HB_SOFT: [(&[f64], &[f64]); 6] = [
    (&[0.23647102099689224], &[0.7145421497126001]),
    (&[0.07986642623635751, 0.5453536510711322], &[0.28382934487410993, 0.8344118914807379]),
    (&[0.06029739095712437, 0.4125907203610563, 0.7727156537429234], &[0.21597144456092948, 0.6043586264658363, 0.9238861386532906]),
    (
        &[0.03583278843106211, 0.2720401433964576, 0.5720571972357003, 0.827124761997324],
        &[0.1340901419430669, 0.4243248712718685, 0.7062921421386394, 0.9415030941737551],
    ),
    (
        &[0.02366831419883467, 0.18989476227180174, 0.43157318062118555, 0.6632020224193995, 0.860015542499582],
        &[0.09056555904993387, 0.3078575723749043, 0.5516782402507934, 0.7652146863779808, 0.95247728378667541],
    ),
    (
        &[0.01677466677723562, 0.13902148819717805, 0.3325011117394731, 0.53766105314488, 0.7214184024215805, 0.8821858402078155],
        &[0.06501319274445962, 0.23094129990840923, 0.4364942348420355, 0.6329609551399348, 0.80378086794111226, 0.9599687404800694],
    ),
];

#[derive(Clone, Copy, Debug, Default)]
struct Allpass2 {
    a: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Allpass2 {
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let y = self.x2 + self.a * (x - self.y2);
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// One channel of the polyphase half-band: chains A and B of M allpass
/// sections each, every high-rate sample through both.
#[derive(Clone, Copy, Debug, Default)]
struct HbChannel {
    a: [Allpass2; 6],
    b: [Allpass2; 6],
    m: usize,
    b_prev: f32,
}

impl HbChannel {
    #[inline]
    fn both(&mut self, x: f32) -> (f32, f32) {
        let (mut ya, mut yb) = (x, x);
        for k in 0..self.m {
            ya = self.a[k].tick(ya);
            yb = self.b[k].tick(yb);
        }
        (ya, yb)
    }
}

/// The stereo polyphase IIR half-band filter (2x up/down sampler).
#[derive(Clone, Copy, Debug)]
pub struct HalfBand {
    ch: [HbChannel; 2],
}

impl HalfBand {
    /// `order` = 2M (2..12, even); `steep` the sharper set.
    pub fn new(order: usize, steep: bool) -> HalfBand {
        let m = (order / 2).clamp(1, 6);
        let (ca, cb) = if steep { HB_STEEP[m - 1] } else { HB_SOFT[m - 1] };
        let mut c = HbChannel { m, ..HbChannel::default() };
        for k in 0..m {
            c.a[k].a = ca[k] as f32;
            c.b[k].a = cb[k] as f32;
        }
        HalfBand { ch: [c, c] }
    }

    pub fn reset(&mut self) {
        for c in &mut self.ch {
            let m = c.m;
            let ca: Vec<f32> = c.a.iter().map(|s| s.a).collect();
            let cb: Vec<f32> = c.b.iter().map(|s| s.a).collect();
            *c = HbChannel { m, ..HbChannel::default() };
            for k in 0..6 {
                c.a[k].a = ca[k];
                c.b[k].a = cb[k];
            }
        }
    }

    /// 2N high-rate samples per channel in, N out (in the first N of each
    /// buffer): out[m] = 0.5 (B[2m] + A[2m+1]).
    pub fn decimate(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (ch, buf) in [(0usize, l), (1, r)] {
            let c = &mut self.ch[ch];
            let n = buf.len() / 2;
            for m in 0..n {
                let (_, b0) = c.both(buf[2 * m]);
                let (a1, _) = c.both(buf[2 * m + 1]);
                buf[m] = 0.5 * (b0 + a1);
            }
        }
    }

    /// Decimate mono-in-place helper for one channel buffer.
    pub fn decimate_into(&mut self, inl: &[f32], inr: &[f32], outl: &mut [f32], outr: &mut [f32]) {
        for (ch, src, dst) in [(0usize, inl, outl), (1, inr, outr)] {
            let c = &mut self.ch[ch];
            for m in 0..dst.len().min(src.len() / 2) {
                let (_, b0) = c.both(src[2 * m]);
                let (a1, _) = c.both(src[2 * m + 1]);
                dst[m] = 0.5 * (b0 + a1);
            }
        }
    }

    /// N samples per channel in, 2N out: zero-stuffed stream through both
    /// chains, out[k] = (A[k] + B[k-1]) * scale (0.5 the default, 1 "full
    /// scale").
    pub fn upsample_into(&mut self, inl: &[f32], inr: &[f32], outl: &mut [f32], outr: &mut [f32], scale: f32) {
        for (ch, src, dst) in [(0usize, inl, outl), (1, inr, outr)] {
            let c = &mut self.ch[ch];
            for (i, x) in src.iter().enumerate() {
                if 2 * i + 1 >= dst.len() {
                    break;
                }
                for (k, v) in [(2 * i, *x), (2 * i + 1, 0.0)] {
                    let (a, b) = c.both(v);
                    dst[k] = (a + c.b_prev) * scale;
                    c.b_prev = b;
                }
            }
        }
    }

    /// Same-rate half-band low-pass in place: out[k] = 0.5 (A[k] + B[k-1]).
    pub fn lowpass(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (ch, buf) in [(0usize, l), (1, r)] {
            let c = &mut self.ch[ch];
            for s in buf.iter_mut() {
                let (a, b) = c.both(*s);
                *s = 0.5 * (a + c.b_prev);
                c.b_prev = b;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_and_db_tables() {
        for n in -128..=127 {
            let n = n as f32 + 0.37;
            let want = (n / 12.0).exp2();
            assert!((note_to_pitch(n) / want - 1.0).abs() < 1e-5, "{n}");
        }
        for d in -96..=24 {
            let d = d as f32 + 0.5;
            // Integer-dB entries, linearly interpolated: at most 1.6e-3 off mid-step.
            let want = 10f32.powf(d / 20.0);
            assert!((db_to_linear(d) / want - 1.0).abs() < 2e-3, "{d}");
            assert!((db_to_linear(d.round()) / 10f32.powf(d.round() / 20.0) - 1.0).abs() < 1e-5, "{d}");
        }
        let fs_os = 96000.0;
        let tb = 64.0 / fs_os;
        for v in [-8.0f32, -2.5, 0.0, 3.0] {
            let r = envelope_rate_linear(v, tb) * (v.exp2() * fs_os / 64.0);
            assert!((r - 1.0).abs() < 1e-4, "{v} {r}");
        }
    }

    #[test]
    fn pan_law() {
        for p in [-1.0f32, -0.5, 0.0, 0.5, 1.0] {
            let (l, r) = megapan(p);
            assert!((l - (1.0 - 0.75 * p - 0.25 * p * p)).abs() < 1e-6);
            assert!((r - (1.0 + 0.75 * p - 0.25 * p * p)).abs() < 1e-6);
        }
    }

    #[test]
    fn halfband_passes_low_and_rejects_high() {
        let fs = 96000.0f32;
        for (hz, pass) in [(1000.0f32, true), (40000.0, false)] {
            let mut hb = HalfBand::new(12, true);
            let mut peak = 0.0f32;
            for blk in 0..200 {
                let mut l = [0.0f32; 64];
                let mut r = [0.0f32; 64];
                for i in 0..64 {
                    let ph = ((blk * 64 + i) as f64 * hz as f64 / fs as f64).fract();
                    l[i] = (std::f64::consts::TAU * ph).sin() as f32;
                    r[i] = l[i];
                }
                hb.decimate(&mut l, &mut r);
                if blk > 50 {
                    peak = l[..32].iter().fold(peak, |p, x| p.max(x.abs()));
                }
            }
            if pass {
                assert!((peak - 1.0).abs() < 0.02, "{peak}");
            } else {
                assert!(peak < 1e-4, "{peak}");
            }
        }
    }

    #[test]
    fn biquad_lp_response() {
        let mut b = Biquad::new();
        let w = calc_omega_hz(1000.0, 48000.0);
        b.lp(w, 0.707);
        assert!((b.response(w) - 0.707).abs() < 0.01);
        assert!((b.response(1e-4) - 1.0).abs() < 1e-3);
    }
}
