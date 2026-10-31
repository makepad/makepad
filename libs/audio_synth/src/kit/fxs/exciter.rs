//! Exciter (type 19): a high-passed copy of the input is saturated and
//! scaled by an exponential level detector, then added to the dry signal.
//! Runs 2x oversampled. EFFECTS-2.md 6.4.

use super::common::*;
use super::FxCtx;
use crate::kit::core::{Biquad, HalfBand, SlotDef, SlotUnit, SlotValue};

/// The detector's time constants use the BASE sample rate although the
/// detector runs at 2x.
const QUIRK_DETECTOR_BASE_RATE: bool = true;
const _: () = assert!(QUIRK_DETECTOR_BASE_RATE);
/// One detector state serves both channels, updated with L then R each
/// oversampled sample.
const QUIRK_SHARED_DETECTOR: bool = true;
const _: () = assert!(QUIRK_SHARED_DETECTOR);

/// log2(5)/log2(100) and log2(20)/log2(100).
const T_MIN: f32 = 0.349485;
const T_MAX: f32 = 0.650515;

pub fn slot_defs() -> [SlotDef; 12] {
    [
        percent("Drive", 0.5),
        percent("Tone", 0.5),
        about(fl("Attack", T_MIN, T_MAX, 0.5, SlotUnit::None), "attack ms = 100^v (5..20 ms)"),
        about(fl("Release", T_MIN, T_MAX, 0.5, SlotUnit::None), "release ms = 10 * 100^v (50..200 ms)"),
        percent("Mix", 0.5),
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

pub struct Exciter {
    sr: f32,
    up: HalfBand,
    down: HalfBand,
    hp: Biquad,
    z: [f32; 2],
    drive: Line,
    mix: Line,
}

impl Exciter {
    pub fn new(sr: f32) -> Exciter {
        Exciter { sr, up: HalfBand::new(6, false), down: HalfBand::new(6, false), hp: Biquad::new(), z: [0.0; 2], drive: Line::new(), mix: Line::new() }
    }

    pub fn reset(&mut self) {
        *self = Exciter::new(self.sr);
    }

    pub fn process_only_control(&mut self, _p: &[SlotValue; 12], _ctx: &FxCtx) {}

    pub fn process(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        let sr = self.sr;
        let tone = clamp01(p[1].v);
        let fc = (500.0 * 20f32.powf(tone)).clamp(10.0, 0.48 * sr);
        self.hp.hp(2.0 * std::f64::consts::PI * fc as f64 / (2.0 * sr as f64), 0.7071);
        self.drive.set_target_smoothed(8.0 * clamp01(p[0].v).powf(1.5) * 0.2f32.powf(1.0 - tone));
        let att_ms = (100f32.powf(p[2].v)).clamp(2.5, 40.0);
        let rel_ms = (10.0 * 100f32.powf(p[3].v)).clamp(25.0, 400.0);
        let det_sr = sr;
        let tau = |ms: f32| (-std::f32::consts::TAU * 1000.0 / (det_sr * ms)).exp();
        let (ta, tr) = (tau(att_ms), tau(rel_ms));

        let mut dl = *l;
        let mut dr = *r;
        self.drive.multiply_2_blocks(&mut dl, &mut dr);
        let mut ul = [0f32; 64];
        let mut ur = [0f32; 64];
        self.up.upsample_into(&dl, &dr, &mut ul, &mut ur, 0.5);
        for k in 0..64 {
            let (xl, xr) = self.hp.tick(ul[k], ur[k]);
            let mut y = [0f32; 2];
            for (c, x) in [xl, xr].into_iter().enumerate() {
                let rect = 5.0 * ((1.9305 * x).exp() - 1.0);
                let zi = 0;
                let z = self.z[zi];
                let t = if rect > z { ta } else { tr };
                let z = rect + t * (z - rect);
                self.z[zi] = z;
                y[c] = x.tanh() * z;
            }
            ul[k] = y[0];
            ur[k] = y[1];
        }
        self.down.decimate(&mut ul, &mut ur);
        self.mix.set_target_smoothed(clamp01(p[4].v));
        for k in 0..B {
            let m = self.mix.at(k);
            l[k] += m * ul[k];
            r[k] += m * ur[k];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{default_values, FxCtx};
    use super::*;

    /// Magnitude of the DFT bin at `hz` over `x`.
    fn bin(x: &[f32], hz: f32) -> f32 {
        let (mut re, mut im) = (0f64, 0f64);
        for (n, v) in x.iter().enumerate() {
            let a = std::f64::consts::TAU * hz as f64 * n as f64 / 48000.0;
            re += *v as f64 * a.cos();
            im += *v as f64 * a.sin();
        }
        ((re * re + im * im).sqrt() / x.len() as f64) as f32
    }

    fn run(hz: f32) -> Vec<f32> {
        let mut p = default_values(19);
        p[4].v = 1.0;
        let ctx = FxCtx::new(48000.0);
        let mut fx = Exciter::new(48000.0);
        let mut wet = Vec::new();
        for b in 0..600 {
            let mut l = [0f32; 32];
            for k in 0..32 {
                l[k] = 0.5 * (std::f32::consts::TAU * hz * (b * 32 + k) as f32 / 48000.0).sin();
            }
            let dry = l;
            let mut r = l;
            fx.process(&p, &ctx, &mut l, &mut r);
            if b >= 100 {
                for k in 0..32 {
                    assert!(l[k].is_finite());
                    wet.push(l[k] - dry[k]);
                }
            }
        }
        wet
    }

    #[test]
    fn harmonics_only_above_the_tone_highpass() {
        // Tone 0.5: HP near 2.2 kHz. A 200 Hz sine is mostly filtered out of
        // the excited branch; a 3 kHz sine gets a strong 3rd harmonic.
        let low = run(200.0);
        let high = run(3000.0);
        let h_low = bin(&low, 600.0);
        let h_high = bin(&high, 9000.0);
        assert!(h_high > 1e-4, "{h_high}");
        assert!(h_high > 10.0 * h_low, "{h_high} {h_low}");
    }
}
