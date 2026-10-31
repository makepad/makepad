//! Pitch-tracking sine generator (type 24): a zero-crossing period tracker
//! on a filtered copy of the input drives a sine oscillator per channel; the
//! sine is either shaped by the input's envelope or ring-modulated with the
//! input.

use super::common::*;
use super::FxCtx;
use crate::kit::core::{calc_omega, db_to_linear, note_to_pitch, Biquad, SlotDef, SlotUnit, SlotValue};

/// Both channels' envelope-shaped sine uses the LEFT channel's envelope:
/// the reference does this, so stereo material yields a left-driven level.
pub const QUIRK_LEFT_ENVELOPE_BOTH: bool = true;
const _: () = assert!(QUIRK_LEFT_ENVELOPE_BOTH);
/// The width line starts from a side gain of 0 at init (the reference's
/// smoother starts at zero), so the first blocks fade in from mono.
pub const QUIRK_WIDTH_FROM_ZERO: f32 = 0.0;

pub fn slot_defs() -> [SlotDef; 12] {
    [
        db("Threshold", -96.0, 0.0, -24.0),
        percent("Speed", 0.5),
        // The reference's labels for p2/p3 are swapped; behaviour: p2 = HP
        // on the detector, p3 = LP on the detector.
        freq_d("Low Cut", 0.0),
        freq_d("High Cut", 0.0),
        fl("Pitch", -60.0, 60.0, 0.0, SlotUnit::Semis),
        percent("Ring Modulation", 0.5),
        about(bipolar("Width", 0.0), "side gain in dB (-1..1 dB)"),
        percent("Mix", 1.0),
        unused(),
        unused(),
        unused(),
        unused(),
    ]
}

// SPEC? no default deactivation is stated for the detector cuts; both start
// active at 0 (440 Hz) as listed.
pub fn init_defaults(_p: &mut [SlotValue; 12]) {}

pub fn migrate(_rev: i32, _p: &mut [SlotValue; 12]) {}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    1000
}

#[derive(Clone, Copy)]
struct Tracker {
    len: f32,
    target: f32,
    smooth: f32,
    prev: f32,
    detected: bool,
}

impl Default for Tracker {
    fn default() -> Self {
        Tracker { len: 100.0, target: 100.0, smooth: 100.0, prev: 0.0, detected: false }
    }
}

impl Tracker {
    #[inline]
    fn tick(&mut self, x: f32, thr: f32) {
        self.len += 1.0;
        if self.prev < 0.0 && x >= 0.0 {
            if x > thr && self.len > 16.0 {
                self.target = if self.len > 10.0 * self.smooth { self.smooth } else { self.len };
                if !self.detected {
                    self.smooth = self.len;
                    self.detected = true;
                }
            }
            self.len = 0.0;
        }
        self.prev = x;
    }
}

pub struct Treemonster {
    sr: f32,
    hp: Biquad,
    lp: Biquad,
    trk: [Tracker; 2],
    osc: [Quadrature; 2],
    env: [f32; 2],
    att: f32,
    rel: f32,
    rm: Line,
    width: Line,
    mix: Line,
}

impl Treemonster {
    pub fn new(sr: f32) -> Treemonster {
        let mut width = Line::new();
        width.snap(QUIRK_WIDTH_FROM_ZERO);
        Treemonster {
            sr,
            hp: Biquad::new(),
            lp: Biquad::new(),
            trk: [Tracker::default(); 2],
            osc: [Quadrature::new(0.0), Quadrature::new(std::f64::consts::FRAC_PI_2)],
            env: [0.0; 2],
            att: 0.01f32.powf(1.0 / (0.005 * sr)),
            rel: 0.01f32.powf(1.0 / (0.5 * sr)),
            rm: Line::new(),
            width,
            mix: Line::new(),
        }
    }

    pub fn reset(&mut self) {
        let sr = self.sr;
        *self = Treemonster::new(sr);
    }

    fn smooth_block(&mut self, p: &[SlotValue; 12]) {
        let q = clamp01(p[1].v).powi(4);
        let c = 0.9999 - q * 0.0999 / 128.0;
        let n = B as f32 * 48000.0 / self.sr;
        for t in &mut self.trk {
            let mut i = 0.0;
            while i < n {
                t.smooth = c * t.smooth + (1.0 - c) * t.target;
                i += 1.0;
            }
        }
    }

    pub fn process(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        let mut dl = *l;
        let mut dr = *r;
        self.hp.hp(calc_omega(p[2].v / 12.0, self.sr), 0.707);
        self.lp.lp2b(calc_omega(p[3].v / 12.0, self.sr), 0.707);
        if !p[2].deactivated {
            self.hp.process(&mut dl, &mut dr);
        }
        if !p[3].deactivated {
            self.lp.process(&mut dl, &mut dr);
        }
        let thr = db_to_linear(p[0].v.clamp(-96.0, 0.0));
        for k in 0..32 {
            self.trk[0].tick(dl[k], thr);
            self.trk[1].tick(dr[k], thr);
        }
        self.smooth_block(p);
        let pm = note_to_pitch(p[4].v) as f64;
        for c in 0..2 {
            let w = std::f64::consts::TAU / (self.trk[c].smooth.max(2.0) as f64) * pm;
            self.osc[c].set_rate(w);
        }
        self.rm.set_target_smoothed(clamp01(p[5].v));
        let mut wl = [0f32; 32];
        let mut wr = [0f32; 32];
        for k in 0..32 {
            let x = [l[k], r[k]];
            let mut o = [0f32; 2];
            for c in 0..2 {
                let v = x[c];
                let cf = if v > self.env[c] { self.att } else { self.rel };
                self.env[c] = cf * (self.env[c] - v) + v;
                o[c] = self.osc[c].r as f32;
                self.osc[c].step();
            }
            let m = self.rm.at(k);
            let e = self.env[0];
            wl[k] = o[0] * e * (1.0 - m) + o[0] * x[0] * m;
            wr[k] = o[1] * e * (1.0 - m) + o[1] * x[1] * m;
        }
        self.width.set_target_smoothed(db_to_linear(p[6].v));
        apply_width(&mut wl, &mut wr, &self.width);
        self.mix.set_target_smoothed(p[7].v);
        self.mix.fade_2_blocks(l, r, &wl, &wr);
    }

    pub fn process_only_control(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx) {
        self.smooth_block(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::fxs::{default_values, FxCtx};

    #[test]
    fn tracks_sine_pitch() {
        let sr = 48000.0f32;
        let mut p = default_values(24);
        p[0].v = -96.0;
        p[1].v = 1.0;
        p[2].deactivated = true;
        p[3].deactivated = true;
        p[5].v = 0.0;
        let mut fx = Treemonster::new(sr);
        let ctx = FxCtx::new(sr);
        let mut out = Vec::new();
        for b in 0..3000 {
            let mut l = [0f32; 32];
            for (k, s) in l.iter_mut().enumerate() {
                let n = (b * 32 + k) as f64;
                *s = 0.5 * (std::f64::consts::TAU * 300.0 * n / sr as f64).sin() as f32;
            }
            let mut r = l;
            fx.process(&p, &ctx, &mut l, &mut r);
            out.extend_from_slice(&l);
        }
        let tail = &out[out.len() - 24000..];
        let ups = tail.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f32;
        let hz = ups / 0.5;
        assert!((hz / 300.0 - 1.0).abs() < 0.01, "{hz}");
    }
}
