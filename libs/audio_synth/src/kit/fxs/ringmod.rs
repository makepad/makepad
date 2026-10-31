//! Ring modulator (type 13): a diode ring model at 2x the host rate, the
//! carrier being a unison stack of the sine oscillator's 32 wave shapes or
//! the host's audio input; optional high and low cuts after decimation and
//! the wet/dry mix.

use super::common::*;
use super::FxCtx;
use crate::kit::core::{calc_omega, note_to_pitch, Biquad, HalfBand, SlotDef, SlotUnit, SlotValue};

/// In "absolute" detune mode the reference leaves out the MIDI-note-0
/// base frequency (8.1758 Hz) from the carrier increment; mirrored.
#[allow(dead_code)]
const QUIRK_ABSOLUTE_DROPS_BASE_FREQ: bool = true;
/// Each input channel's diode output goes to BOTH output accumulators
/// through the voice pans (the left and right inputs are summed); mirrored.
#[allow(dead_code)]
const QUIRK_CHANNELS_SUM_THROUGH_PANS: bool = true;

const MIDI0_HZ: f32 = 8.1758;
const AUDIO_IN: i32 = 32;

const SHAPES: [&str; 33] = [
    "Wave 1", "Wave 2", "Wave 3", "Wave 4", "Wave 5", "Wave 6", "Wave 7", "Wave 8", "Wave 9", "Wave 10", "Wave 11", "Wave 12", "Wave 13", "Wave 14", "Wave 15",
    "Wave 16", "Wave 17", "Wave 18", "Wave 19", "Wave 20", "Wave 21", "Wave 22", "Wave 23", "Wave 24", "Wave 25", "Wave 26", "Wave 27", "Wave 28",
    "Wave 29", "Wave 30", "Wave 31", "Wave 32", "Audio In",
];

pub fn slot_defs() -> [SlotDef; 12] {
    [
        choice("Shape", &SHAPES, 0.0),
        fl("Frequency", 0.0, 127.0, 60.0, SlotUnit::MidiNote),
        about(absolute(extend(fl("Unison Detune", 0.0, 1.0, 0.2, SlotUnit::None))), "x100 cents; extended x12 semitones"),
        int("Unison Voices", 1.0, 16.0, 1.0),
        percent("Forward Bias", 0.3),
        percent("Linear Region", 0.7),
        freq_d("Low Cut", -60.0),
        freq_d("High Cut", 70.0),
        percent("Mix", 1.0),
        unused(),
        unused(),
        unused(),
    ]
}

pub fn init_defaults(_p: &mut [SlotValue; 12]) {}

pub fn migrate(rev: i32, p: &mut [SlotValue; 12]) {
    if rev <= 15 {
        p[6].deactivated = false;
        p[7].deactivated = false;
    }
    // SPEC? PATCH-FORMAT lists this rule in the global table; applied here
    // per effect (SYNTHPORT: do not apply it twice). 28 -> 33 is literal
    // (out of range; any value >= 32 is the audio input carrier).
    if rev <= 27 {
        let v = p[0].i();
        if v == 28 {
            p[0].v = 33.0;
        } else if matches!(v, 1 | 3 | 5 | 7) {
            p[0].v = (28 + (v - 1) / 2) as f32;
        }
    }
}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    -1
}

/// The sine oscillator's shape functions (s = sin x, c = cos x).
fn sine_shape(mode: i32, s: f32, c: f32) -> f32 {
    let s2 = 2.0 * s * c;
    let c2 = 1.0 - 2.0 * s * s;
    let s4 = 2.0 * s2 * c2;
    let h = s >= 0.0;
    let sg = if h { 1.0 } else { -1.0 };
    let q = match (s <= 0.0, c <= 0.0) {
        (false, false) => 1.0,
        (false, true) => 2.0,
        (true, true) => 3.0,
        (true, false) => 4.0,
    };
    match mode {
        0 => s,
        1 => {
            if h {
                (1.0 - c2) / 2.0
            } else {
                (c2 - 1.0) / 2.0
            }
        }
        2 => if h { s } else { 0.0 },
        3 => if h { (1.0 - c2) / 2.0 } else { 0.0 },
        4 => if h { s2 } else { 0.0 },
        5 => if h { sine_shape(1, s2, c2) } else { 0.0 },
        6 => if h { s2.abs() } else { 0.0 },
        7 => sine_shape(5, s, c).abs(),
        8 => 2.0 * (if h { s } else { 0.0 }) - 1.0,
        9 => if s * c <= 0.0 { s } else { 0.0 },
        10 => if s * c >= 0.0 { s } else { 0.0 },
        11 => 2.0 * sine_shape(3, s, c) - 1.0,
        12 => s2 * if c >= 0.0 { 1.0 } else { -1.0 },
        13 => {
            if s2 >= 0.0 {
                s2 * if s <= 0.0 { -1.0 } else { 1.0 }
            } else {
                0.0
            }
        }
        14 => if h { c2.abs() } else { 0.0 },
        15 => {
            if c >= 0.0 {
                if h {
                    1.0 - s
                } else {
                    -1.0 - s
                }
            } else {
                0.0
            }
        }
        16 => {
            if c >= 0.0 {
                if h {
                    1.0 - s
                } else {
                    c - 1.0
                }
            } else {
                0.0
            }
        }
        17 => sg - s,
        18 => if c <= 0.0 { c } else { sg * s2 },
        19 => {
            if h {
                if c >= 0.0 {
                    s2
                } else {
                    -s4
                }
            } else {
                s
            }
        }
        20 => if s * c >= 0.0 { s } else { sg },
        21 => if s * c >= 0.0 { sg } else { s },
        22 => if c >= 0.0 { s } else { 0.0 },
        23 => if c <= 0.0 { s } else { 0.0 },
        24 => if h { 1.0 - s } else { s },
        25 => if h { s2 / q } else { 0.0 },
        26 => if s <= 0.0 && c <= 0.0 { 0.0 } else { s },
        27 => s2 / q,
        28 => sg + (if s * c < 0.0 { 1.0 } else { -1.0 }) * c,
        29 => {
            if h {
                1.0 + c * if c <= 0.0 { 1.0 } else { -1.0 }
            } else {
                0.0
            }
        }
        30 => if h { sine_shape(28, s2, c2) } else { 0.0 },
        31 => sine_shape(30, s, c).abs(),
        _ => s,
    }
}

#[inline]
fn diode(v: f32, vb: f32, vl: f32) -> f32 {
    if v < vb {
        0.0
    } else if v < vl {
        (v - vb) * (v - vb) / (2.0 * (vl - vb))
    } else {
        v - vl + (vl - vb) * (vl - vb) / (2.0 * (vl - vb))
    }
}

pub struct RingMod {
    sr: f32,
    up: HalfBand,
    up_in: HalfBand,
    down: HalfBand,
    voices: usize,
    phase: [f32; 16],
    offset: [f32; 16],
    pan_l: [f32; 16],
    pan_r: [f32; 16],
    gain: f32,
    lp: Biquad,
    hp: Biquad,
    mix: Line,
}

impl RingMod {
    pub fn new(sr: f32) -> RingMod {
        RingMod {
            sr,
            up: HalfBand::new(12, true),
            up_in: HalfBand::new(12, true),
            down: HalfBand::new(12, true),
            voices: 0,
            phase: [0.0; 16],
            offset: [0.0; 16],
            pan_l: [1.0; 16],
            pan_r: [1.0; 16],
            gain: 1.0,
            lp: Biquad::new(),
            hp: Biquad::new(),
            mix: Line::new(),
        }
    }

    pub fn reset(&mut self) {
        *self = RingMod::new(self.sr);
    }

    fn set_voices(&mut self, n: usize) {
        self.voices = n;
        if n == 1 {
            self.phase[0] = 0.0;
            self.offset[0] = 0.0;
            self.pan_l[0] = 1.0;
            self.pan_r[0] = 1.0;
        } else {
            let m = (n - 1) as f32;
            for k in 0..n {
                self.phase[k] = k as f32 / n as f32;
                self.offset[k] = -1.0 + 2.0 * k as f32 / m;
                self.pan_l[k] = k as f32 / m;
                self.pan_r[k] = (m - k as f32) / m;
            }
        }
        self.gain = 0.4 + 0.6 / (n as f32).sqrt();
    }

    pub fn process(&mut self, p: &[SlotValue; 12], ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        let sr = self.sr;
        let shape = p[0].i();
        let audio = shape >= AUDIO_IN;
        let n = if audio { 1 } else { p[3].i().clamp(1, 16) as usize };
        if n != self.voices {
            self.set_voices(n);
        }
        let mut inc = [0f32; 16];
        let base = if p[2].absolute { 1.0 } else { MIDI0_HZ };
        for k in 0..n {
            let d = ext(&p[2], |v| 12.0 * v) * self.offset[k];
            inc[k] = note_to_pitch(p[1].v + d) * base / (2.0 * sr);
        }

        let mut xl = [0f32; 64];
        let mut xr = [0f32; 64];
        self.up.upsample_into(&l[..], &r[..], &mut xl, &mut xr, 0.5);
        // The interpolator's 1/2 gain is restored by the 2x below for the
        // carrier; the program input keeps the reference's staging.
        let mut cl = [0f32; 64];
        let mut cr = [0f32; 64];
        if audio {
            if let Some((al, ar)) = ctx.audio_in {
                self.up_in.upsample_into(&al[..], &ar[..], &mut cl, &mut cr, 0.5);
            }
        }
        let vb = p[4].v;
        let vl = p[5].v.max(vb + 0.02);
        let tau = std::f32::consts::TAU;
        for i in 0..64 {
            let mut rl = 0.0;
            let mut rr = 0.0;
            for k in 0..n {
                let (cls, crs) = if audio {
                    (2.0 * cl[i], 2.0 * cr[i])
                } else {
                    let a = tau * (self.phase[k] - 0.5);
                    let c = sine_shape(shape, a.sin(), a.cos());
                    self.phase[k] += inc[k];
                    self.phase[k] -= self.phase[k].floor();
                    (c, c)
                };
                for (x, c) in [(xl[i], cls), (xr[i], crs)] {
                    let a = 0.5 * x + c;
                    let b = c - 0.5 * x;
                    let v = diode(a, vb, vl) + diode(-a, vb, vl) - diode(b, vb, vl) - diode(-b, vb, vl);
                    rl += v * self.pan_l[k];
                    rr += v * self.pan_r[k];
                }
            }
            xl[i] = cubic_sat(self.gain * rl);
            xr[i] = cubic_sat(self.gain * rr);
        }
        self.down.decimate(&mut xl, &mut xr);
        let mut wl = [0f32; B];
        let mut wr = [0f32; B];
        wl.copy_from_slice(&xl[..B]);
        wr.copy_from_slice(&xr[..B]);
        if !p[7].deactivated {
            self.lp.lp2b(calc_omega(oct(p[7].v), sr), 0.707);
            self.lp.process(&mut wl, &mut wr);
        }
        if !p[6].deactivated {
            self.hp.hp(calc_omega(oct(p[6].v), sr), 0.707);
            self.hp.process(&mut wl, &mut wr);
        }
        self.mix.set_target_smoothed(clamp01(p[8].v));
        self.mix.fade_2_blocks(l, r, &wl, &wr);
    }

    pub fn process_only_control(&mut self, _p: &[SlotValue; 12], _ctx: &FxCtx) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bin(x: &[f32], hz: f64, sr: f64) -> f64 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (n, v) in x.iter().enumerate() {
            let a = std::f64::consts::TAU * hz * n as f64 / sr;
            re += *v as f64 * a.cos();
            im += *v as f64 * a.sin();
        }
        (re * re + im * im).sqrt() / x.len() as f64
    }

    #[test]
    fn linear_region_gives_sum_and_difference() {
        let mut p = super::super::default_values(13);
        p[4].v = 0.0;
        p[5].v = 1.0;
        p[1].v = 69.0 - 12.0; // 220 Hz carrier
        let ctx = super::super::FxCtx::new(48000.0);
        let mut fx = RingMod::new(48000.0);
        let sr = 48000.0f64;
        let mut out = Vec::new();
        for b in 0..300 {
            let mut l = [0f32; 32];
            let mut r = [0f32; 32];
            for k in 0..32 {
                let t = (b * 32 + k) as f64 / sr;
                l[k] = 0.05 * (std::f64::consts::TAU * 1000.0 * t).sin() as f32;
                r[k] = l[k];
            }
            fx.process(&p, &ctx, &mut l, &mut r);
            assert!(l.iter().all(|x| x.is_finite()));
            if b >= 50 {
                out.extend_from_slice(&l);
            }
        }
        let carrier = 8.1758f64 * 2f64.powf(57.0 / 12.0);
        let sum = bin(&out, 1000.0 + carrier, sr);
        let dif = bin(&out, 1000.0 - carrier, sr);
        let dry = bin(&out, 1000.0, sr);
        let car = bin(&out, carrier, sr);
        assert!(sum > 10.0 * dry && dif > 10.0 * dry, "{sum} {dif} {dry}");
        assert!(sum > 10.0 * car, "{sum} {car}");
    }

    #[test]
    fn shapes_and_unison_finite() {
        for shape in 0..32 {
            let mut p = super::super::default_values(13);
            p[0].v = shape as f32;
            p[3].v = 5.0;
            let ctx = super::super::FxCtx::new(48000.0);
            let mut fx = RingMod::new(48000.0);
            let mut rng = crate::dsp::Rng::new(5);
            for _ in 0..20 {
                let mut l = [0f32; 32];
                let mut r = [0f32; 32];
                for k in 0..32 {
                    l[k] = rng.bipolar();
                    r[k] = rng.bipolar();
                }
                fx.process(&p, &ctx, &mut l, &mut r);
                assert!(l.iter().chain(r.iter()).all(|x| x.is_finite()), "{shape}");
            }
        }
    }
}
