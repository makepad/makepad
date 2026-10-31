//! Radix-2 FFT for the power-of-two sizes Cantor uses (512..2048), and the
//! real-signal wrappers the STFT, iSTFT and their adjoints are built from.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

pub struct Fft {
    pub n: usize,
    cos: Vec<f32>,
    sin: Vec<f32>,
    rev: Vec<u32>,
}

impl Fft {
    pub fn new(n: usize) -> Fft {
        assert!(n.is_power_of_two() && n >= 2, "fft size {n} must be a power of two");
        let bits = n.trailing_zeros();
        let rev = (0..n as u32).map(|i| i.reverse_bits() >> (32 - bits)).collect();
        let half = n / 2;
        let mut cos = Vec::with_capacity(half);
        let mut sin = Vec::with_capacity(half);
        for i in 0..half {
            let a = -2.0 * std::f64::consts::PI * i as f64 / n as f64;
            cos.push(a.cos() as f32);
            sin.push(a.sin() as f32);
        }
        Fft { n, cos, sin, rev }
    }

    /// The shared plan for size `n`.
    pub fn plan(n: usize) -> Arc<Fft> {
        static PLANS: OnceLock<Mutex<HashMap<usize, Arc<Fft>>>> = OnceLock::new();
        let plans = PLANS.get_or_init(|| Mutex::new(HashMap::new()));
        let mut plans = plans.lock().unwrap();
        plans.entry(n).or_insert_with(|| Arc::new(Fft::new(n))).clone()
    }

    /// In-place forward transform (e^{-i}); `inverse` uses e^{+i} and does not scale.
    pub fn complex(&self, re: &mut [f32], im: &mut [f32], inverse: bool) {
        let n = self.n;
        for i in 0..n {
            let j = self.rev[i] as usize;
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let sign = if inverse { -1.0 } else { 1.0 };
        let mut len = 2;
        while len <= n {
            let step = n / len;
            let half = len / 2;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let wr = self.cos[k * step];
                    let wi = sign * self.sin[k * step];
                    let a = start + k;
                    let b = a + half;
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len *= 2;
        }
    }

    /// Real input (length n) -> half spectrum (n/2+1 bins).
    pub fn rfft(&self, x: &[f32], out_re: &mut [f32], out_im: &mut [f32]) {
        let n = self.n;
        let mut re = x.to_vec();
        re.resize(n, 0.0);
        let mut im = vec![0.0; n];
        self.complex(&mut re, &mut im, false);
        let bins = n / 2 + 1;
        out_re[..bins].copy_from_slice(&re[..bins]);
        out_im[..bins].copy_from_slice(&im[..bins]);
    }

    /// Half spectrum -> real signal, scaled by 1/n (the true inverse of `rfft`).
    /// The imaginary parts of the DC and Nyquist bins are ignored.
    pub fn irfft(&self, in_re: &[f32], in_im: &[f32], out: &mut [f32]) {
        let n = self.n;
        let half = n / 2;
        let mut re = vec![0.0; n];
        let mut im = vec![0.0; n];
        re[0] = in_re[0];
        re[half] = in_re[half];
        for f in 1..half {
            re[f] = in_re[f];
            im[f] = in_im[f];
            re[n - f] = in_re[f];
            im[n - f] = -in_im[f];
        }
        self.complex(&mut re, &mut im, true);
        let s = 1.0 / n as f32;
        for i in 0..n {
            out[i] = re[i] * s;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfft_irfft_round_trip_and_dft() {
        let n = 64;
        let fft = Fft::new(n);
        let x: Vec<f32> = (0..n).map(|i| ((i * 7 % 13) as f32 - 6.0) / 5.0).collect();
        let mut re = vec![0.0; n / 2 + 1];
        let mut im = vec![0.0; n / 2 + 1];
        fft.rfft(&x, &mut re, &mut im);
        for f in 0..=n / 2 {
            let (mut dr, mut di) = (0.0f64, 0.0f64);
            for (t, v) in x.iter().enumerate() {
                let a = -2.0 * std::f64::consts::PI * (f * t) as f64 / n as f64;
                dr += *v as f64 * a.cos();
                di += *v as f64 * a.sin();
            }
            assert!((dr as f32 - re[f]).abs() < 1e-3 && (di as f32 - im[f]).abs() < 1e-3);
        }
        let mut y = vec![0.0; n];
        fft.irfft(&re, &im, &mut y);
        for i in 0..n {
            assert!((x[i] - y[i]).abs() < 1e-5);
        }
    }
}
