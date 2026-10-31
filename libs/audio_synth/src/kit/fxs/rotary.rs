//! Rotary speaker (type 4): mono input, optional drive through one of the
//! eight shaper models, an 800 Hz crossover into a horn (rotating around
//! the origin, heard by two listeners: doppler delay plus a facing-gain
//! tremolo) and a low rotor (a 200 Hz split whose upper part is amplitude
//! modulated by the rotor phase). Width and mix on the result.

use super::common::*;
use crate::kit::shaper::{Shaper, FX_SHAPER_SUBLIST};
use super::FxCtx;
use crate::kit::core::{calc_omega, db_to_linear, Biquad, Lag, SlotDef, SlotUnit, SlotValue};
use std::f64::consts::TAU;

/// The reference multiplies the log2 horn rate by the tempo ratio before
/// exponentiating (f = 2^(v * tsr)) instead of scaling the Hz; mirrored.
#[allow(dead_code)]
const QUIRK_TEMPO_SCALES_EXPONENT: bool = true;
/// The reference's Hard model falls through to the Asymmetric model's gain
/// constants; mirrored in `MODEL_GAINS`.
#[allow(dead_code)]
const QUIRK_HARD_USES_ASYM_GAINS: bool = true;

/// Per model: (tweak, comp, start, squared).
const MODEL_GAINS: [(f32, f32, f32, bool); 8] = [
    (1.0, 4.0, 0.18, false),
    (1.15, 9.0, 0.05, false),
    (1.15, 9.0, 0.05, false),
    (4.4, 10.0, 0.0, true),
    (1.0, 4.0, 0.0, false),
    (1.0, 4.0, 0.18, false),
    (1.0, 2.0, 0.0, false),
    (1.0, 2.0, 0.0, false),
];

const RINGOUT: i32 = 1000;

pub fn slot_defs() -> [SlotDef; 12] {
    [
        lforate("Horn Rate", 0.0),
        percent("Doppler", 0.0),
        percent("Tremolo", 0.0),
        // SPEC? the table gives default 0 with a UI default of 0.7; we use 0.
        fl("Rotor Rate", 0.0, 2.0, 0.0, SlotUnit::Percent),
        deact(percent("Drive", 0.0)),
        choice("Model", &MODEL_NAMES, 0.0),
        width_db("Width"),
        percent("Mix", 0.33),
        unused(),
        unused(),
        unused(),
        unused(),
    ]
}

pub fn init_defaults(_p: &mut [SlotValue; 12]) {}

pub fn migrate(rev: i32, p: &mut [SlotValue; 12]) {
    if rev <= 12 {
        p[3].v = 0.7;
        p[4].v = 0.0;
        p[4].deactivated = true;
        p[5].v = 0.0;
        // SPEC? literal "Width := 1" on a dB width slot.
        p[6].v = 1.0;
        p[7].v = 1.0;
    }
}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    RINGOUT
}

pub struct Rotary {
    sr: f32,
    horn: Quadrature,
    rotor: Quadrature,
    xover: Biquad,
    sub: Biquad,
    delay: BlockDelay,
    dl: Line,
    dr: Line,
    gl: Line,
    gr: Line,
    drive: Lag,
    shaper: Shaper,
    width: Line,
    mix: Line,
}

impl Rotary {
    pub fn new(sr: f32) -> Rotary {
        let mut xover = Biquad::new();
        xover.lp2b(calc_omega(0.862496, sr), 0.707);
        let mut sub = Biquad::new();
        sub.lp2b(calc_omega(-1.14, sr), 0.707);
        Rotary {
            sr,
            horn: Quadrature::new(0.0),
            rotor: Quadrature::new(0.0),
            xover,
            sub,
            delay: BlockDelay::new(1 << 18),
            dl: Line::new(),
            dr: Line::new(),
            gl: Line::new(),
            gr: Line::new(),
            drive: Lag::new(0.004),
            shaper: Shaper::default(),
            width: Line::new(),
            mix: Line::new(),
        }
    }

    pub fn reset(&mut self) {
        *self = Rotary::new(self.sr);
    }

    fn horn_hz(p: &[SlotValue; 12], tsr: f32) -> f64 {
        // QUIRK_TEMPO_SCALES_EXPONENT
        ((p[0].v * tsr_if(&p[0], tsr)) as f64).exp2()
    }

    pub fn process(&mut self, p: &[SlotValue; 12], ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        let sr = self.sr;
        let f = Self::horn_hz(p, ctx.tsr);
        // Geometry from the horn phasor before this block's rotation.
        let (hr, hi) = (self.horn.r as f32, self.horn.i as f32);
        let dxl = -1.0 - hr;
        let dxr = 1.0 - hr;
        let dy = -2.0 - hi;
        let lenl = (dxl * dxl + dy * dy).sqrt();
        let lenr = (dxr * dxr + dy * dy).sqrt();
        let dop = p[1].v;
        self.dl.set_target(sr * 0.0018 * dop * lenl);
        self.dr.set_target(sr * 0.0018 * dop * lenr);
        let dotl = (dxl * hr + dy * hi) / lenl;
        let dotr = (dxr * hr + dy * hi) / lenr;
        let a = 0.6 * p[2].v;
        self.gl.set_target((1.0 - a) + a * dotl);
        self.gr.set_target((1.0 - a) + a * dotr);
        self.horn.set_rate(TAU * f * B as f64 / sr as f64);
        self.horn.step();
        self.rotor.set_rate(p[3].v as f64 * TAU * f / sr as f64);

        let drive_on = !p[4].deactivated;
        let m = p[5].i();
        let model = if (0..8).contains(&m) { m as usize } else { 0 };
        let ws = FX_SHAPER_SUBLIST[model];
        let (tweak, comp, start, squared) = MODEL_GAINS[model];
        if drive_on {
            self.drive.set(p[4].v);
        }
        let d = self.drive.v;
        let comp_factor = if d < start {
            1.0
        } else if squared {
            1.0 + (d * d - start) * comp
        } else {
            1.0 + (d - start) * comp
        };

        let mut horn = [0f32; B];
        let mut bass = [0f32; B];
        for k in 0..B {
            let mut x = 0.5 * (l[k] + r[k]);
            if drive_on {
                let dv = self.drive.process();
                let df = 1.0 + 15.0 * dv * dv;
                let y = if model < 3 { lookup_waveshape(ws, x * df) } else { self.shaper.tick(ws, x, df) };
                x = y * tweak / comp_factor;
            }
            let low = self.xover.tick_mono(x);
            horn[k] = x - low;
            let sub = self.sub.tick_mono(low);
            let mid = low - sub;
            bass[k] = sub + mid * (0.6 * self.rotor.r as f32 + 0.3);
            self.rotor.step();
        }
        let mut wl = [0f32; B];
        let mut wr = [0f32; B];
        for k in 0..B {
            let hl = self.delay.read(self.dl.at(k).max(B as f32), k);
            let hr = self.delay.read(self.dr.at(k).max(B as f32), k);
            wl[k] = self.gl.at(k) * hl + bass[k];
            wr[k] = self.gr.at(k) * hr + bass[k];
        }
        for (k, h) in horn.iter().enumerate() {
            self.delay.write_at(k, *h);
        }
        self.delay.advance();

        self.width.set_target_smoothed(db_to_linear(p[6].v));
        apply_width(&mut wl, &mut wr, &self.width);
        self.mix.set_target_smoothed(p[7].v);
        self.mix.fade_2_blocks(l, r, &wl, &wr);
    }

    pub fn process_only_control(&mut self, p: &[SlotValue; 12], ctx: &FxCtx) {
        let f = Self::horn_hz(p, ctx.tsr);
        let sr = self.sr as f64;
        self.horn.set_rate(TAU * f * B as f64 / sr);
        self.horn.step();
        self.rotor.set_rate(p[3].v as f64 * TAU * f * B as f64 / sr);
        self.rotor.step();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doppler_makes_left_and_right_differ() {
        let mut p = super::super::default_values(4);
        p[0].v = 3.0;
        p[1].v = 1.0;
        p[2].v = 0.5;
        p[3].v = 0.7;
        p[7].v = 1.0;
        let ctx = super::super::FxCtx::new(48000.0);
        let mut fx = Rotary::new(48000.0);
        let mut diff = 0.0f64;
        let mut n = 0usize;
        for b in 0..400 {
            let mut l = [0f32; 32];
            let mut r = [0f32; 32];
            for k in 0..32 {
                let t = (b * 32 + k) as f64 / 48000.0;
                l[k] = (std::f64::consts::TAU * 2000.0 * t).sin() as f32 * 0.5;
                r[k] = l[k];
            }
            fx.process(&p, &ctx, &mut l, &mut r);
            for k in 0..32 {
                assert!(l[k].is_finite() && r[k].is_finite());
                if b > 50 {
                    diff += ((l[k] - r[k]) as f64).abs();
                    n += 1;
                }
            }
        }
        assert!(diff / n as f64 > 1e-3, "{}", diff / n as f64);
    }

    #[test]
    fn driven_models_finite() {
        for model in 0..8 {
            let mut p = super::super::default_values(4);
            p[4].v = 1.0;
            p[5].v = model as f32;
            let ctx = super::super::FxCtx::new(48000.0);
            let mut fx = Rotary::new(48000.0);
            let mut rng = crate::dsp::Rng::new(3);
            for _ in 0..100 {
                let mut l = [0f32; 32];
                let mut r = [0f32; 32];
                for k in 0..32 {
                    l[k] = rng.bipolar();
                    r[k] = rng.bipolar();
                }
                fx.process(&p, &ctx, &mut l, &mut r);
                assert!(l.iter().chain(r.iter()).all(|x| x.is_finite() && x.abs() < 20.0), "model {model}");
            }
        }
    }
}
