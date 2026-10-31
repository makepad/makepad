//! Mid-side tool (type 26): encode (or take the input as mid/side already),
//! filter mid and side separately (low cut, peak band, high cut), set their
//! levels, decode (or leave as mid/side) and balance.

use super::common::*;
use super::FxCtx;
use crate::kit::core::{calc_omega, db_to_linear, Biquad, SlotDef, SlotValue};

pub const MATRIX_NAMES: [&str; 3] = ["L-R > M-S > L-R", "L-R > M-S", "M-S > L-R"];

pub fn slot_defs() -> [SlotDef; 12] {
    [
        choice("Matrix", &MATRIX_NAMES, 0.0),
        freq_d("Mid Low Cut", -60.0),
        deact(db("Mid Gain", -24.0, 24.0, 0.0)),
        freq("Mid Frequency", -6.63049),
        freq_d("Mid High Cut", 70.0),
        freq_d("Side Low Cut", -60.0),
        deact(db("Side Gain", -24.0, 24.0, 0.0)),
        freq("Side Frequency", 50.2131),
        freq_d("Side High Cut", 70.0),
        db("Mid Level", -48.0, 12.0, 0.0),
        db("Side Level", -48.0, 12.0, 0.0),
        bipolar("Balance", 0.0),
    ]
}

pub fn init_defaults(p: &mut [SlotValue; 12]) {
    for i in [1, 2, 4, 5, 6, 8] {
        p[i].deactivated = true;
    }
}

pub fn migrate(_rev: i32, _p: &mut [SlotValue; 12]) {}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    -1
}

/// One chain's filters: low cut, peak, high cut.
#[derive(Clone, Copy, Default)]
struct Chain {
    hp: Biquad,
    pk: Biquad,
    lp: Biquad,
}

impl Chain {
    fn set(&mut self, p: &[SlotValue; 12], base: usize, sr: f32) {
        self.hp.hp(calc_omega(p[base].v / 12.0, sr), 0.4);
        self.pk.peak_eq(calc_omega(p[base + 2].v / 12.0, sr), 1.0, p[base + 1].v as f64);
        self.lp.lp(calc_omega(p[base + 3].v / 12.0, sr), 0.4);
    }
    fn run(&mut self, p: &[SlotValue; 12], base: usize, x: &mut [f32; 32]) {
        if !p[base].deactivated {
            self.hp.process_mono(x);
        }
        if !p[base + 1].deactivated {
            self.pk.process_mono(x);
        }
        if !p[base + 3].deactivated {
            self.lp.process_mono(x);
        }
    }
    fn reset(&mut self) {
        self.hp.suspend();
        self.pk.suspend();
        self.lp.suspend();
    }
}

pub struct MsTool {
    sr: f32,
    mid: Chain,
    side: Chain,
    gm: Line,
    gs: Line,
    bl: Line,
    br: Line,
}

impl MsTool {
    pub fn new(sr: f32) -> MsTool {
        MsTool {
            sr,
            mid: Chain::default(),
            side: Chain::default(),
            gm: Line::new(),
            gs: Line::new(),
            bl: Line::new(),
            br: Line::new(),
        }
    }

    pub fn reset(&mut self) {
        self.mid.reset();
        self.side.reset();
    }

    pub fn process(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        self.mid.set(p, 1, self.sr);
        self.side.set(p, 5, self.sr);
        let matrix = p[0].i();
        let mut m = [0f32; 32];
        let mut s = [0f32; 32];
        for k in 0..32 {
            if matrix == 2 {
                m[k] = l[k];
                s[k] = r[k];
            } else {
                m[k] = 0.5 * (l[k] + r[k]);
                s[k] = 0.5 * (l[k] - r[k]);
            }
        }
        self.mid.run(p, 1, &mut m);
        self.side.run(p, 5, &mut s);
        self.gm.set_target_smoothed(db_to_linear(p[9].v));
        self.gs.set_target_smoothed(db_to_linear(p[10].v));
        self.gm.multiply_block(&mut m);
        self.gs.multiply_block(&mut s);
        self.bl.set_target_smoothed((1.0 - p[11].v).clamp(-1.0, 1.0));
        self.br.set_target_smoothed((1.0 + p[11].v).clamp(-1.0, 1.0));
        for k in 0..32 {
            let (a, b) = if matrix == 1 { (m[k], s[k]) } else { (m[k] + s[k], m[k] - s[k]) };
            l[k] = a * self.bl.at(k);
            r[k] = b * self.br.at(k);
        }
    }

    pub fn process_only_control(&mut self, _p: &[SlotValue; 12], _ctx: &FxCtx) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::fxs::eq::tests::sine_gain;
    use crate::kit::fxs::{default_values, FxCtx};

    #[test]
    fn mid_chain_matches_recipe() {
        let sr = 48000.0f32;
        let mut p = default_values(26);
        p[1].v = -36.0;
        p[1].deactivated = false;
        p[2].v = 6.0;
        p[2].deactivated = false;
        p[4].v = 60.0;
        p[4].deactivated = false;
        p[9].v = -3.0;
        let mut c = Chain::default();
        c.set(&p, 1, sr);
        for hz in [100.0f64, 300.0, 3000.0, 12000.0] {
            let mut fx = MsTool::new(sr);
            let ctx = FxCtx::new(sr);
            let g = sine_gain(|l, r| fx.process(&p, &ctx, l, r), hz, 0.25, 1.0);
            let w = std::f64::consts::TAU * hz / sr as f64;
            let want = c.hp.response(w) * c.pk.response(w) * c.lp.response(w) * db_to_linear(-3.0) as f64;
            let err = 20.0 * (g / want).log10();
            assert!(err.abs() < 0.1, "{hz} Hz: {g} vs {want} ({err} dB)");
        }
    }

    #[test]
    fn defaults_pass_through() {
        let p = default_values(26);
        let mut fx = MsTool::new(48000.0);
        let ctx = FxCtx::new(48000.0);
        let mut l = [0f32; 32];
        let mut r = [0f32; 32];
        for k in 0..32 {
            l[k] = (k as f32 * 0.3).sin();
            r[k] = (k as f32 * 0.7).cos();
        }
        let (l0, r0) = (l, r);
        fx.process(&p, &ctx, &mut l, &mut r);
        for k in 0..32 {
            assert!((l[k] - l0[k]).abs() < 1e-5 && (r[k] - r0[k]).abs() < 1e-5);
        }
    }
}
