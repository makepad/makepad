//! The analog family: four circuit models of classic analog synth chips,
//! built from what their datasheets publish (`CREDITS.md` of this crate
//! names the sheets). Each is a kit component a layout composes like the
//! rest; in Morph they are `a_mode: @analog`, `f1_type: @analog_lp`,
//! `env1_model: @analog` and `amp_type: @analog`.
//!
//! - [`vco`]: exponential core; triangle, saw and pulse from one integrator;
//!   the sheet's three kinds of sync; linear FM.
//! - [`vcf`]: four cascaded gain cells with a transconductance resonance
//!   cell, solved without a unit delay.
//! - [`vca`]: two gain cells, exponential and linear law, class B..A.
//! - [`eg`]: true RC segments with independent gate and trigger.
//!
//! Every component takes `drift`: 0 is the ideal circuit, 0.5 a part at the
//! sheet's typical figures, 1 at its worst guaranteed figures ([`tol`]).
//!
//! Nothing in a render path allocates, locks or does I/O.

pub mod blep;
pub mod eg;
pub mod vca;
pub mod vcf;
pub mod vco;

#[cfg(test)]
mod tests;

use crate::dsp::Rng;

/// A figure at `drift`: `ideal` at 0, `typ` at 0.5, `max` at 1.
#[inline]
pub fn tol(drift: f32, ideal: f32, typ: f32, max: f32) -> f32 {
    let d = drift.clamp(0.0, 1.0) * 2.0;
    if d <= 1.0 { ideal + (typ - ideal) * d } else { typ + (max - typ) * (d - 1.0) }
}

/// A part's own deviation -1..1 for characteristic `i`: fixed per seed, the
/// way one chip's errors are fixed once it is made.
#[inline]
pub fn part(seed: u32, i: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9e37_79b1) ^ i.wrapping_mul(0x85eb_ca77);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    (x >> 8) as f32 / 8_388_608.0 - 1.0
}

/// A slow temperature wander in degrees from where the part was trimmed:
/// what a tempco figure acts on.
#[derive(Clone, Copy, Debug)]
pub struct Thermal {
    celsius: f32,
    rng: Rng,
}

/// The wander's spread (degrees, rms) and time constant (seconds).
const THERMAL_RMS: f32 = 3.0;
const THERMAL_TAU: f32 = 60.0;

impl Thermal {
    pub fn new(seed: u32) -> Thermal {
        let mut rng = Rng::new(seed.wrapping_mul(0x2545_f491) | 1);
        let celsius = rng.bipolar() * THERMAL_RMS;
        Thermal { celsius, rng }
    }

    /// Move on `dt` seconds; the temperature offset.
    #[inline]
    pub fn step(&mut self, dt: f32) -> f32 {
        let a = dt / THERMAL_TAU;
        self.celsius += -self.celsius * a + self.rng.bipolar() * THERMAL_RMS * 1.73 * (2.0 * a).sqrt();
        self.celsius
    }
}
