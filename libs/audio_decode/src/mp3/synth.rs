//! The polyphase synthesis filterbank (ISO/IEC 11172-3 clause 2.4.3.2 and
//! Figure 3-A.2): 32 subband samples in, 32 PCM samples out, per time slot.
//!
//! One [`Synthesis`] per channel — it owns the 1024-sample FIFO `V` that
//! carries 16 slots of history, which is what makes the filterbank an
//! overlapping reconstruction rather than a per-slot transform. Everything is
//! preallocated: the per-frame loop touches no allocator.

use super::tables::synth_window;

/// Matrixing coefficients `N[i][k] = cos((16 + i)(2k + 1) pi / 64)`.
fn matrix() -> [[f32; 32]; 64] {
    let mut n = [[0.0f32; 32]; 64];
    for (i, row) in n.iter_mut().enumerate() {
        for (k, slot) in row.iter_mut().enumerate() {
            let angle = (16 + i) as f64 * (2 * k + 1) as f64 * std::f64::consts::PI / 64.0;
            *slot = angle.cos() as f32;
        }
    }
    n
}

pub struct Synthesis {
    /// Boxed: 1024 + 512 + 8192 floats is more than belongs on the stack, and
    /// a deck holds one of these per channel per deck.
    v: Box<[f32; 1024]>,
    u: Box<[f32; 512]>,
    window: Box<[f32; 512]>,
    matrix: Box<[[f32; 32]; 64]>,
}

impl Synthesis {
    pub fn new() -> Self {
        Self {
            v: Box::new([0.0; 1024]),
            u: Box::new([0.0; 512]),
            window: Box::new(synth_window()),
            matrix: Box::new(matrix()),
        }
    }

    /// Drop the history, so a seek does not bleed the previous position's
    /// tail into the new one.
    pub fn reset(&mut self) {
        self.v.fill(0.0);
    }

    /// One time slot: 32 subband samples in, 32 PCM samples appended to `out`.
    pub fn slot(&mut self, subband: &[f32; 32], out: &mut [f32; 32]) {
        // Shift the FIFO down by one slot and matrix the new samples in.
        self.v.copy_within(0..1024 - 64, 64);
        for i in 0..64 {
            let row = &self.matrix[i];
            let mut acc = 0.0f32;
            for k in 0..32 {
                acc += row[k] * subband[k];
            }
            self.v[i] = acc;
        }
        // Build U by taking alternating half-slots out of the FIFO.
        for i in 0..8 {
            for j in 0..32 {
                self.u[i * 64 + j] = self.v[i * 128 + j];
                self.u[i * 64 + 32 + j] = self.v[i * 128 + 96 + j];
            }
        }
        // Window and fold the 512 taps down to 32 samples.
        for (j, slot) in out.iter_mut().enumerate() {
            let mut acc = 0.0f32;
            for i in 0..16 {
                let at = j + 32 * i;
                acc += self.u[at] * self.window[at];
            }
            *slot = acc;
        }
    }
}

impl Default for Synthesis {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_matches_its_definition() {
        let n = matrix();
        assert!((n[0][0] - (16.0 * std::f64::consts::PI / 64.0).cos() as f32).abs() < 1e-6);
        // Row 16 is cos((32)(2k+1)pi/64) = cos((2k+1)pi/2) = 0.
        for k in 0..32 {
            assert!(n[16][k].abs() < 1e-6, "k={k}");
        }
    }

    #[test]
    fn window_is_the_tabulated_one() {
        let w = synth_window();
        assert_eq!(w[0], 0.0);
        assert!((w[1] + 1.0 / 65536.0).abs() < 1e-12);
        // Table 3-B.3 is antisymmetric about its centre, D[512 - i] == -D[i],
        // except at the block boundaries where the tabulated sign flips: those
        // three entries mirror without negating. Asserting the exception as
        // well as the rule is what makes this a transcription check.
        for i in 1..256 {
            let mirrored = w[512 - i];
            if i % 64 == 0 {
                assert!((mirrored - w[i]).abs() < 1e-9, "i={i}");
            } else {
                assert!((mirrored + w[i]).abs() < 1e-9, "i={i}");
            }
        }
        // Spot values straight out of the standard's table.
        assert!((w[16] - (-0.000_076_294)).abs() < 1e-9);
        assert!((w[64] - (213.0 / 65536.0)).abs() < 1e-9);
    }

    #[test]
    fn silence_in_is_silence_out() {
        let mut s = Synthesis::new();
        let mut out = [0.0f32; 32];
        for _ in 0..40 {
            s.slot(&[0.0; 32], &mut out);
            assert!(out.iter().all(|v| *v == 0.0));
        }
    }

    #[test]
    fn a_dc_subband_impulse_produces_a_bounded_response() {
        // Subband 0 held at 1.0 is the lowest-frequency basis function; the
        // filterbank must stay bounded and eventually settle.
        let mut s = Synthesis::new();
        let mut sb = [0.0f32; 32];
        sb[0] = 1.0;
        let mut out = [0.0f32; 32];
        let mut peak = 0.0f32;
        for _ in 0..64 {
            s.slot(&sb, &mut out);
            for v in out {
                assert!(v.is_finite());
                peak = peak.max(v.abs());
            }
        }
        assert!(peak > 0.0 && peak < 4.0, "peak {peak}");
        s.reset();
        s.slot(&[0.0; 32], &mut out);
        assert!(out.iter().all(|v| *v == 0.0));
    }
}
