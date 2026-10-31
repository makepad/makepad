//! Three-band parametric EQ (type 6): three peaking bands (each can be
//! switched off by deactivating its gain), an output gain and a bipolar
//! wet/dry mix (negative mix subtracts the equalised signal).

use super::common::*;
use super::FxCtx;
use crate::kit::core::{calc_omega, db_to_linear, Biquad, SlotDef, SlotUnit, SlotValue};

pub fn slot_defs() -> [SlotDef; 12] {
    let bw = |n: &'static str| about(fl(n, 0.0, 5.0, 2.0, SlotUnit::None), "bandwidth in octaves");
    [
        deact(db("Gain 1", -48.0, 48.0, 0.0)),
        freq("Frequency 1", -30.0),
        bw("Bandwidth 1"),
        deact(db("Gain 2", -48.0, 48.0, 0.0)),
        freq("Frequency 2", 6.0),
        bw("Bandwidth 2"),
        deact(db("Gain 3", -48.0, 48.0, 0.0)),
        freq("Frequency 3", 54.0),
        bw("Bandwidth 3"),
        db("Gain", -48.0, 48.0, 0.0),
        bipolar("Mix", 1.0),
        unused(),
    ]
}

pub fn init_defaults(_p: &mut [SlotValue; 12]) {}

pub fn migrate(rev: i32, p: &mut [SlotValue; 12]) {
    if rev <= 12 {
        p[10].v = 1.0;
    }
    if rev <= 15 {
        for i in [0, 3, 6] {
            p[i].deactivated = false;
        }
    }
}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    -1
}

pub struct Eq {
    sr: f32,
    band: [Biquad; 3],
    gain: Line,
    mix: Line,
    bi: u32,
}

impl Eq {
    pub fn new(sr: f32) -> Eq {
        Eq { sr, band: [Biquad::new(); 3], gain: Line::new(), mix: Line::new(), bi: 0 }
    }

    pub fn reset(&mut self) {
        for b in &mut self.band {
            b.suspend();
        }
        self.bi = 0;
    }

    fn control(&mut self, p: &[SlotValue; 12]) {
        if self.bi == 0 {
            for i in 0..3 {
                let g = p[3 * i].v;
                let f = p[3 * i + 1].v;
                let bw = p[3 * i + 2].v;
                self.band[i].peak_eq(calc_omega(f / 12.0, self.sr), bw as f64, g as f64);
            }
        }
        self.bi = (self.bi + 1) & 7;
    }

    pub fn process(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        self.control(p);
        let mut wl = *l;
        let mut wr = *r;
        for i in 0..3 {
            if !p[3 * i].deactivated {
                self.band[i].process(&mut wl, &mut wr);
            }
        }
        self.gain.set_target_smoothed(db_to_linear(p[9].v));
        self.gain.multiply_2_blocks(&mut wl, &mut wr);
        self.mix.set_target_smoothed(p[10].v.clamp(-1.0, 1.0));
        self.mix.fade_2_blocks(l, r, &wl, &wr);
    }

    pub fn process_only_control(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx) {
        self.control(p);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::kit::fxs::{default_values, FxCtx};

    /// Amplitude of the `hz` component of `x` (an integer number of periods).
    pub fn tone_amp(x: &[f32], hz: f64, sr: f64) -> f64 {
        let (mut c, mut s) = (0.0f64, 0.0f64);
        for (n, v) in x.iter().enumerate() {
            let ph = std::f64::consts::TAU * hz * n as f64 / sr;
            c += *v as f64 * ph.cos();
            s += *v as f64 * ph.sin();
        }
        2.0 * (c * c + s * s).sqrt() / x.len() as f64
    }

    /// Drive `f` with a stereo sine for `secs`, return the output amplitude
    /// over the last `period * cycles` samples.
    pub fn sine_gain(mut f: impl FnMut(&mut [f32; 32], &mut [f32; 32]), hz: f64, amp: f32, secs: f64) -> f64 {
        let sr = 48000.0;
        let blocks = (secs * sr / 32.0) as usize;
        let mut out = Vec::new();
        for b in 0..blocks {
            let mut l = [0f32; 32];
            for (k, s) in l.iter_mut().enumerate() {
                let n = (b * 32 + k) as f64;
                *s = amp * (std::f64::consts::TAU * hz * n / sr).sin() as f32;
            }
            let mut r = l;
            f(&mut l, &mut r);
            out.extend_from_slice(&l);
        }
        let period = sr / hz;
        let cycles = ((0.25 * sr) / period).floor().max(1.0);
        let n = (period * cycles).round() as usize;
        tone_amp(&out[out.len() - n..], hz, sr) / amp as f64
    }

    #[test]
    fn magnitude_matches_recipe() {
        let sr = 48000.0f32;
        let mut p = default_values(6);
        p[0].v = 6.0;
        p[3].v = -4.0;
        p[6].v = 9.0;
        p[8].v = 1.0;
        p[9].v = 2.0;
        let mut refs = [Biquad::new(); 3];
        for i in 0..3 {
            refs[i].peak_eq(calc_omega(p[3 * i + 1].v / 12.0, sr), p[3 * i + 2].v as f64, p[3 * i].v as f64);
        }
        for hz in [100.0f64, 600.0, 3000.0, 9600.0] {
            let mut fx = Eq::new(sr);
            let ctx = FxCtx::new(sr);
            let g = sine_gain(|l, r| fx.process(&p, &ctx, l, r), hz, 0.25, 1.0);
            let w = std::f64::consts::TAU * hz / sr as f64;
            let want = refs.iter().map(|b| b.response(w)).product::<f64>() * db_to_linear(2.0) as f64;
            let err = 20.0 * (g / want).log10();
            assert!(err.abs() < 0.1, "{hz} Hz: {g} vs {want} ({err} dB)");
        }
    }

    #[test]
    fn negative_mix_subtracts() {
        let mut p = default_values(6);
        for i in [0, 3, 6] {
            p[i].deactivated = true;
        }
        p[9].v = -6.0;
        p[10].v = -1.0;
        let mut fx = Eq::new(48000.0);
        let ctx = FxCtx::new(48000.0);
        let g = sine_gain(|l, r| fx.process(&p, &ctx, l, r), 600.0, 0.25, 0.3);
        let want = 2.0 - db_to_linear(-6.0) as f64;
        assert!((g - want).abs() < 1e-3, "{g} {want}");
    }
}
