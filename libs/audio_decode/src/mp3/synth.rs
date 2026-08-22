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

/// `matrix()` transposed: `matrix_t[k][i] == matrix()[i][k]`.
///
/// The matrixing sums over `k` for each of 64 outputs. Written as it reads in
/// the standard — output `i` outermost — every output is one 32-long serial
/// dependency chain of FMAs and nothing vectorises. Written with `k` outermost
/// over an array of 64 accumulators, each accumulator still sees exactly the
/// same operands in exactly the same order (so the result is bit-identical),
/// but the 64 lanes are independent and contiguous, which is a shape LLVM
/// turns into 16 vector FMAs per `k`. That needs the coefficients laid out
/// with `i` contiguous, hence the transpose.
fn matrix_transposed() -> [[f32; 64]; 32] {
    let n = matrix();
    let mut t = [[0.0f32; 64]; 32];
    for (i, row) in n.iter().enumerate() {
        for (k, &v) in row.iter().enumerate() {
            t[k][i] = v;
        }
    }
    t
}

/// Backing store for the 1024-sample FIFO. The FIFO shifts down by 64 every
/// slot, which as a literal `copy_within` is a 3840-byte move per slot; here
/// the window slides through a buffer twice its length instead and only the
/// wrap costs a copy, once every 16 slots.
const FIFO_CAP: usize = 2048;
const FIFO_TOP: usize = FIFO_CAP - 1024;

pub struct Synthesis {
    /// Boxed: this is more than belongs on the stack, and a deck holds one of
    /// these per channel per deck.
    v: Box<[f32; FIFO_CAP]>,
    /// Physical index of logical `V[0]`; steps down by 64 per slot.
    off: usize,
    window: Box<[f32; 512]>,
    matrix_t: Box<[[f32; 64]; 32]>,
}

impl Synthesis {
    pub fn new() -> Self {
        Self {
            v: Box::new([0.0; FIFO_CAP]),
            off: FIFO_TOP,
            window: Box::new(synth_window()),
            matrix_t: Box::new(matrix_transposed()),
        }
    }

    /// Drop the history, so a seek does not bleed the previous position's
    /// tail into the new one.
    pub fn reset(&mut self) {
        self.v.fill(0.0);
        self.off = FIFO_TOP;
    }

    /// One time slot: 32 subband samples in, 32 PCM samples appended to `out`.
    pub fn slot(&mut self, subband: &[f32; 32], out: &mut [f32; 32]) {
        // Shift the FIFO down by one slot. Sliding the window down by 64 is
        // the shift: logical `V[64 + x]` after the step is the same memory as
        // logical `V[x]` before it. Only when the window reaches the bottom of
        // the buffer does anything move.
        if self.off < 64 {
            self.v.copy_within(self.off..self.off + 1024 - 64, FIFO_TOP + 64);
            self.off = FIFO_TOP + 64;
        }
        self.off -= 64;
        let off = self.off;
        // Matrix the new samples in: `V[i] = sum_k N[i][k] * S[k]`, summed in
        // ascending `k` exactly as the row-at-a-time form does.
        let mut head = [0.0f32; 64];
        for (k, row) in self.matrix_t.iter().enumerate() {
            let s = subband[k];
            for (acc, &c) in head.iter_mut().zip(row.iter()) {
                *acc += c * s;
            }
        }
        self.v[off..off + 64].copy_from_slice(&head);
        // Window and fold the 512 taps down to 32 samples. The intermediate
        // `U` vector the standard names is not built: its 512 entries are
        // exactly two contiguous 32-runs of `V` per half-slot, so the window
        // pass reads them straight out of the FIFO. Again the accumulation for
        // each output `j` runs over ascending `i`, as before.
        let mut acc = [0.0f32; 32];
        for i in 0..16 {
            let at = off + (i / 2) * 128 + if i & 1 == 1 { 96 } else { 0 };
            let taps = &self.v[at..at + 32];
            let w = &self.window[i * 32..i * 32 + 32];
            for ((a, &t), &wv) in acc.iter_mut().zip(taps.iter()).zip(w.iter()) {
                *a += t * wv;
            }
        }
        *out = acc;
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
