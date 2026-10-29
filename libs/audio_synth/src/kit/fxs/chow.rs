//! One-sided hard-knee clipper/compressor (type 18), 4x oversampled: above
//! the threshold the signal is divided by the ratio (below minus the
//! threshold when flipped), then a makeup gain and a mix.

use super::common::*;
use super::FxCtx;
use crate::kit::core::{db_to_linear, HalfBand, SlotDef, SlotUnit, SlotValue};

/// The threshold applies to the 4x-upsampled signal, which the zero-stuffing
/// upsamplers leave at a quarter of the input level; the makeup carries a
/// factor 4 for it. The reference does this, so we mirror the level staging.
pub const QUIRK_MAKEUP_TIMES_4: f32 = 4.0;

pub fn slot_defs() -> [SlotDef; 12] {
    [
        db("Threshold", -96.0, 0.0, -24.0),
        fl("Ratio", 1.0, 20.0, 10.0, SlotUnit::None),
        choice("Flip", &["Off", "On"], 0.0),
        percent("Mix", 1.0),
        unused(),
        unused(),
        unused(),
        unused(),
        unused(),
        unused(),
        unused(),
        unused(),
    ]
}

pub fn init_defaults(_p: &mut [SlotValue; 12]) {}

pub fn migrate(_rev: i32, _p: &mut [SlotValue; 12]) {}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    -1
}

/// The static curve on one (oversampled) sample.
#[inline]
pub fn truculate(x: f32, t: f32, r: f32, flip: bool) -> f32 {
    if !flip {
        if x > t {
            t + (x - t) / r
        } else {
            x
        }
    } else if x < -t {
        -t + (x + t) / r
    } else {
        x
    }
}

/// A multiplicative (geometric) ramp to a new target over 200 steps.
#[derive(Clone, Copy, Debug)]
struct GeoRamp {
    cur: f32,
    target: f32,
    factor: f32,
    left: u32,
    started: bool,
}

impl GeoRamp {
    const STEPS: u32 = 200;
    fn new() -> GeoRamp {
        GeoRamp { cur: 1.0, target: 1.0, factor: 1.0, left: 0, started: false }
    }
    fn set(&mut self, t: f32) {
        if !self.started {
            self.cur = t;
            self.target = t;
            self.started = true;
            self.left = 0;
        } else if t != self.target {
            self.target = t;
            self.factor = (t / self.cur).powf(1.0 / Self::STEPS as f32);
            self.left = Self::STEPS;
        }
    }
    #[inline]
    fn step(&mut self) -> f32 {
        if self.left > 0 {
            self.cur *= self.factor;
            self.left -= 1;
            if self.left == 0 {
                self.cur = self.target;
            }
        }
        self.cur
    }
}

pub struct Chow {
    up1: HalfBand,
    up2: HalfBand,
    down1: HalfBand,
    down2: HalfBand,
    thr: GeoRamp,
    ratio: GeoRamp,
    makeup: Line,
    mix: Line,
}

impl Chow {
    pub fn new(_sr: f32) -> Chow {
        Chow {
            up1: HalfBand::new(6, false),
            up2: HalfBand::new(6, false),
            down1: HalfBand::new(6, false),
            down2: HalfBand::new(6, false),
            thr: GeoRamp::new(),
            ratio: GeoRamp::new(),
            makeup: Line::new(),
            mix: Line::new(),
        }
    }

    pub fn reset(&mut self) {
        self.up1.reset();
        self.up2.reset();
        self.down1.reset();
        self.down2.reset();
    }

    pub fn process(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        let thr_db = p[0].v.clamp(-96.0, 0.0);
        let ratio = p[1].v.clamp(1.0, 20.0);
        self.thr.set(db_to_linear(thr_db));
        self.ratio.set(ratio);
        self.makeup.set_target_smoothed(QUIRK_MAKEUP_TIMES_4 * db_to_linear((thr_db / 12.0) * (1.0 / ratio - 1.0) - 1.0));
        let flip = p[2].on();

        let mut l2 = [0f32; 64];
        let mut r2 = [0f32; 64];
        self.up1.upsample_into(&l[..], &r[..], &mut l2, &mut r2, 0.5);
        let mut l4 = [0f32; 128];
        let mut r4 = [0f32; 128];
        self.up2.upsample_into(&l2, &r2, &mut l4, &mut r4, 0.5);
        for k in 0..32 {
            let t = self.thr.step();
            let q = self.ratio.step();
            for j in 4 * k..4 * k + 4 {
                l4[j] = truculate(l4[j], t, q, flip);
                r4[j] = truculate(r4[j], t, q, flip);
            }
        }
        self.down1.decimate(&mut l4, &mut r4);
        let (mut wl, mut wr) = ([0f32; 32], [0f32; 32]);
        self.down2.decimate_into(&l4[..64], &r4[..64], &mut wl, &mut wr);
        self.makeup.multiply_2_blocks(&mut wl, &mut wr);
        self.mix.set_target_smoothed(clamp01(p[3].v));
        self.mix.fade_2_blocks(l, r, &wl, &wr);
    }

    pub fn process_only_control(&mut self, _p: &[SlotValue; 12], _ctx: &FxCtx) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::fxs::eq::tests::sine_gain;
    use crate::kit::fxs::{default_values, FxCtx};

    #[test]
    fn static_curve() {
        let (t, r) = (0.25, 4.0);
        assert_eq!(truculate(0.1, t, r, false), 0.1);
        assert!((truculate(0.65, t, r, false) - 0.35).abs() < 1e-6);
        assert_eq!(truculate(-0.9, t, r, false), -0.9);
        assert!((truculate(-0.65, t, r, true) + 0.35).abs() < 1e-6);
        assert_eq!(truculate(0.9, t, r, true), 0.9);
        // Continuous at the knee, slope 1/r above it.
        let e = 1e-3;
        assert!((truculate(t + e, t, r, false) - (t + e / r)).abs() < 1e-6);
    }

    #[test]
    fn small_signal_gain_is_makeup() {
        let p = default_values(18);
        let mut fx = Chow::new(48000.0);
        let ctx = FxCtx::new(48000.0);
        let g = sine_gain(|l, r| fx.process(&p, &ctx, l, r), 600.0, 0.001, 0.5);
        let want = db_to_linear((-24.0 / 12.0) * (1.0 / 10.0 - 1.0) - 1.0) as f64;
        let err = 20.0 * (g / want).log10();
        assert!(err.abs() < 0.1, "{g} {want} {err}");
    }
}
