//! Conditioner (type 8): bass and treble shelving-style peaks, stereo width
//! and balance with a side low cut, then a 128-sample look-ahead limiter
//! and an output attenuator.

use super::common::*;
use super::FxCtx;
use crate::kit::core::{calc_omega, db_to_linear, Biquad, SlotDef, SlotValue};

/// The limiter's ring of peak values is read at this fixed slot, not at the
/// newest entry, so the detector samples one input every 128 samples; the
/// reference does this and patches are tuned to the resulting release
/// behaviour, so we mirror it.
pub const QUIRK_DETECTOR_SLOT: usize = 126;
/// Look-ahead length of the limiter (samples).
const LOOKAHEAD: usize = 128;

pub fn slot_defs() -> [SlotDef; 12] {
    [
        deact(db("Bass", -12.0, 12.0, 0.0)),
        deact(db("Treble", -12.0, 12.0, 0.0)),
        // SPEC? the spec lists the default as "(0)" and the side gain as
        // S x Width, which makes the default output mono.
        bipolar("Width", 0.0),
        bipolar("Balance", 0.0),
        db("Threshold", -48.0, 0.0, 0.0),
        bipolar("Attack Rate", 0.0),
        bipolar("Release Rate", 0.0),
        db("Gain", -48.0, 0.0, 0.0),
        freq_d("Side Low Cut", -60.0),
        unused(),
        unused(),
        unused(),
    ]
}

pub fn init_defaults(p: &mut [SlotValue; 12]) {
    p[8].deactivated = true;
}

pub fn migrate(rev: i32, p: &mut [SlotValue; 12]) {
    if rev <= 15 {
        p[0].deactivated = false;
        p[1].deactivated = false;
    }
    if rev <= 16 {
        p[8].v = -60.0;
        p[8].deactivated = true;
    }
}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    100
}

pub struct Conditioner {
    sr: f32,
    bass: Biquad,
    treble: Biquad,
    side_hp: Biquad,
    gain_l: Line,
    gain_r: Line,
    width: Line,
    post: Line,
    dl: [f32; LOOKAHEAD],
    dr: [f32; LOOKAHEAD],
    pk: [f32; LOOKAHEAD],
    pos: usize,
    f1: f32,
    f2: f32,
    /// The last block's limiter gain (for a meter).
    pub gain_reduction: f32,
}

impl Conditioner {
    pub fn new(sr: f32) -> Conditioner {
        Conditioner {
            sr,
            bass: Biquad::new(),
            treble: Biquad::new(),
            side_hp: Biquad::new(),
            gain_l: Line::new(),
            gain_r: Line::new(),
            width: Line::new(),
            post: Line::new(),
            dl: [0.0; LOOKAHEAD],
            dr: [0.0; LOOKAHEAD],
            pk: [0.0; LOOKAHEAD],
            pos: 0,
            f1: 1.0,
            f2: 1.0,
            gain_reduction: 1.0,
        }
    }

    pub fn reset(&mut self) {
        self.bass.suspend();
        self.treble.suspend();
        self.side_hp.suspend();
        self.dl = [0.0; LOOKAHEAD];
        self.dr = [0.0; LOOKAHEAD];
        self.pk = [0.0; LOOKAHEAD];
        self.pos = 0;
        self.f1 = 1.0;
        self.f2 = 1.0;
    }

    fn coefs(p: &[SlotValue; 12]) -> (f32, f32) {
        (0.001 * (1.0 + 0.9 * p[5].v).powi(2), 0.0001 * (1.0 + 0.9 * p[6].v).powi(2))
    }

    pub fn process(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        self.bass.peak_eq(calc_omega(-2.5, self.sr), 2.0, p[0].v as f64);
        self.treble.peak_eq(calc_omega(4.75, self.sr), 2.0, p[1].v as f64);
        self.side_hp.hp(calc_omega(p[8].v / 12.0, self.sr), 0.4);
        if !p[0].deactivated {
            self.bass.process(l, r);
        }
        if !p[1].deactivated {
            self.treble.process(l, r);
        }
        let pregain = db_to_linear(-p[4].v);
        self.gain_l.set_target_smoothed(pregain * 0.5 * (1.0 - p[3].v).clamp(-1.0, 1.0));
        self.gain_r.set_target_smoothed(pregain * 0.5 * (1.0 + p[3].v).clamp(-1.0, 1.0));
        self.width.set_target_smoothed(p[2].v.clamp(-1.0, 1.0));
        self.post.set_target_smoothed(db_to_linear(p[7].v));

        // SPEC? the 0.5 in the L/R gains implies an unhalved M/S encode
        // (M = L + R, S = L - R); with it the defaults pass at unity.
        let mut m = [0f32; 32];
        let mut s = [0f32; 32];
        for k in 0..32 {
            m[k] = l[k] + r[k];
            s[k] = l[k] - r[k];
        }
        if !p[8].deactivated {
            self.side_hp.process_mono(&mut s);
        }
        for k in 0..32 {
            let sk = s[k] * self.width.at(k);
            l[k] = (m[k] + sk) * self.gain_l.at(k);
            r[k] = (m[k] - sk) * self.gain_r.at(k);
        }

        let (att, rel) = Self::coefs(p);
        for k in 0..32 {
            let (xl, xr) = (l[k], r[k]);
            let pos = self.pos;
            let ol = self.dl[pos];
            let or = self.dr[pos];
            self.dl[pos] = xl;
            self.dr[pos] = xr;
            let a = xl.abs().max(xr.abs());
            self.pk[pos] = a * a;
            let la = (2.0 * self.pk[QUIRK_DETECTOR_SLOT]).sqrt().max(1.0);
            self.f1 = (1.0 - att) * self.f1 + att * la;
            self.f2 = (1.0 - rel) * self.f2 + rel * self.f1;
            if self.f1 > self.f2 {
                self.f2 = self.f1;
            }
            let g = 1.0 / self.f2;
            let post = self.post.at(k);
            l[k] = ol * g * post;
            r[k] = or * g * post;
            self.pos = (pos + 1) & (LOOKAHEAD - 1);
        }
        self.gain_reduction = 1.0 / self.f2;
    }

    pub fn process_only_control(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx) {
        let (att, rel) = Self::coefs(p);
        for _ in 0..32 {
            self.f1 = (1.0 - att) * self.f1 + att;
            self.f2 = (1.0 - rel) * self.f2 + rel * self.f1;
            if self.f1 > self.f2 {
                self.f2 = self.f1;
            }
        }
        self.gain_reduction = 1.0 / self.f2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::fxs::eq::tests::sine_gain;
    use crate::kit::fxs::{default_values, FxCtx};

    #[test]
    fn bass_treble_match_recipe() {
        let sr = 48000.0f32;
        let mut p = default_values(8);
        p[0].v = 6.0;
        p[1].v = -5.0;
        p[2].v = 1.0;
        let mut b = Biquad::new();
        b.peak_eq(calc_omega(-2.5, sr), 2.0, 6.0);
        let mut t = Biquad::new();
        t.peak_eq(calc_omega(4.75, sr), 2.0, -5.0);
        for hz in [80.0f64, 1000.0, 12000.0] {
            let mut fx = Conditioner::new(sr);
            let ctx = FxCtx::new(sr);
            let g = sine_gain(|l, r| fx.process(&p, &ctx, l, r), hz, 0.2, 1.0);
            let w = std::f64::consts::TAU * hz / sr as f64;
            let want = b.response(w) * t.response(w);
            let err = 20.0 * (g / want).log10();
            assert!(err.abs() < 0.1, "{hz} Hz: {g} vs {want} ({err} dB)");
        }
    }

    #[test]
    fn limiter_holds_loud_input_down() {
        let sr = 48000.0f32;
        let mut p = default_values(8);
        p[2].v = 1.0;
        p[4].v = -12.0;
        let mut fx = Conditioner::new(sr);
        let ctx = FxCtx::new(sr);
        let g = sine_gain(|l, r| fx.process(&p, &ctx, l, r), 1000.0, 0.5, 2.0);
        // 0.5 x 4 (+12 dB) = 2 into the limiter: it must pull it back near 1.
        assert!(g * 0.5 < 1.2, "{g}");
        assert!(fx.gain_reduction < 0.8);
    }
}
