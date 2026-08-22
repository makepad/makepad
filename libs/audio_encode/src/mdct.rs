//! Forward MDCT: `n` windowed time samples in, `n/2` spectral coefficients
//! out, scaled so the sibling decoder's unnormalised IMDCT plus overlap-add
//! reconstructs the input exactly (the `4/n` factor its round-trip test pins).
//!
//! Implemented as the classic TDAC fold to an `N`-point DCT-IV, computed
//! through an `N/2`-point complex FFT — the same shape as the decoder's
//! inverse, written forward. Like the decoder, the tests carry the textbook
//! O(N^2) definition as the oracle; unlike the decoder this runs in `f32`
//! (encode is bulk work and the transform noise sits ~120 dB under the
//! signal, far below the quantizer).

use std::f64::consts::PI;

pub struct MdctFwd {
    /// Time-domain block length `n`.
    n: usize,
    /// FFT length, `n/4`.
    m: usize,
    bitrev: Vec<u32>,
    /// Per-stage twiddles, contiguous per stage: for stage length `len`
    /// (8, 16, ... m), `half = len/2` factors `e^{-2 pi i k/len}`, `k < half`.
    /// Contiguous storage lets the butterfly loop read them without strided
    /// gathers, which is what keeps the compiler's vectoriser engaged.
    stage_re: Vec<Vec<f32>>,
    stage_im: Vec<Vec<f32>>,
    /// DCT-IV pre/post rotations, `m` each.
    pre_re: Vec<f32>,
    pre_im: Vec<f32>,
    post_re: Vec<f32>,
    post_im: Vec<f32>,
    /// Fold buffer, `n/2` long.
    fold: Vec<f32>,
    /// FFT working buffers.
    re: Vec<f32>,
    im: Vec<f32>,
}

impl MdctFwd {
    /// `n` must be a power of two, at least 16.
    pub fn new(n: usize) -> MdctFwd {
        assert!(n.is_power_of_two() && n >= 16, "mdct block size {n}");
        let half = n / 2;
        let m = n / 4;
        let bits = m.trailing_zeros();
        let mut bitrev = vec![0u32; m];
        for (i, slot) in bitrev.iter_mut().enumerate() {
            let mut v = 0usize;
            for b in 0..bits {
                if i & (1 << b) != 0 {
                    v |= 1 << (bits - 1 - b);
                }
            }
            *slot = v as u32;
        }
        let mut stage_re: Vec<Vec<f32>> = Vec::new();
        let mut stage_im: Vec<Vec<f32>> = Vec::new();
        let mut len = 8usize;
        while len <= m {
            let half = len / 2;
            let mut re = Vec::with_capacity(half);
            let mut im = Vec::with_capacity(half);
            for k in 0..half {
                let ang = -2.0 * PI * k as f64 / len as f64;
                re.push(ang.cos() as f32);
                im.push(ang.sin() as f32);
            }
            stage_re.push(re);
            stage_im.push(im);
            len <<= 1;
        }
        let mut pre_re = vec![0.0f32; m];
        let mut pre_im = vec![0.0f32; m];
        let mut post_re = vec![0.0f32; m];
        let mut post_im = vec![0.0f32; m];
        for j in 0..m {
            let ang = -PI * (4.0 * j as f64 + 1.0) / (4.0 * half as f64);
            pre_re[j] = ang.cos() as f32;
            pre_im[j] = ang.sin() as f32;
            let ang = -PI * j as f64 / half as f64;
            post_re[j] = ang.cos() as f32;
            post_im[j] = ang.sin() as f32;
        }
        MdctFwd {
            n,
            m,
            bitrev,
            stage_re,
            stage_im,
            pre_re,
            pre_im,
            post_re,
            post_im,
            fold: vec![0.0; half],
            re: vec![0.0; m],
            im: vec![0.0; m],
        }
    }

    pub fn block_size(&self) -> usize {
        self.n
    }

    /// Forward MDCT of `time` (already windowed, `n` samples) into `spectrum`
    /// (`n/2` values), including the `4/n` TDAC normalisation the decoder's
    /// unnormalised inverse expects.
    pub fn mdct(&mut self, time: &[f32], spectrum: &mut [f32]) {
        let n = self.n;
        let half = n / 2;
        assert!(time.len() >= n && spectrum.len() >= half);

        // TDAC fold of 2N samples onto N: with y = windowed input,
        //   u[j]        = -y[3N/2 - 1 - j] - y[3N/2 + j]   j in [0, N/2)
        //   u[N/2 + j]  =  y[j]            - y[N - 1 - j]   j in [0, N/2)
        // verified against the O(N^2) definition in the tests below.
        let quarter = half / 2;
        let (lo, hi) = self.fold.split_at_mut(quarter);
        for (j, slot) in lo.iter_mut().enumerate() {
            *slot = -time[half + quarter - 1 - j] - time[half + quarter + j];
        }
        for (j, slot) in hi.iter_mut().enumerate() {
            *slot = time[j] - time[half - 1 - j];
        }

        // DCT-IV via the half-length complex FFT.
        let m = self.m;
        for j in 0..m {
            let a = self.fold[2 * j];
            let b = self.fold[half - 1 - 2 * j];
            let (c, s) = (self.pre_re[j], self.pre_im[j]);
            self.re[j] = a * c - b * s;
            self.im[j] = a * s + b * c;
        }
        self.fft();
        let scale = 4.0f32 / n as f32;
        for j in 0..m {
            let (c, s) = (self.post_re[j], self.post_im[j]);
            let tr = self.re[j] * c - self.im[j] * s;
            let ti = self.re[j] * s + self.im[j] * c;
            spectrum[2 * j] = tr * scale;
            spectrum[half - 1 - 2 * j] = -ti * scale;
        }
    }

    /// Iterative radix-2 DIT FFT over the scratch buffers, identical shape to
    /// the decoder's (but `f32`). The first two stages use fixed twiddles so
    /// the compiler can vectorise them without gather loads.
    fn fft(&mut self) {
        let m = self.m;
        let re = &mut self.re;
        let im = &mut self.im;
        for i in 0..m {
            let j = self.bitrev[i] as usize;
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        // len == 2: twiddle is 1.
        let mut i = 0;
        while i < m {
            let (ur, ui) = (re[i], im[i]);
            let (vr, vi) = (re[i + 1], im[i + 1]);
            re[i] = ur + vr;
            im[i] = ui + vi;
            re[i + 1] = ur - vr;
            im[i + 1] = ui - vi;
            i += 2;
        }
        if m >= 4 {
            // len == 4: twiddles are 1 and -i.
            let mut i = 0;
            while i < m {
                let (ur, ui) = (re[i], im[i]);
                let (vr, vi) = (re[i + 2], im[i + 2]);
                re[i] = ur + vr;
                im[i] = ui + vi;
                re[i + 2] = ur - vr;
                im[i + 2] = ui - vi;
                let (ur, ui) = (re[i + 1], im[i + 1]);
                // (vr0 + i vi0) * (-i) = vi0 - i vr0
                let (vr0, vi0) = (re[i + 3], im[i + 3]);
                let (vr, vi) = (vi0, -vr0);
                re[i + 1] = ur + vr;
                im[i + 1] = ui + vi;
                re[i + 3] = ur - vr;
                im[i + 3] = ui - vi;
                i += 4;
            }
        }
        let mut len = 8usize;
        let mut stage = 0usize;
        while len <= m {
            let half = len / 2;
            let tw_re = &self.stage_re[stage][..half];
            let tw_im = &self.stage_im[stage][..half];
            let mut base = 0usize;
            while base < m {
                let (lo, hi) = re[base..base + len].split_at_mut(half);
                let (lo_i, hi_i) = im[base..base + len].split_at_mut(half);
                butterfly_span(lo, lo_i, hi, hi_i, tw_re, tw_im);
                base += len;
            }
            len <<= 1;
            stage += 1;
        }
    }
}

/// One span of radix-2 butterflies: `lo/hi` are the two halves of a stage
/// block, `tw` the per-stage twiddles. Dispatches to NEON on aarch64; the
/// scalar body is the reference twin and the fallback everywhere else.
///
/// The NEON path uses only mul/add/sub (no fused multiply-add), so its
/// results are bit-identical to the scalar twin — the test below asserts
/// exact equality, not a tolerance.
#[inline]
fn butterfly_span(
    lo: &mut [f32],
    lo_i: &mut [f32],
    hi: &mut [f32],
    hi_i: &mut [f32],
    tw_re: &[f32],
    tw_im: &[f32],
) {
    let half = lo.len();
    debug_assert!(
        lo_i.len() == half && hi.len() == half && hi_i.len() == half,
        "butterfly halves must agree"
    );
    debug_assert!(tw_re.len() >= half && tw_im.len() >= half);
    #[cfg(target_arch = "aarch64")]
    {
        // Whole 4-lane groups only: FFT stage halves are always powers of
        // two so this is every real call, but the guard (not just a
        // debug_assert) is what makes an odd length safe rather than an
        // out-of-bounds vector load.
        if half >= 4 && half % 4 == 0 {
            // SAFETY: all six slices hold at least `half` f32s (asserted
            // above); `k + 4 <= half` inside the loop, so every 128-bit
            // load/store is in bounds. Lanewise fmul/fadd/fsub round
            // identically to scalar f32 arithmetic.
            unsafe {
                use std::arch::aarch64::*;
                let mut k = 0usize;
                while k < half {
                    let cr = vld1q_f32(tw_re.as_ptr().add(k));
                    let ci = vld1q_f32(tw_im.as_ptr().add(k));
                    let ur = vld1q_f32(lo.as_ptr().add(k));
                    let ui = vld1q_f32(lo_i.as_ptr().add(k));
                    let vr0 = vld1q_f32(hi.as_ptr().add(k));
                    let vi0 = vld1q_f32(hi_i.as_ptr().add(k));
                    let vr = vsubq_f32(vmulq_f32(vr0, cr), vmulq_f32(vi0, ci));
                    let vi = vaddq_f32(vmulq_f32(vr0, ci), vmulq_f32(vi0, cr));
                    vst1q_f32(lo.as_mut_ptr().add(k), vaddq_f32(ur, vr));
                    vst1q_f32(lo_i.as_mut_ptr().add(k), vaddq_f32(ui, vi));
                    vst1q_f32(hi.as_mut_ptr().add(k), vsubq_f32(ur, vr));
                    vst1q_f32(hi_i.as_mut_ptr().add(k), vsubq_f32(ui, vi));
                    k += 4;
                }
            }
            return;
        }
    }
    butterfly_span_scalar(lo, lo_i, hi, hi_i, tw_re, tw_im);
}

/// The scalar twin of [`butterfly_span`], also the non-aarch64 path.
fn butterfly_span_scalar(
    lo: &mut [f32],
    lo_i: &mut [f32],
    hi: &mut [f32],
    hi_i: &mut [f32],
    tw_re: &[f32],
    tw_im: &[f32],
) {
    for k in 0..lo.len() {
        let (cr, ci) = (tw_re[k], tw_im[k]);
        let (ur, ui) = (lo[k], lo_i[k]);
        let (vr0, vi0) = (hi[k], hi_i[k]);
        let vr = vr0 * cr - vi0 * ci;
        let vi = vr0 * ci + vi0 * cr;
        lo[k] = ur + vr;
        lo_i[k] = ui + vi;
        hi[k] = ur - vr;
        hi_i[k] = ui - vi;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_audio_decode::vorbis::mdct::{window, Mdct};

    fn rand_vec(n: usize, seed: u64) -> Vec<f32> {
        let mut s = seed | 1;
        (0..n)
            .map(|_| {
                s ^= s >> 12;
                s ^= s << 25;
                s ^= s >> 27;
                let v = s.wrapping_mul(0x2545F491_4F6CDD1D);
                ((v >> 40) as f32 / 16_777_216.0) * 2.0 - 1.0
            })
            .collect()
    }

    /// Forward MDCT straight from the definition, with the 4/n factor.
    fn mdct_direct(time: &[f32]) -> Vec<f32> {
        let n = time.len();
        let n2 = n / 2;
        (0..n2)
            .map(|k| {
                let mut acc = 0.0f64;
                for (i, &x) in time.iter().enumerate() {
                    let ang =
                        PI / n2 as f64 * (i as f64 + 0.5 + n2 as f64 / 2.0) * (k as f64 + 0.5);
                    acc += x as f64 * ang.cos();
                }
                (acc * 4.0 / n as f64) as f32
            })
            .collect()
    }

    #[test]
    fn fast_mdct_matches_the_direct_definition() {
        for &n in &[16usize, 64, 256, 1024, 2048] {
            let x = rand_vec(n, n as u64 * 3 + 1);
            let mut m = MdctFwd::new(n);
            let mut got = vec![0f32; n / 2];
            m.mdct(&x, &mut got);
            let want = mdct_direct(&x);
            for k in 0..n / 2 {
                assert!(
                    (got[k] - want[k]).abs() < 2e-4,
                    "n={n} k={k}: fast {} vs direct {}",
                    got[k],
                    want[k]
                );
            }
        }
    }

    #[test]
    fn forward_then_decoder_inverse_reconstructs_through_overlap_add() {
        // The property the whole codec rests on: window, forward (ours),
        // inverse (the decoder's), window, overlap-add — the middle of the
        // signal comes back exactly.
        let n = 1024usize;
        let h = n / 2;
        let w = window(n);
        let signal = rand_vec(n * 4, 99);
        let mut fwd = MdctFwd::new(n);
        let mut inv = Mdct::new(n);
        let mut out = vec![0f32; n * 4];
        let mut windowed = vec![0f32; n];
        let mut spec = vec![0f32; h];
        let mut block = vec![0f32; n];
        for start in (0..=n * 3).step_by(h) {
            for i in 0..n {
                windowed[i] = signal[start + i] * w[i];
            }
            fwd.mdct(&windowed, &mut spec);
            inv.imdct(&spec, &mut block);
            for i in 0..n {
                out[start + i] += block[i] * w[i];
            }
        }
        for i in h..n * 3 {
            assert!(
                (out[i] - signal[i]).abs() < 1e-3,
                "i={i}: {} vs {}",
                out[i],
                signal[i]
            );
        }
    }

    #[test]
    fn a_pure_tone_lands_in_one_bin_pair() {
        let n = 1024usize;
        let n2 = n / 2;
        let w = window(n);
        // Bin-centred cosine at k=40 (frequency (k+0.5) bins).
        let k0 = 40usize;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let ang = PI / n2 as f64 * (i as f64 + 0.5 + n2 as f64 / 2.0) * (k0 as f64 + 0.5);
                (ang.cos() * 0.7) as f32 * w[i]
            })
            .collect();
        let mut m = MdctFwd::new(n);
        let mut spec = vec![0f32; n2];
        m.mdct(&x, &mut spec);
        let peak = spec[k0].abs();
        assert!(peak > 0.3, "peak {peak}");
        for (k, &v) in spec.iter().enumerate() {
            if (k as isize - k0 as isize).abs() > 4 {
                assert!(v.abs() < peak * 0.05, "leak at {k}: {v} vs {peak}");
            }
        }
    }
}

#[cfg(test)]
mod simd_tests {
    use super::*;

    #[test]
    fn neon_butterflies_match_the_scalar_twin_exactly() {
        let mut s = 0x9e3779b97f4a7c15u64;
        let mut rng = move || {
            s ^= s >> 12;
            s ^= s << 25;
            s ^= s >> 27;
            ((s.wrapping_mul(0x2545F4914F6CDD1D) >> 40) as f32 / 16_777_216.0) * 2.0 - 1.0
        };
        for half in [4usize, 8, 32, 128, 7, 3, 1] {
            let tw_re: Vec<f32> = (0..half).map(|_| rng()).collect();
            let tw_im: Vec<f32> = (0..half).map(|_| rng()).collect();
            let mut a: Vec<Vec<f32>> =
                (0..4).map(|_| (0..half).map(|_| rng()).collect()).collect();
            let mut b = a.clone();
            {
                let (lo, rest) = a.split_at_mut(1);
                let (lo_i, rest) = rest.split_at_mut(1);
                let (hi, hi_i) = rest.split_at_mut(1);
                butterfly_span(&mut lo[0], &mut lo_i[0], &mut hi[0], &mut hi_i[0], &tw_re, &tw_im);
            }
            {
                let (lo, rest) = b.split_at_mut(1);
                let (lo_i, rest) = rest.split_at_mut(1);
                let (hi, hi_i) = rest.split_at_mut(1);
                butterfly_span_scalar(
                    &mut lo[0], &mut lo_i[0], &mut hi[0], &mut hi_i[0], &tw_re, &tw_im,
                );
            }
            // Bit-exact: the SIMD path uses no fused operations.
            assert_eq!(a, b, "half={half}");
        }
    }
}
