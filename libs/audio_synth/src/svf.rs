//! A state-variable filter with a trapezoidal integrator: low, band and
//! high out of one pass, and stable when the cutoff moves every frame.
//!
//! Why this shape and not a biquad. The units above sweep their cutoff
//! continuously -- a wah follows an envelope, an auto-wah follows an
//! LFO -- and a biquad's direct-form memory is in terms of past INPUTS
//! and OUTPUTS, which belong to the coefficients that made them. Swap
//! those coefficients under it and the memory no longer means what the
//! difference equation says it means; the result is a thump at best and
//! a blow-up at worst. This filter's memory is the two integrator states
//! instead, which are voltages inside the circuit rather than a record of
//! the arithmetic, so the cutoff can move as fast as the caller likes and
//! the filter simply follows.
//!
//! The coefficients are split from the state so a caller sweeping a
//! cutoff builds one set per frame and runs several filters through it,
//! and a caller holding a cutoff builds one set and keeps it.

/// The frequency-dependent part of a state-variable filter: everything
/// that depends on the cutoff, the resonance and the device rate, and
/// nothing that depends on the signal.
#[derive(Clone, Copy)]
pub struct SvfCoeffs {
    /// The bilinear warp of the cutoff, `tan(pi*fc/fs)`. Kept because it
    /// IS the cutoff in this filter's own terms -- what anything that
    /// wants to reason about the corner rather than run the filter reads.
    g: f32,
    /// Damping: one over the resonance. It is also the normaliser on the
    /// band output below.
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
}

impl SvfCoeffs {
    /// The coefficients for a cutoff and a resonance at a device rate.
    ///
    /// The cutoff is held inside 10 Hz and 45% of the rate: the warp
    /// runs to infinity at half the rate, and a cutoff swept by an
    /// envelope has no reason to respect that on its own. The resonance
    /// is held at or above 0.1, which is the flattest this filter is ever
    /// asked to be, and keeps `k` out of the range where the damping term
    /// swamps everything else.
    pub fn new(cutoff_hz: f32, q: f32, device_rate: f32) -> SvfCoeffs {
        let rate = if device_rate.is_finite() { device_rate.max(1.0) } else { 48_000.0 };
        let top = 0.45 * rate;
        // A cutoff that is not a number lands at the bottom of the range
        // rather than anywhere near the warp's pole.
        let cutoff = if cutoff_hz.is_finite() { cutoff_hz.clamp(10.0f32.min(top), top) } else { 10.0f32.min(top) };
        let q = if q.is_finite() { q.max(0.1) } else { 0.1 };
        let g = (std::f32::consts::PI * cutoff / rate).tan();
        let k = 1.0 / q;
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        SvfCoeffs { g, k, a1, a2, a3 }
    }

    /// The warped cutoff these coefficients were built at.
    pub fn g(&self) -> f32 {
        self.g
    }
}

/// The two integrator states: the whole of what a state-variable filter
/// remembers. Small enough to copy, so a unit that needs one per channel
/// keeps an array of them and not a channel index.
#[derive(Clone, Copy, Default)]
pub struct Svf {
    ic1eq: f32,
    ic2eq: f32,
}

impl Svf {
    /// One frame through the filter: `(low, band, high)`.
    ///
    /// `band` is normalised -- the raw band output of this topology peaks
    /// at the resonance, so a wah with its resonance up would get louder
    /// as well as more coloured. Scaling by `k` (one over the resonance)
    /// leaves a tone at the cutoff passing at unity whatever the
    /// resonance is, which is what a caller mixing a band output against
    /// a dry signal needs. `high` is the same scaled band taken out of
    /// the input alongside the low, so the three still sum back to it.
    #[inline]
    pub fn process(&mut self, c: &SvfCoeffs, x: f32) -> (f32, f32, f32) {
        let v3 = x - self.ic2eq;
        let v1 = c.a1 * self.ic1eq + c.a2 * v3;
        let v2 = self.ic2eq + c.a2 * self.ic1eq + c.a3 * v3;
        self.ic1eq = 2.0 * v1 - self.ic1eq;
        self.ic2eq = 2.0 * v2 - self.ic2eq;
        let band = c.k * v1;
        (v2, band, x - band - v2)
    }

    pub fn reset(&mut self) {
        self.ic1eq = 0.0;
        self.ic2eq = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms_ratio(mut run: impl FnMut(f32) -> f32, hz: f32, rate: f32) -> f32 {
        let mut num = 0.0;
        let mut den = 0.0;
        for n in 0..(rate as usize) {
            let x = (n as f32 * std::f32::consts::TAU * hz / rate).sin();
            let y = run(x);
            if n > 4_000 {
                num += y * y;
                den += x * x;
            }
        }
        (num / den).sqrt()
    }

    #[test]
    fn the_svf_band_output_peaks_at_unity_at_its_centre() {
        let rate = 48_000.0;
        let c = SvfCoeffs::new(1_000.0, 4.0, rate);
        let mut s = Svf::default();
        let at_centre = rms_ratio(|x| s.process(&c, x).1, 1_000.0, rate);
        assert!((at_centre - 1.0).abs() < 0.02, "{at_centre}");
        let mut s = Svf::default();
        let two_octaves_up = rms_ratio(|x| s.process(&c, x).1, 4_000.0, rate);
        assert!(two_octaves_up < 0.3, "{two_octaves_up}");
    }

    #[test]
    fn low_and_high_outputs_split_the_spectrum() {
        let rate = 48_000.0;
        let c = SvfCoeffs::new(1_000.0, std::f32::consts::FRAC_1_SQRT_2, rate);
        let mut s = Svf::default();
        assert!(rms_ratio(|x| s.process(&c, x).0, 100.0, rate) > 0.98);
        let mut s = Svf::default();
        assert!(rms_ratio(|x| s.process(&c, x).0, 10_000.0, rate) < 0.02);
        let mut s = Svf::default();
        assert!(rms_ratio(|x| s.process(&c, x).2, 10_000.0, rate) > 0.98);
    }

    #[test]
    fn a_cutoff_swept_every_sample_stays_bounded() {
        let rate = 48_000.0;
        let mut s = Svf::default();
        let mut worst = 0.0f32;
        for n in 0..96_000 {
            let hz = 200.0 + 8_000.0 * (0.5 + 0.5 * (n as f32 * 0.001).sin());
            let c = SvfCoeffs::new(hz, 10.0, rate);
            let x = (n as f32 * 0.09).sin() * 0.5;
            let (l, b, h) = s.process(&c, x);
            for v in [l, b, h] {
                assert!(v.is_finite());
                worst = worst.max(v.abs());
            }
        }
        assert!(worst < 8.0, "{worst}");
    }

    #[test]
    fn the_cutoff_and_the_resonance_are_held_inside_what_the_warp_can_carry() {
        let rate = 48_000.0;
        for (hz, q) in [
            (f32::NAN, 1.0),
            (1.0e12, 1.0),
            (-100.0, 1.0),
            (1_000.0, f32::NAN),
            (1_000.0, 0.0),
            (1_000.0, -4.0),
        ] {
            let c = SvfCoeffs::new(hz, q, rate);
            assert!(c.g().is_finite() && c.g() > 0.0, "{hz} Hz at q {q}");
            let mut s = Svf::default();
            let (l, b, h) = s.process(&c, 1.0);
            assert!(l.is_finite() && b.is_finite() && h.is_finite());
            s.reset();
            assert_eq!(s.process(&c, 0.0), (0.0, 0.0, 0.0));
        }
    }
}
