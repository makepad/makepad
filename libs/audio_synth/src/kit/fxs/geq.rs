//! Eleven-band graphic EQ (type 16): fixed-frequency half-octave peaking
//! bands (each can be deactivated) and an output gain; fully wet.

use super::common::*;
use super::FxCtx;
use crate::kit::core::{calc_omega_hz, db_to_linear, Biquad, SlotDef, SlotValue};

/// Band centre frequencies in Hz.
pub const BAND_HZ: [f32; 11] = [30.0, 60.0, 120.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 12000.0, 16000.0];

pub fn slot_defs() -> [SlotDef; 12] {
    let b = |n: &'static str| deact(db(n, -24.0, 24.0, 0.0));
    [
        b("30 Hz"),
        b("60 Hz"),
        b("120 Hz"),
        b("250 Hz"),
        b("500 Hz"),
        b("1 kHz"),
        b("2 kHz"),
        b("4 kHz"),
        b("8 kHz"),
        b("12 kHz"),
        b("16 kHz"),
        db("Gain", -48.0, 48.0, 0.0),
    ]
}

pub fn init_defaults(_p: &mut [SlotValue; 12]) {}

pub fn migrate(_rev: i32, _p: &mut [SlotValue; 12]) {}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    -1
}

pub struct GraphicEq {
    sr: f32,
    band: [Biquad; 11],
    gain: Line,
    bi: u32,
}

impl GraphicEq {
    pub fn new(sr: f32) -> GraphicEq {
        GraphicEq { sr, band: [Biquad::new(); 11], gain: Line::new(), bi: 0 }
    }

    pub fn reset(&mut self) {
        for b in &mut self.band {
            b.suspend();
        }
        self.bi = 0;
    }

    fn control(&mut self, p: &[SlotValue; 12]) {
        if self.bi == 0 {
            for i in 0..11 {
                self.band[i].peak_eq(calc_omega_hz(BAND_HZ[i], self.sr), 0.5, p[i].v as f64);
            }
        }
        self.bi = (self.bi + 1) & 7;
    }

    pub fn process(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        self.control(p);
        for i in 0..11 {
            if !p[i].deactivated {
                self.band[i].process(l, r);
            }
        }
        self.gain.set_target_smoothed(db_to_linear(p[11].v));
        self.gain.multiply_2_blocks(l, r);
    }

    pub fn process_only_control(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx) {
        self.control(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::fxs::eq::tests::sine_gain;
    use crate::kit::fxs::{default_values, FxCtx};

    #[test]
    fn magnitude_matches_recipe() {
        let sr = 48000.0f32;
        let mut p = default_values(16);
        let gains = [3.0f32, -6.0, 0.0, 12.0, -3.0, 6.0, -9.0, 4.0, 0.0, -12.0, 5.0];
        for i in 0..11 {
            p[i].v = gains[i];
        }
        p[2].deactivated = true;
        p[2].v = 10.0;
        p[11].v = -2.0;
        let mut refs = [Biquad::new(); 11];
        for i in 0..11 {
            refs[i].peak_eq(calc_omega_hz(BAND_HZ[i], sr), 0.5, p[i].v as f64);
        }
        for hz in [60.0f64, 250.0, 1000.0, 3000.0, 9600.0] {
            let mut fx = GraphicEq::new(sr);
            let ctx = FxCtx::new(sr);
            let g = sine_gain(|l, r| fx.process(&p, &ctx, l, r), hz, 0.1, 1.0);
            let w = std::f64::consts::TAU * hz / sr as f64;
            let want = (0..11).filter(|i| !p[*i].deactivated).map(|i| refs[i].response(w)).product::<f64>()
                * db_to_linear(-2.0) as f64;
            let err = 20.0 * (g / want).log10();
            assert!(err.abs() < 0.1, "{hz} Hz: {g} vs {want} ({err} dB)");
        }
    }
}
