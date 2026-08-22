//! IMDCT and the Vorbis window.
//!
//! Adapted from the same author's sandbox decoder, reshaped so a block costs no
//! allocations: the twiddles, the bit-reversal permutation and the working
//! buffers are built once per block size and reused for the life of the stream.
//!
//! The transform is the one place where a subtle sign or twiddle error produces
//! plausible-looking garbage rather than an obvious failure, so the tests carry
//! a second implementation — the textbook O(N^2) definition — and assert the
//! fast one agrees with it. The slow one is the oracle for the fast one.

use std::f64::consts::PI;

/// One block size's inverse MDCT: `n/2` spectral coefficients in, `n` time
/// samples out, via a DCT-IV built on a complex FFT of length `n/4`.
pub struct Mdct {
    /// Time-domain length.
    n: usize,
    /// FFT length, `n/4`.
    m: usize,
    bitrev: Vec<u32>,
    /// `e^{-2*pi*i*k/m}` for `k < m/2`, shared by every FFT stage.
    root_re: Vec<f64>,
    root_im: Vec<f64>,
    /// DCT-IV pre- and post-rotation, `m` each.
    pre_re: Vec<f64>,
    pre_im: Vec<f64>,
    post_re: Vec<f64>,
    post_im: Vec<f64>,
    /// FFT working buffers.
    re: Vec<f64>,
    im: Vec<f64>,
    /// DCT-IV output, `n/2` long.
    u: Vec<f32>,
}

impl Mdct {
    /// `n` is the block size in time samples; it must be a multiple of four.
    pub fn new(n: usize) -> Mdct {
        let n = n.max(4);
        let half = n / 2;
        let m = n / 4;
        let mut bitrev = vec![0u32; m];
        let bits = m.trailing_zeros();
        for (i, slot) in bitrev.iter_mut().enumerate() {
            let mut v = 0usize;
            for b in 0..bits {
                if i & (1 << b) != 0 {
                    v |= 1 << (bits - 1 - b);
                }
            }
            *slot = v as u32;
        }
        let mut root_re = vec![0.0f64; (m / 2).max(1)];
        let mut root_im = vec![0.0f64; (m / 2).max(1)];
        for k in 0..root_re.len() {
            let ang = -2.0 * PI * k as f64 / m as f64;
            root_re[k] = ang.cos();
            root_im[k] = ang.sin();
        }
        let mut pre_re = vec![0.0f64; m];
        let mut pre_im = vec![0.0f64; m];
        let mut post_re = vec![0.0f64; m];
        let mut post_im = vec![0.0f64; m];
        for j in 0..m {
            let ang = -PI * (4.0 * j as f64 + 1.0) / (4.0 * half as f64);
            pre_re[j] = ang.cos();
            pre_im[j] = ang.sin();
            let ang = -PI * j as f64 / half as f64;
            post_re[j] = ang.cos();
            post_im[j] = ang.sin();
        }
        Mdct {
            n,
            m,
            bitrev,
            root_re,
            root_im,
            pre_re,
            pre_im,
            post_re,
            post_im,
            re: vec![0.0; m],
            im: vec![0.0; m],
            u: vec![0.0; half],
        }
    }

    pub fn block_size(&self) -> usize {
        self.n
    }

    /// In-place iterative radix-2 complex FFT (decimation in time):
    /// `Z[j] = sum_k z[k] e^{-2*pi*i*j*k/m}`.
    fn fft(&mut self) {
        let m = self.m;
        for i in 0..m {
            let j = self.bitrev[i] as usize;
            if i < j {
                self.re.swap(i, j);
                self.im.swap(i, j);
            }
        }
        let mut len = 2usize;
        while len <= m {
            let half = len / 2;
            let stride = m / len;
            let mut i = 0usize;
            while i < m {
                for k in 0..half {
                    let t = k * stride;
                    let (cr, ci) = (self.root_re[t], self.root_im[t]);
                    let (ur, ui) = (self.re[i + k], self.im[i + k]);
                    let (vr0, vi0) = (self.re[i + k + half], self.im[i + k + half]);
                    let vr = vr0 * cr - vi0 * ci;
                    let vi = vr0 * ci + vi0 * cr;
                    self.re[i + k] = ur + vr;
                    self.im[i + k] = ui + vi;
                    self.re[i + k + half] = ur - vr;
                    self.im[i + k + half] = ui - vi;
                }
                i += len;
            }
            len <<= 1;
        }
    }

    /// DCT-IV via a half-length complex FFT, into `self.u`.
    ///
    /// `z[j] = (x[2j] + i x[N-1-2j]) * e^{-i*pi*(4j+1)/(4N)}`, forward FFT, then
    /// a post-rotation of `e^{-i*pi*j/N}`; the real and imaginary parts scatter
    /// to the even and reversed-odd outputs. (The quarter-sample offset belongs
    /// to the pre-twiddle only; applying it twice was the bug the direct oracle
    /// caught in the first place.)
    fn dct4(&mut self, x: &[f32]) {
        let half = self.n / 2;
        let m = self.m;
        for j in 0..m {
            let a = x.get(2 * j).copied().unwrap_or(0.0) as f64;
            let b = x.get(half - 1 - 2 * j).copied().unwrap_or(0.0) as f64;
            let (c, s) = (self.pre_re[j], self.pre_im[j]);
            self.re[j] = a * c - b * s;
            self.im[j] = a * s + b * c;
        }
        self.fft();
        for j in 0..m {
            let (c, s) = (self.post_re[j], self.post_im[j]);
            let tr = self.re[j] * c - self.im[j] * s;
            let ti = self.re[j] * s + self.im[j] * c;
            self.u[2 * j] = tr as f32;
            self.u[half - 1 - 2 * j] = -ti as f32;
        }
    }

    /// IMDCT: `n/2` spectral coefficients in, `n` time samples into `out`.
    ///
    /// `y[i] = sum_k X[k] cos(pi/n2 * (i + 0.5 + n2/2) * (k + 0.5))`, which is a
    /// DCT-IV read through the kernel's symmetries: antisymmetric about
    /// `j = n2 - 0.5`, negating every `2*n2` step.
    ///
    /// Deliberately unnormalised — the encoder's forward transform already
    /// carried the `1/M` factor, and applying `2/n` here scales every block down
    /// by `n/2` (measured: the reference is ours times 1024 on 2048-sample
    /// blocks, and a different factor on 256-sample ones, because `2/n` varies
    /// with the block size while the true correction does not).
    pub fn imdct(&mut self, spectrum: &[f32], out: &mut [f32]) {
        let half = self.n / 2;
        if out.len() < self.n || spectrum.len() < half {
            return;
        }
        self.dct4(spectrum);
        let shift = half / 2;
        let period = 2 * half;
        for (i, o) in out.iter_mut().enumerate().take(self.n) {
            let j = i + shift;
            let wrapped = j % (2 * period);
            let (idx, sign) =
                if wrapped < period { (wrapped, 1.0f32) } else { (wrapped - period, -1.0f32) };
            *o = if idx < half { sign * self.u[idx] } else { -sign * self.u[period - 1 - idx] };
        }
    }
}

/// The Vorbis window: `sin(pi/2 * sin^2(pi/n * (i + 0.5)))`.
pub fn window(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let s = (PI / n as f64 * (i as f64 + 0.5)).sin();
            ((PI / 2.0) * s * s).sin() as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// DCT-IV, textbook definition. The oracle, O(N^2).
    fn dct4_direct(x: &[f32]) -> Vec<f32> {
        let n = x.len();
        (0..n)
            .map(|i| {
                let mut acc = 0.0f64;
                for (k, &xk) in x.iter().enumerate() {
                    let ang = PI / n as f64 * (i as f64 + 0.5) * (k as f64 + 0.5);
                    acc += xk as f64 * ang.cos();
                }
                acc as f32
            })
            .collect()
    }

    /// The IMDCT straight from its definition. The oracle for `Mdct::imdct`.
    fn imdct_direct(spectrum: &[f32]) -> Vec<f32> {
        let n2 = spectrum.len();
        let n = n2 * 2;
        (0..n)
            .map(|i| {
                let mut acc = 0.0f64;
                for (k, &xk) in spectrum.iter().enumerate() {
                    let ang =
                        PI / n2 as f64 * (i as f64 + 0.5 + n2 as f64 / 2.0) * (k as f64 + 0.5);
                    acc += xk as f64 * ang.cos();
                }
                acc as f32
            })
            .collect()
    }

    #[test]
    fn fast_dct4_matches_the_direct_definition() {
        for &n in &[4usize, 8, 16, 64, 256, 1024] {
            let x = rand_vec(n, n as u64 * 7 + 1);
            let mut m = Mdct::new(n * 2);
            m.dct4(&x);
            let slow = dct4_direct(&x);
            let tol = 1e-3 * n as f32;
            #[allow(clippy::needless_range_loop)]
            for i in 0..n {
                assert!(
                    (m.u[i] - slow[i]).abs() < tol,
                    "n={n} i={i}: fast {} vs direct {}",
                    m.u[i],
                    slow[i]
                );
            }
        }
    }

    #[test]
    fn imdct_matches_its_definition() {
        for &n2 in &[2usize, 4, 16, 64, 256, 1024] {
            let x = rand_vec(n2, n2 as u64 * 13 + 3);
            let mut m = Mdct::new(n2 * 2);
            let mut got = vec![0f32; n2 * 2];
            m.imdct(&x, &mut got);
            let want = imdct_direct(&x);
            let tol = 1e-3 * n2 as f32;
            for i in 0..n2 * 2 {
                assert!((got[i] - want[i]).abs() < tol, "n2={n2} i={i}: {} vs {}", got[i], want[i]);
            }
        }
    }

    #[test]
    fn imdct_of_a_single_bin_is_a_cosine() {
        // A lone coefficient must produce exactly its kernel, which is the
        // property every sign error breaks.
        let n2 = 32usize;
        let mut x = vec![0f32; n2];
        x[5] = 1.0;
        let mut m = Mdct::new(n2 * 2);
        let mut got = vec![0f32; n2 * 2];
        m.imdct(&x, &mut got);
        for (i, &g) in got.iter().enumerate() {
            let ang = PI / n2 as f64 * (i as f64 + 0.5 + n2 as f64 / 2.0) * 5.5;
            assert!((g as f64 - ang.cos()).abs() < 1e-4, "i={i}");
        }
    }

    #[test]
    fn imdct_reuses_its_buffers_without_drift() {
        // The same transform run twice must give the same answer: a stale
        // scratch buffer is exactly the bug reuse invites.
        let x = rand_vec(128, 99);
        let mut m = Mdct::new(256);
        let mut a = vec![0f32; 256];
        let mut b = vec![0f32; 256];
        m.imdct(&x, &mut a);
        m.imdct(&rand_vec(128, 7), &mut vec![0f32; 256]);
        m.imdct(&x, &mut b);
        assert_eq!(a, b);
    }

    #[test]
    fn short_buffers_are_refused_not_indexed() {
        let mut m = Mdct::new(64);
        let mut out = vec![0f32; 8];
        m.imdct(&[0.0; 32], &mut out);
        assert!(out.iter().all(|&v| v == 0.0));
        let mut out = vec![0f32; 64];
        m.imdct(&[0.0; 4], &mut out);
        assert!(out.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn window_is_symmetric_and_power_complementary() {
        let w = window(64);
        for i in 0..32 {
            assert!((w[i] - w[63 - i]).abs() < 1e-6);
        }
        // Princen-Bradley: w[i]^2 + w[i + n/2]^2 == 1 over the overlap.
        for i in 0..32 {
            let s = w[i] * w[i] + w[i + 32] * w[i + 32];
            assert!((s - 1.0).abs() < 1e-5, "i={i} sum={s}");
        }
    }

    #[test]
    fn fft_of_an_impulse_is_flat() {
        let mut m = Mdct::new(64);
        m.re.fill(0.0);
        m.im.fill(0.0);
        m.re[0] = 1.0;
        m.fft();
        for k in 0..m.m {
            assert!((m.re[k] - 1.0).abs() < 1e-9 && m.im[k].abs() < 1e-9);
        }
    }

    #[test]
    fn mdct_round_trips_through_overlap_add() {
        // Two overlapped blocks of the same signal must reconstruct it: this is
        // the property the whole decoder rests on. Forward MDCT by definition,
        // inverse by the fast path, windowed both ways.
        let n = 128usize;
        let n2 = n / 2;
        let w = window(n);
        let signal = rand_vec(n * 3, 4242);
        let mut out = vec![0f32; n * 3];
        let mut m = Mdct::new(n);
        let mut block = vec![0f32; n];
        for start in (0..n * 2).step_by(n2) {
            // Forward MDCT of the windowed frame, by definition.
            let mut spec = vec![0f32; n2];
            for (k, s) in spec.iter_mut().enumerate() {
                let mut acc = 0.0f64;
                for i in 0..n {
                    let ang = PI / n2 as f64 * (i as f64 + 0.5 + n2 as f64 / 2.0) * (k as f64 + 0.5);
                    acc += (signal[start + i] * w[i]) as f64 * ang.cos();
                }
                // 4/n is the TDAC normalisation for this unnormalised pair.
                *s = (acc * 4.0 / n as f64) as f32;
            }
            m.imdct(&spec, &mut block);
            for i in 0..n {
                out[start + i] += block[i] * w[i];
            }
        }
        // The middle of the run — covered by two blocks — is the input back.
        for i in n2..n * 2 {
            assert!((out[i] - signal[i]).abs() < 1e-3, "i={i}: {} vs {}", out[i], signal[i]);
        }
    }
}
