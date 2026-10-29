//! Waveshaper (type 25): optional pre high/low cuts, then any of the 43
//! shapers 2x oversampled with drive and bias (gain-staged like the voice
//! shaper), optional post cuts, an output boost and the wet/dry mix.

use super::common::*;
use crate::kit::shaper::{Shaper, SHAPER_NAMES};
use super::FxCtx;
use crate::kit::core::{calc_omega, Biquad, HalfBand, SlotDef, SlotValue};

/// The reference reads Drive and Gain from the slots' unmodulated base
/// values, so modulation routed to them has no effect; mirrored by the host
/// handing this effect base values for p4 and p7.
// SPEC? the effect only sees one value per slot; SYNTHPORT must pass p4/p7 unmodulated.
#[allow(dead_code)]
const QUIRK_DRIVE_GAIN_IGNORE_MODULATION: bool = true;

pub fn slot_defs() -> [SlotDef; 12] {
    [
        freq_d("Pre Low Cut", -60.0),
        freq_d("Pre High Cut", 70.0),
        choice("Shape", &SHAPER_NAMES, 0.0),
        bipolar("Bias", 0.0),
        extend(db("Drive", -24.0, 24.0, 0.0)),
        freq_d("Post Low Cut", -60.0),
        freq_d("Post High Cut", 70.0),
        extend(db("Gain", -24.0, 24.0, 0.0)),
        percent("Mix", 1.0),
        unused(),
        unused(),
        unused(),
    ]
}

/// The pre cuts start deactivated.
// SPEC? the post cuts are only marked "D"; we start them active (transparent at -60 / 70).
pub fn init_defaults(p: &mut [SlotValue; 12]) {
    p[0].deactivated = true;
    p[1].deactivated = true;
}

pub fn migrate(rev: i32, p: &mut [SlotValue; 12]) {
    if rev < 27 {
        p[4].extend = false;
        p[7].extend = false;
    }
}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    -1
}

pub struct Waveshaper {
    sr: f32,
    pre_lp: Biquad,
    pre_hp: Biquad,
    post_lp: Biquad,
    post_hp: Biquad,
    up: HalfBand,
    down: HalfBand,
    sh: [Shaper; 2],
    shape: u8,
    drive: f32,
    bias: f32,
    started: bool,
    boost: Line,
    mix: Line,
}

impl Waveshaper {
    pub fn new(sr: f32) -> Waveshaper {
        Waveshaper {
            sr,
            pre_lp: Biquad::new(),
            pre_hp: Biquad::new(),
            post_lp: Biquad::new(),
            post_hp: Biquad::new(),
            up: HalfBand::new(12, true),
            down: HalfBand::new(12, true),
            sh: [Shaper::default(); 2],
            shape: 0,
            drive: 1.0,
            bias: 0.0,
            started: false,
            boost: Line::new(),
            mix: Line::new(),
        }
    }

    pub fn reset(&mut self) {
        *self = Waveshaper::new(self.sr);
    }

    pub fn process(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        let sr = self.sr;
        let mut wl = *l;
        let mut wr = *r;
        if !p[1].deactivated {
            self.pre_lp.lp2b(calc_omega(oct(p[1].v), sr), 0.707);
            self.pre_lp.process(&mut wl, &mut wr);
        }
        if !p[0].deactivated {
            self.pre_hp.hp(calc_omega(oct(p[0].v), sr), 0.707);
            self.pre_hp.process(&mut wl, &mut wr);
        }

        let s = p[2].i();
        let shape = if (0..43).contains(&s) { s as u8 } else { 0 };
        if shape != self.shape {
            self.sh = [Shaper::default(); 2];
            self.shape = shape;
        }
        let drive = (ext(&p[4], |v| 2.0 * v) / 6.0).exp2();
        let bias = p[3].v.clamp(-1.0, 1.0);
        if !self.started {
            self.drive = drive;
            self.bias = bias;
            self.started = true;
        }
        let mut ol = [0f32; 64];
        let mut or = [0f32; 64];
        self.up.upsample_into(&wl, &wr, &mut ol, &mut or, 0.5);
        for i in 0..64 {
            let t = (i + 1) as f32 / 64.0;
            let d = self.drive + (drive - self.drive) * t;
            let b = self.bias + (bias - self.bias) * t;
            ol[i] = self.sh[0].tick(shape, 2.0 * 3.0 * ol[i] + b, d) / 3.0;
            or[i] = self.sh[1].tick(shape, 2.0 * 3.0 * or[i] + b, d) / 3.0;
        }
        self.drive = drive;
        self.bias = bias;
        self.down.decimate(&mut ol, &mut or);
        for k in 0..B {
            wl[k] = ol[k];
            wr[k] = or[k];
        }
        if !p[6].deactivated {
            self.post_lp.lp2b(calc_omega(oct(p[6].v), sr), 0.707);
            self.post_lp.process(&mut wl, &mut wr);
        }
        if !p[5].deactivated {
            self.post_hp.hp(calc_omega(oct(p[5].v), sr), 0.707);
            self.post_hp.process(&mut wl, &mut wr);
        }
        self.boost.set_target_smoothed((ext(&p[7], |v| 2.0 * v) / 18.0).exp2());
        self.boost.multiply_2_blocks(&mut wl, &mut wr);
        self.mix.set_target_smoothed(clamp01(p[8].v));
        self.mix.fade_2_blocks(l, r, &wl, &wr);
    }

    pub fn process_only_control(&mut self, _p: &[SlotValue; 12], _ctx: &FxCtx) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shape_finite_and_bounded_on_loud_noise() {
        for shape in 0..43 {
            let mut p = super::super::default_values(25);
            p[2].v = shape as f32;
            // Keep folder inputs inside +-10 (outside, the folders output 0
            // and the ADAA quotient jumps: real, but not what this checks).
            p[4].v = 6.0;
            p[3].v = 0.3;
            let ctx = super::super::FxCtx::new(48000.0);
            let mut fx = Waveshaper::new(48000.0);
            let mut rng = crate::dsp::Rng::new(11);
            for _ in 0..200 {
                let mut l = [0f32; 32];
                let mut r = [0f32; 32];
                for k in 0..32 {
                    l[k] = 0.5 * rng.bipolar();
                    r[k] = 0.5 * rng.bipolar();
                }
                fx.process(&p, &ctx, &mut l, &mut r);
                assert!(l.iter().chain(r.iter()).all(|x| x.is_finite() && x.abs() < 20.0), "shape {shape}");
            }
        }
    }

    #[test]
    fn soft_shape_small_signal_is_near_unity() {
        // Soft at 0 dB: small sines pass with ~unit gain (2 x 3 / 3 staging,
        // half-band interpolator gain 1/2).
        let mut p = super::super::default_values(25);
        p[2].v = 1.0;
        let ctx = super::super::FxCtx::new(48000.0);
        let mut fx = Waveshaper::new(48000.0);
        let mut peak = 0.0f32;
        for b in 0..200 {
            let mut l = [0f32; 32];
            let mut r = [0f32; 32];
            for k in 0..32 {
                let t = (b * 32 + k) as f64 / 48000.0;
                l[k] = 0.01 * (std::f64::consts::TAU * 500.0 * t).sin() as f32;
                r[k] = l[k];
            }
            fx.process(&p, &ctx, &mut l, &mut r);
            if b > 100 {
                peak = l.iter().fold(peak, |m, x| m.max(x.abs()));
            }
        }
        assert!((peak / 0.01 - 1.0).abs() < 0.05, "{peak}");
    }
}
