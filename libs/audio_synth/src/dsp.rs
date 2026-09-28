//! Small realtime building blocks: band-limited oscillators, envelopes,
//! filters, noise colours, smoothing and saturation.
//!
//! Everything here is `Copy`-sized state with no heap, so a voice built from
//! these can live in a fixed array owned by the audio callback. Nothing
//! allocates, locks or panics while rendering.

use crate::svf::{Svf, SvfCoeffs};

pub const TAU: f32 = core::f32::consts::TAU;
pub const PI: f32 = core::f32::consts::PI;

/// `value` when finite, `fallback` otherwise: parameters arrive from scripts
/// and a NaN must never reach a filter state.
#[inline]
pub fn finite(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

#[inline]
pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db * 0.05)
}

#[inline]
pub fn gain_to_db(gain: f32) -> f32 {
    20.0 * gain.max(1.0e-9).log10()
}

/// Rational tanh approximation: smooth, odd, saturates at ±1, cheap.
#[inline]
pub fn soft_clip(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
}

/// Asymmetric saturation (even harmonics) for exhaust and tube colour.
#[inline]
pub fn asym_clip(x: f32, bias: f32) -> f32 {
    soft_clip(x + bias) - soft_clip(bias)
}

/// Linear interpolation.
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// MIDI note to Hz (A4 = 69 = 440 Hz).
#[inline]
pub fn midi_to_hz(note: f32) -> f32 {
    440.0 * 2f32.powf((note - 69.0) / 12.0)
}

/// Note names such as `C4`, `F#5`, `Bb3`. Returns a MIDI note number.
pub fn note_number(token: &str) -> Option<u8> {
    let bytes = token.as_bytes();
    let semitone: i32 = match bytes.first()?.to_ascii_uppercase() {
        b'C' => 0,
        b'D' => 2,
        b'E' => 4,
        b'F' => 5,
        b'G' => 7,
        b'A' => 9,
        b'B' => 11,
        _ => return None,
    };
    let mut index = 1;
    let mut accidental = 0;
    match bytes.get(index) {
        Some(b'#') => {
            accidental = 1;
            index += 1;
        }
        Some(b'b') if bytes.len() > 2 => {
            accidental = -1;
            index += 1;
        }
        _ => {}
    }
    let octave: i32 = token.get(index..)?.parse().ok()?;
    let midi = (octave + 1) * 12 + semitone + accidental;
    (0..=127).contains(&midi).then_some(midi as u8)
}

/// xorshift32: a device-local noise source, deterministic per seed.
#[derive(Clone, Copy, Debug)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Rng(seed | 1)
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// Uniform in [-1, 1).
    #[inline]
    pub fn bipolar(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (2.0 / 16_777_216.0) - 1.0
    }

    /// Uniform in [0, 1).
    #[inline]
    pub fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (1.0 / 16_777_216.0)
    }
}

/// Noise colours. Pink is Paul Kellet's economy filter (−3 dB/oct within
/// ±0.5 dB over the audio band); brown is leaky-integrated white; blue is
/// differentiated white; velvet is sparse ±1 impulses (crackle, gravel).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NoiseColor {
    #[default]
    White,
    Pink,
    Brown,
    Blue,
    Velvet,
}

impl NoiseColor {
    pub fn parse(s: &str) -> Option<NoiseColor> {
        Some(match s {
            "white" | "noise" => NoiseColor::White,
            "pink" => NoiseColor::Pink,
            "brown" | "red" => NoiseColor::Brown,
            "blue" => NoiseColor::Blue,
            "velvet" | "crackle" => NoiseColor::Velvet,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Noise {
    pub color: NoiseColor,
    rng: Rng,
    b: [f32; 3],
    last: f32,
    /// Velvet density: impulses per sample (0..1).
    pub density: f32,
}

impl Noise {
    pub fn new(color: NoiseColor, seed: u32) -> Self {
        Noise { color, rng: Rng::new(seed), b: [0.0; 3], last: 0.0, density: 0.02 }
    }

    #[inline]
    pub fn next(&mut self) -> f32 {
        let w = self.rng.bipolar();
        match self.color {
            NoiseColor::White => w,
            NoiseColor::Pink => {
                self.b[0] = 0.99765 * self.b[0] + w * 0.0990460;
                self.b[1] = 0.96300 * self.b[1] + w * 0.2965164;
                self.b[2] = 0.57000 * self.b[2] + w * 1.0526913;
                (self.b[0] + self.b[1] + self.b[2] + w * 0.1848) * 0.22
            }
            NoiseColor::Brown => {
                self.last = (self.last + w * 0.04) * 0.998;
                self.last * 3.0
            }
            NoiseColor::Blue => {
                let out = w - self.last;
                self.last = w;
                out * 0.5
            }
            NoiseColor::Velvet => {
                if self.rng.unit() < self.density {
                    if w >= 0.0 { 1.0 } else { -1.0 }
                } else {
                    0.0
                }
            }
        }
    }
}

/// `sin(2π·t)` for a phase `t` in 0..1 by an odd Taylor polynomial on
/// [-π, π] (error < 1e-4): the oscillators' sine without libm's cost.
#[inline]
pub fn sin_turns(t: f32) -> f32 {
    let x = TAU * (t - t.floor() - 0.5);
    let x2 = x * x;
    // sin(x) Taylor to x^13, Horner form; sin(2πt) = -sin(x).
    let p = 1.0
        - x2 / 6.0
            * (1.0 - x2 / 20.0 * (1.0 - x2 / 42.0 * (1.0 - x2 / 72.0 * (1.0 - x2 / 110.0 * (1.0 - x2 / 156.0)))));
    -x * p
}

/// PolyBLEP residual for a discontinuity at phase 0 with increment `dt`.
#[inline]
pub fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Wave {
    #[default]
    Sine,
    Triangle,
    Saw,
    Square,
    /// Variable-width pulse (`Osc::width`).
    Pulse,
    /// Band-limited saw with a softened top: warmer than a raw saw.
    SoftSaw,
}

impl Wave {
    pub fn parse(s: &str) -> Option<Wave> {
        Some(match s {
            "sine" | "sin" => Wave::Sine,
            "triangle" | "tri" => Wave::Triangle,
            "saw" | "sawtooth" => Wave::Saw,
            "square" | "sq" => Wave::Square,
            "pulse" => Wave::Pulse,
            "softsaw" | "warm" => Wave::SoftSaw,
            _ => return None,
        })
    }
}

/// Band-limited oscillator (PolyBLEP saw/square/pulse, integrated triangle).
/// Phase is 0..1; `step` takes the frequency in Hz every sample so glides and
/// FM are free.
#[derive(Clone, Copy, Debug, Default)]
pub struct Osc {
    pub wave: Wave,
    pub phase: f32,
    /// Pulse width for `Wave::Pulse`, 0.05..0.95.
    pub width: f32,
    tri: f32,
}

impl Osc {
    pub fn new(wave: Wave, phase: f32) -> Self {
        Osc { wave, phase: phase.fract().abs(), width: 0.5, tri: 0.0 }
    }

    /// One sample at `hz`, sample period `inv_rate`. `pm` is phase
    /// modulation in cycles (FM operators feed this).
    #[inline]
    pub fn next(&mut self, hz: f32, inv_rate: f32, pm: f32) -> f32 {
        let dt = (hz * inv_rate).clamp(0.0, 0.5);
        let mut t = self.phase + pm;
        t -= t.floor();
        let out = match self.wave {
            Wave::Sine => sin_turns(t),
            Wave::Saw | Wave::SoftSaw => {
                let v = 2.0 * t - 1.0 - poly_blep(t, dt);
                if self.wave == Wave::SoftSaw {
                    soft_clip(v * 1.4) * 0.85
                } else {
                    v
                }
            }
            Wave::Square | Wave::Pulse | Wave::Triangle => {
                let w = if self.wave == Wave::Pulse { self.width.clamp(0.05, 0.95) } else { 0.5 };
                let mut v = if t < w { 1.0 } else { -1.0 };
                v += poly_blep(t, dt);
                let mut t2 = t + 1.0 - w;
                t2 -= t2.floor();
                v -= poly_blep(t2, dt);
                if self.wave == Wave::Triangle {
                    // Leaky integration of the band-limited square.
                    self.tri = dt * 4.0 * v + (1.0 - dt * 0.5) * self.tri;
                    self.tri
                } else {
                    v
                }
            }
        };
        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        out
    }
}

/// ADSR with exponential segments (analog-style curvature). Times in
/// seconds, sustain 0..1. `gate(false)` releases from wherever it is.
#[derive(Clone, Copy, Debug, Default)]
pub struct Adsr {
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    stage: u8,
    level: f32,
    /// Per-sample coefficients for the sample period they were built at
    /// (the exponentials stay out of the per-sample path).
    coef_inv: f32,
    coef_a: f32,
    coef_d: f32,
    coef_r: f32,
}

const ADSR_IDLE: u8 = 0;
const ADSR_ATTACK: u8 = 1;
const ADSR_DECAY: u8 = 2;
const ADSR_SUSTAIN: u8 = 3;
const ADSR_RELEASE: u8 = 4;

impl Adsr {
    pub fn new(attack: f32, decay: f32, sustain: f32, release: f32) -> Self {
        Adsr {
            attack: finite(attack, 0.002).max(0.0),
            decay: finite(decay, 0.1).max(0.0),
            sustain: finite(sustain, 0.0).clamp(0.0, 1.0),
            release: finite(release, 0.1).max(0.0),
            stage: ADSR_IDLE,
            level: 0.0,
            coef_inv: -1.0,
            coef_a: 1.0,
            coef_d: 1.0,
            coef_r: 1.0,
        }
    }

    pub fn gate(&mut self, on: bool) {
        if on {
            self.stage = ADSR_ATTACK;
        } else if self.stage != ADSR_IDLE {
            self.stage = ADSR_RELEASE;
        }
    }

    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn active(&self) -> bool {
        self.stage != ADSR_IDLE
    }

    pub fn releasing(&self) -> bool {
        self.stage == ADSR_RELEASE
    }

    /// Per-sample coefficient reaching ~99.3 % of a target in `secs`.
    #[inline]
    fn coef(secs: f32, inv_rate: f32) -> f32 {
        if secs <= inv_rate {
            1.0
        } else {
            1.0 - (-5.0 * inv_rate / secs).exp()
        }
    }

    #[inline]
    pub fn next(&mut self, inv_rate: f32) -> f32 {
        if inv_rate != self.coef_inv {
            self.coef_inv = inv_rate;
            self.coef_a = Self::coef(self.attack, inv_rate) * 0.9;
            self.coef_d = Self::coef(self.decay, inv_rate);
            self.coef_r = Self::coef(self.release, inv_rate);
        }
        match self.stage {
            ADSR_ATTACK => {
                // Attack aims past 1 so the curve is concave and lands on
                // time, like an RC charging towards a higher rail.
                if self.attack <= inv_rate {
                    self.level = 1.0;
                } else {
                    self.level += (1.3 - self.level) * self.coef_a;
                }
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = ADSR_DECAY;
                }
            }
            ADSR_DECAY => {
                self.level += (self.sustain - self.level) * self.coef_d;
                if (self.level - self.sustain).abs() < 1.0e-4 {
                    self.level = self.sustain;
                    self.stage = ADSR_SUSTAIN;
                }
            }
            ADSR_SUSTAIN => {
                self.level = self.sustain;
                if self.sustain <= 1.0e-5 {
                    self.stage = ADSR_IDLE;
                }
            }
            ADSR_RELEASE => {
                self.level += (0.0 - self.level) * self.coef_r;
                if self.level < 1.0e-5 {
                    self.level = 0.0;
                    self.stage = ADSR_IDLE;
                }
            }
            _ => self.level = 0.0,
        }
        self.level
    }
}

/// One-pole smoother / low-pass with the coefficient from a cutoff.
#[derive(Clone, Copy, Debug, Default)]
pub struct OnePole {
    pub z: f32,
    a: f32,
}

impl OnePole {
    pub fn new(cutoff_hz: f32, rate: f32) -> Self {
        let mut f = OnePole { z: 0.0, a: 1.0 };
        f.set(cutoff_hz, rate);
        f
    }

    #[inline]
    pub fn set(&mut self, cutoff_hz: f32, rate: f32) {
        let x = (-TAU * finite(cutoff_hz, 1000.0).clamp(1.0, rate * 0.49) / rate).exp();
        self.a = 1.0 - x;
    }

    #[inline]
    pub fn lp(&mut self, x: f32) -> f32 {
        self.z += (x - self.z) * self.a;
        self.z
    }

    #[inline]
    pub fn hp(&mut self, x: f32) -> f32 {
        x - self.lp(x)
    }
}

/// DC blocker (a leaky differentiator at ~10 Hz).
#[derive(Clone, Copy, Debug, Default)]
pub struct DcBlock {
    x1: f32,
    y1: f32,
}

impl DcBlock {
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + 0.9987 * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

/// Parameter smoother: exponential glide to a target, `rate_hz` ≈ 1/time.
#[derive(Clone, Copy, Debug, Default)]
pub struct Smooth {
    pub value: f32,
    pub target: f32,
}

impl Smooth {
    pub fn new(v: f32) -> Self {
        Smooth { value: v, target: v }
    }

    #[inline]
    pub fn next(&mut self, coef: f32) -> f32 {
        self.value += (self.target - self.value) * coef;
        self.value
    }

    /// Per-sample coefficient for a time constant in seconds.
    pub fn coef(secs: f32, rate: f32) -> f32 {
        1.0 - (-1.0 / (secs.max(1.0e-4) * rate)).exp()
    }
}

/// Filter modes for [`Filter`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FilterMode {
    #[default]
    Off,
    Lowpass,
    Highpass,
    Bandpass,
    Notch,
    /// 4-pole transistor-ladder low-pass with tanh stages (resonant, fat).
    Ladder,
}

impl FilterMode {
    pub fn parse(s: &str) -> Option<FilterMode> {
        Some(match s {
            "off" | "none" => FilterMode::Off,
            "lp" | "lowpass" | "low" => FilterMode::Lowpass,
            "hp" | "highpass" | "high" => FilterMode::Highpass,
            "bp" | "bandpass" | "band" => FilterMode::Bandpass,
            "notch" => FilterMode::Notch,
            "ladder" | "moog" => FilterMode::Ladder,
            _ => return None,
        })
    }
}

/// Fast `tan(pi * x)` for x in 0..0.49 (Padé, within 0.2 % below 0.4).
#[inline]
fn tan_pi(x: f32) -> f32 {
    let x = x.clamp(0.0, 0.49);
    if x > 0.4 {
        return (PI * x).tan();
    }
    let a = PI * x;
    let a2 = a * a;
    a * (15.0 - a2) / (15.0 - 6.0 * a2)
}

/// 4-pole ladder (Huovilainen-style, tanh per stage, zero-delay-free but
/// stable to self-oscillation at resonance 1).
#[derive(Clone, Copy, Debug, Default)]
pub struct Ladder {
    s: [f32; 4],
    g: f32,
    res: f32,
}

impl Ladder {
    #[inline]
    pub fn set(&mut self, cutoff: f32, resonance: f32, rate: f32) {
        let fc = (finite(cutoff, 1000.0) / rate).clamp(1.0e-4, 0.45);
        let g = tan_pi(fc);
        self.g = g / (1.0 + g);
        self.res = finite(resonance, 0.0).clamp(0.0, 1.05) * 4.0;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let fb = self.s[3];
        let mut u = soft_clip(x - self.res * fb);
        for i in 0..4 {
            let v = (u - self.s[i]) * self.g;
            let y = v + self.s[i];
            self.s[i] = y + v;
            u = y;
        }
        // Resonance steals low end; give some of it back.
        u * (1.0 + self.res * 0.25)
    }
}

/// A multimode filter: SVF for the classic responses, ladder for the fat
/// one. Coefficients are recomputed only when cutoff/Q move noticeably.
#[derive(Clone, Copy)]
pub struct Filter {
    pub mode: FilterMode,
    svf: Svf,
    svf2: Svf,
    coeffs: SvfCoeffs,
    ladder: Ladder,
    last_cutoff: f32,
    last_q: f32,
    /// Two cascaded SVF stages (24 dB/oct) for LP/HP.
    pub steep: bool,
}

impl Default for Filter {
    fn default() -> Self {
        Filter::new(FilterMode::Off)
    }
}

impl Filter {
    pub fn new(mode: FilterMode) -> Self {
        Filter {
            mode,
            svf: Svf::default(),
            svf2: Svf::default(),
            coeffs: SvfCoeffs::new(1000.0, 0.707, 48_000.0),
            ladder: Ladder::default(),
            last_cutoff: -1.0,
            last_q: -1.0,
            steep: false,
        }
    }

    /// Set cutoff (Hz) and Q (0.5..20; for the ladder, resonance = (q-0.5)/10).
    #[inline]
    pub fn set(&mut self, cutoff: f32, q: f32, rate: f32) {
        let cutoff = finite(cutoff, 1000.0).clamp(10.0, rate * 0.45);
        if (cutoff - self.last_cutoff).abs() < self.last_cutoff * 0.002 && q == self.last_q {
            return;
        }
        self.last_cutoff = cutoff;
        self.last_q = q;
        if self.mode == FilterMode::Ladder {
            self.ladder.set(cutoff, ((q - 0.5) / 10.0).clamp(0.0, 1.0), rate);
        } else {
            self.coeffs = SvfCoeffs::new(cutoff, q, rate);
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        match self.mode {
            FilterMode::Off => x,
            FilterMode::Lowpass => {
                let y = self.svf.process(&self.coeffs, x).0;
                if self.steep { self.svf2.process(&self.coeffs, y).0 } else { y }
            }
            FilterMode::Highpass => {
                let y = self.svf.process(&self.coeffs, x).2;
                if self.steep { self.svf2.process(&self.coeffs, y).2 } else { y }
            }
            FilterMode::Bandpass => self.svf.process(&self.coeffs, x).1,
            FilterMode::Notch => {
                let (l, _, h) = self.svf.process(&self.coeffs, x);
                l + h
            }
            FilterMode::Ladder => self.ladder.process(x),
        }
    }

    pub fn reset(&mut self) {
        self.svf.reset();
        self.svf2.reset();
        self.ladder = Ladder::default();
    }
}

/// A resonant band (formant / body mode): an SVF band-pass with a gain.
#[derive(Clone, Copy)]
pub struct Resonator {
    svf: Svf,
    coeffs: SvfCoeffs,
    pub gain: f32,
}

impl Resonator {
    pub fn new(hz: f32, q: f32, gain: f32, rate: f32) -> Self {
        Resonator { svf: Svf::default(), coeffs: SvfCoeffs::new(hz, q, rate), gain }
    }

    pub fn tune(&mut self, hz: f32, q: f32, rate: f32) {
        self.coeffs = SvfCoeffs::new(hz, q, rate);
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.svf.process(&self.coeffs, x).1 * self.gain
    }
}

/// Raised-cosine pulse shape, `u` in 0..1 across the pulse: smooth, so a
/// firing pulse is band-limited by construction once its width spans more
/// than a few samples.
#[inline]
pub fn hann_pulse(u: f32) -> f32 {
    if (0.0..1.0).contains(&u) {
        0.5 - 0.5 * (u * TAU).cos()
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polyblep_saw_has_little_energy_above_nyquist_foldback() {
        // A 4.7 kHz saw at 48 kHz: a naive saw aliases heavily; the BLEP
        // saw's folded 11th harmonic must sit far below the fundamental.
        let rate = 48_000.0;
        let mut osc = Osc::new(Wave::Saw, 0.0);
        let n = 4800;
        let buf: Vec<f32> = (0..n).map(|_| osc.next(4_700.0, 1.0 / rate, 0.0)).collect();
        let bin = |hz: f32| {
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, x) in buf.iter().enumerate() {
                let a = TAU * hz * i as f32 / rate;
                re += x * a.cos();
                im += x * a.sin();
            }
            (re * re + im * im).sqrt()
        };
        let fundamental = bin(4_700.0);
        // 11th harmonic 51.7 kHz folds to 3.7 kHz.
        let alias = bin(3_700.0);
        assert!(alias < fundamental * 0.05, "alias {alias} vs {fundamental}");
    }

    #[test]
    fn adsr_reaches_sustain_and_releases_to_idle() {
        let mut env = Adsr::new(0.01, 0.05, 0.5, 0.05);
        env.gate(true);
        let inv = 1.0 / 48_000.0;
        for _ in 0..48_00 {
            env.next(inv);
        }
        assert!((env.level() - 0.5).abs() < 0.01);
        env.gate(false);
        for _ in 0..48_000 {
            env.next(inv);
        }
        assert!(!env.active());
    }

    #[test]
    fn the_fast_sine_matches_libm() {
        for i in 0..1000 {
            let t = i as f32 / 1000.0;
            assert!((sin_turns(t) - (t * TAU).sin()).abs() < 2.0e-4, "{t}");
        }
    }

    #[test]
    fn note_names_parse() {
        assert_eq!(note_number("A4"), Some(69));
        assert_eq!(note_number("C4"), Some(60));
        assert_eq!(note_number("F#5"), Some(78));
        assert_eq!(note_number("Bb3"), Some(58));
        assert_eq!(note_number("H2"), None);
    }

    #[test]
    fn ladder_is_stable_at_full_resonance() {
        let mut f = Filter::new(FilterMode::Ladder);
        f.set(2_000.0, 20.0, 48_000.0);
        let mut n = Noise::new(NoiseColor::White, 3);
        let mut peak = 0.0f32;
        for _ in 0..48_000 {
            let y = f.process(n.next());
            assert!(y.is_finite());
            peak = peak.max(y.abs());
        }
        assert!(peak < 10.0, "{peak}");
    }
}
