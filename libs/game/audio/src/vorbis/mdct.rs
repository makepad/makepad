//! IMDCT and the Vorbis window.
//!
//! The transform is the one place a subtle sign or twiddle error produces
//! plausible-looking garbage rather than an obvious failure, so this carries
//! two implementations: a direct O(N^2) DCT-IV that is transparently the
//! textbook definition, and an FFT-based one that is what actually runs. A
//! test asserts they agree — the slow one is the oracle for the fast one.

use std::f64::consts::PI;

/// In-place iterative radix-2 complex FFT (decimation in time).
/// `re`/`im` have power-of-two length. Forward transform:
/// `Z[j] = sum_m z[m] e^{-2*pi*i*j*m/M}`.
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two());
    debug_assert_eq!(im.len(), n);
    if n <= 1 {
        return;
    }

    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    let mut len = 2usize;
    while len <= n {
        let ang = -2.0 * PI / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        let mut i = 0usize;
        while i < n {
            let (mut cr, mut ci) = (1.0f64, 0.0f64);
            for k in 0..len / 2 {
                let (ur, ui) = (re[i + k], im[i + k]);
                let (vr0, vi0) = (re[i + k + len / 2], im[i + k + len / 2]);
                let vr = vr0 * cr - vi0 * ci;
                let vi = vr0 * ci + vi0 * cr;
                re[i + k] = ur + vr;
                im[i + k] = ui + vi;
                re[i + k + len / 2] = ur - vr;
                im[i + k + len / 2] = ui - vi;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
            i += len;
        }
        len <<= 1;
    }
}

/// DCT-IV, textbook definition. Reference only — O(N^2).
#[cfg(test)]
pub fn dct4_direct(x: &[f32]) -> Vec<f32> {
    let n = x.len();
    let mut out = vec![0.0f32; n];
    for (i, o) in out.iter_mut().enumerate() {
        let mut acc = 0.0f64;
        for (k, &xk) in x.iter().enumerate() {
            let ang = PI / n as f64 * (i as f64 + 0.5) * (k as f64 + 0.5);
            acc += xk as f64 * ang.cos();
        }
        *o = acc as f32;
    }
    out
}

/// DCT-IV via a half-length complex FFT.
///
/// `z[m] = (x[2m] + i x[N-1-2m]) * e^{-i*pi*(4m+1)/(4N)}`, forward FFT, then
/// the same twiddle again; the real/imaginary parts scatter to the even and
/// reversed-odd outputs.
pub fn dct4(x: &[f32]) -> Vec<f32> {
    let n = x.len();
    if n == 0 {
        return Vec::new();
    }
    if n % 2 != 0 || !n.is_power_of_two() {
        // Vorbis blocksizes are powers of two; anything else falls back to a
        // direct evaluation rather than producing silent garbage.
        return dct4_fallback(x);
    }
    let m = n / 2;
    let mut re = vec![0.0f64; m];
    let mut im = vec![0.0f64; m];
    for j in 0..m {
        let a = x[2 * j] as f64;
        let b = x[n - 1 - 2 * j] as f64;
        let ang = -PI * (4.0 * j as f64 + 1.0) / (4.0 * n as f64);
        let (c, s) = (ang.cos(), ang.sin());
        re[j] = a * c - b * s;
        im[j] = a * s + b * c;
    }
    fft(&mut re, &mut im);
    let mut out = vec![0.0f32; n];
    for j in 0..m {
        // Post-rotation is e^{-i*pi*j/N}: the quarter-sample offset belongs to
        // the pre-twiddle only, and applying it twice was the bug the direct
        // oracle caught.
        let ang = -PI * j as f64 / n as f64;
        let (c, s) = (ang.cos(), ang.sin());
        let tr = re[j] * c - im[j] * s;
        let ti = re[j] * s + im[j] * c;
        out[2 * j] = tr as f32;
        out[n - 1 - 2 * j] = -ti as f32;
    }
    out
}

fn dct4_fallback(x: &[f32]) -> Vec<f32> {
    let n = x.len();
    let mut out = vec![0.0f32; n];
    for (i, o) in out.iter_mut().enumerate() {
        let mut acc = 0.0f64;
        for (k, &xk) in x.iter().enumerate() {
            let ang = PI / n as f64 * (i as f64 + 0.5) * (k as f64 + 0.5);
            acc += xk as f64 * ang.cos();
        }
        *o = acc as f32;
    }
    out
}

/// IMDCT: `n/2` spectral coefficients in, `n` time samples out.
///
/// `y[i] = sum_k X[k] cos(pi/n2 * (i + 0.5 + n2/2) * (k + 0.5))`, which is a
/// DCT-IV read through the kernel's symmetries: antisymmetric about
/// `j = n2 - 0.5`, and negating every `2*n2` step.
pub fn imdct(spectrum: &[f32]) -> Vec<f32> {
    let n2 = spectrum.len();
    if n2 == 0 {
        return Vec::new();
    }
    let n = n2 * 2;
    let u = dct4(spectrum);
    let shift = n2 / 2;
    let mut y = vec![0.0f32; n];
    for (i, out) in y.iter_mut().enumerate() {
        let j = i + shift;
        *out = extend(&u, j, n2);
    }
    y
}

/// Evaluate the periodically-extended DCT-IV at index `j`.
fn extend(u: &[f32], j: usize, n2: usize) -> f32 {
    let period = 2 * n2;
    let wrapped = j % (2 * period);
    let (idx, sign) = if wrapped < period {
        (wrapped, 1.0f32)
    } else {
        (wrapped - period, -1.0f32)
    };
    if idx < n2 {
        sign * u[idx]
    } else {
        -sign * u[period - 1 - idx]
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
                let v = s.wrapping_mul(0x2545F4914F6CDD1D);
                ((v >> 40) as f32 / 16777216.0) * 2.0 - 1.0
            })
            .collect()
    }

    #[test]
    fn fast_dct4_matches_the_direct_definition() {
        // The slow one is the oracle: if these disagree the fast twiddles are
        // wrong, which is exactly the bug that would otherwise ship as noise.
        for &n in &[4usize, 8, 16, 64, 256, 1024] {
            let x = rand_vec(n, n as u64 * 7 + 1);
            let fast = dct4(&x);
            let slow = dct4_direct(&x);
            let tol = 1e-3 * n as f32;
            for i in 0..n {
                assert!(
                    (fast[i] - slow[i]).abs() < tol,
                    "n={n} i={i}: fast {} vs direct {}",
                    fast[i],
                    slow[i]
                );
            }
        }
    }

    #[test]
    fn imdct_matches_its_definition() {
        for &n2 in &[4usize, 16, 64, 256] {
            let x = rand_vec(n2, n2 as u64 * 13 + 3);
            let got = imdct(&x);
            let n = n2 * 2;
            assert_eq!(got.len(), n);
            for i in 0..n {
                let mut acc = 0.0f64;
                for (k, &xk) in x.iter().enumerate() {
                    let ang = PI / n2 as f64
                        * (i as f64 + 0.5 + n2 as f64 / 2.0)
                        * (k as f64 + 0.5);
                    acc += xk as f64 * ang.cos();
                }
                let want = acc as f32;
                let tol = 1e-3 * n2 as f32;
                assert!(
                    (got[i] - want).abs() < tol,
                    "n2={n2} i={i}: {} vs {}",
                    got[i],
                    want
                );
            }
        }
    }

    #[test]
    fn window_is_symmetric_and_power_complementary() {
        let w = window(64);
        for i in 0..32 {
            assert!((w[i] - w[63 - i]).abs() < 1e-6);
        }
        // Princen-Bradley: w[i]^2 + w[i+n/2]^2 == 1 over the overlap.
        for i in 0..32 {
            let s = w[i] * w[i] + w[i + 32] * w[i + 32];
            assert!((s - 1.0).abs() < 1e-5, "i={i} sum={s}");
        }
    }

    #[test]
    fn fft_of_impulse_is_flat() {
        let mut re = vec![0.0f64; 16];
        let mut im = vec![0.0f64; 16];
        re[0] = 1.0;
        fft(&mut re, &mut im);
        for k in 0..16 {
            assert!((re[k] - 1.0).abs() < 1e-9 && im[k].abs() < 1e-9);
        }
    }
}
