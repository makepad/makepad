// Shared numerical analysis for the verification tests: rendering helpers,
// windowed DFT / FFT, partial-peak measurement, decay fitting. All written
// here from first principles (no dependencies), all f64 accumulation.
#![allow(dead_code)]

use makepad_piano_model::{Piano, PianoEvent, TimedEvent};

pub const FS: f32 = 48000.0;

/// An absolute-time event script.
#[derive(Clone, Copy)]
pub struct Ev {
    pub at: u64, // absolute sample
    pub ev: PianoEvent,
}

pub fn ev(at_sec: f64, ev: PianoEvent) -> Ev {
    Ev { at: (at_sec * FS as f64).round() as u64, ev }
}

/// Renders `total` samples in blocks of `block`, feeding events at their
/// exact in-block offsets. Returns (left, right).
pub fn render(p: &mut Piano, script: &[Ev], total: usize, block: usize) -> (Vec<f32>, Vec<f32>) {
    let mut l = vec![0.0f32; total];
    let mut r = vec![0.0f32; total];
    let mut te: Vec<TimedEvent> = Vec::new();
    let mut pos = 0usize;
    while pos < total {
        let n = block.min(total - pos);
        te.clear();
        for e in script {
            if e.at >= pos as u64 && e.at < (pos + n) as u64 {
                te.push(TimedEvent { offset: (e.at - pos as u64) as u32, event: e.ev });
            }
        }
        p.process(&te, &mut l[pos..pos + n], &mut r[pos..pos + n]);
        pos += n;
    }
    (l, r)
}

/// Same, but through the multicore path.
pub fn render_mt(p: &mut Piano, script: &[Ev], total: usize, block: usize, workers: usize) -> (Vec<f32>, Vec<f32>) {
    let mut l = vec![0.0f32; total];
    let mut r = vec![0.0f32; total];
    let mut te: Vec<TimedEvent> = Vec::new();
    let mut pos = 0usize;
    while pos < total {
        let n = block.min(total - pos);
        te.clear();
        for e in script {
            if e.at >= pos as u64 && e.at < (pos + n) as u64 {
                te.push(TimedEvent { offset: (e.at - pos as u64) as u32, event: e.ev });
            }
        }
        p.process_multicore(&te, &mut l[pos..pos + n], &mut r[pos..pos + n], workers);
        pos += n;
    }
    (l, r)
}

/// A piano with the output niceties defeated, for physics measurements on
/// the raw instrument.
pub fn dry_piano() -> Piano {
    let mut p = Piano::new(FS);
    p.set_reverb_mix(0.0);
    p.set_early_reflection_level(0.0);
    p.set_soft_clip(false);
    p
}

pub fn mono(l: &[f32], r: &[f32]) -> Vec<f32> {
    l.iter().zip(r).map(|(a, b)| 0.5 * (a + b)).collect()
}

pub fn rms(x: &[f32]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / x.len() as f64).sqrt()
}

pub fn peak(x: &[f32]) -> f64 {
    x.iter().fold(0.0f64, |m, &v| m.max((v as f64).abs()))
}

pub fn sec(x: &[f32], t0: f64, t1: f64) -> &[f32] {
    let a = ((t0 * FS as f64) as usize).min(x.len());
    let b = ((t1 * FS as f64) as usize).min(x.len());
    &x[a..b]
}

/// Hann-windowed single-frequency DFT magnitude (normalized by window sum).
pub fn dft_mag(x: &[f32], f: f64) -> f64 {
    let n = x.len();
    let mut re = 0.0f64;
    let mut im = 0.0f64;
    let mut wsum = 0.0f64;
    let w0 = std::f64::consts::TAU * f / FS as f64;
    for (k, &v) in x.iter().enumerate() {
        let w = 0.5 - 0.5 * (std::f64::consts::TAU * k as f64 / n as f64).cos();
        wsum += w;
        let ph = w0 * k as f64;
        re += w * v as f64 * ph.cos();
        im -= w * v as f64 * ph.sin();
    }
    2.0 * (re * re + im * im).sqrt() / wsum
}

/// Finds the strongest spectral peak within [guess-half, guess+half] by
/// coarse grid + parabolic refinement. Returns (freq, magnitude).
pub fn peak_near(x: &[f32], guess: f64, half: f64) -> (f64, f64) {
    let step = (half / 30.0).max(0.05);
    let mut best_f = guess;
    let mut best_m = -1.0f64;
    let mut f = guess - half;
    while f <= guess + half {
        if f > 5.0 {
            let m = dft_mag(x, f);
            if m > best_m {
                best_m = m;
                best_f = f;
            }
        }
        f += step;
    }
    // parabolic refine on log-magnitude
    let m0 = dft_mag(x, best_f - step).max(1e-30).ln();
    let m1 = best_m.max(1e-30).ln();
    let m2 = dft_mag(x, best_f + step).max(1e-30).ln();
    let denom = m0 - 2.0 * m1 + m2;
    let d = if denom.abs() > 1e-12 { 0.5 * (m0 - m2) / denom } else { 0.0 };
    let f_ref = best_f + d.clamp(-1.0, 1.0) * step;
    (f_ref, dft_mag(x, f_ref))
}

/// Exponential decay rate sigma (1/s) of the component at frequency `f`,
/// fitted by linear regression of log magnitude over sliding windows in
/// [t0, t1]. Window 80 ms, hop 40 ms.
pub fn decay_sigma(x: &[f32], f: f64, t0: f64, t1: f64) -> f64 {
    let win = (0.080 * FS as f64) as usize;
    let hop = win / 2;
    let mut ts: Vec<f64> = Vec::new();
    let mut ms: Vec<f64> = Vec::new();
    let mut a = (t0 * FS as f64) as usize;
    let end = ((t1 * FS as f64) as usize).min(x.len());
    while a + win <= end {
        let m = dft_mag(&x[a..a + win], f);
        if m > 1e-12 {
            ts.push((a + win / 2) as f64 / FS as f64);
            ms.push(m.ln());
        }
        a += hop;
    }
    linreg_slope(&ts, &ms).map(|s| -s).unwrap_or(0.0)
}

/// Least-squares slope of y over x; None if degenerate.
pub fn linreg_slope(x: &[f64], y: &[f64]) -> Option<f64> {
    if x.len() < 3 {
        return None;
    }
    let n = x.len() as f64;
    let sx: f64 = x.iter().sum();
    let sy: f64 = y.iter().sum();
    let sxx: f64 = x.iter().map(|v| v * v).sum();
    let sxy: f64 = x.iter().zip(y).map(|(a, b)| a * b).sum();
    let d = n * sxx - sx * sx;
    if d.abs() < 1e-12 {
        return None;
    }
    Some((n * sxy - sx * sy) / d)
}

/// Radix-2 FFT (in-place, f64). Length must be a power of two.
pub fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    assert!(n.is_power_of_two() && im.len() == n);
    // bit reversal
    let mut j = 0usize;
    for i in 0..n {
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
        let mut m = n >> 1;
        while m >= 1 && j & m != 0 {
            j ^= m;
            m >>= 1;
        }
        j |= m;
    }
    let mut len = 2;
    while len <= n {
        let ang = -std::f64::consts::TAU / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        let mut i = 0;
        while i < n {
            let mut cr = 1.0f64;
            let mut ci = 0.0f64;
            for k in 0..len / 2 {
                let (ar, ai) = (re[i + k], im[i + k]);
                let (br, bi) = (re[i + k + len / 2], im[i + k + len / 2]);
                let tr = br * cr - bi * ci;
                let ti = br * ci + bi * cr;
                re[i + k] = ar + tr;
                im[i + k] = ai + ti;
                re[i + k + len / 2] = ar - tr;
                im[i + k + len / 2] = ai - ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
            i += len;
        }
        len <<= 1;
    }
}

/// Hann-windowed power spectrum of x (zero-padded to the next power of two).
/// Returns (bin_hz, power-per-bin).
pub fn power_spectrum(x: &[f32]) -> (f64, Vec<f64>) {
    let n = x.len().next_power_of_two();
    let mut re = vec![0.0f64; n];
    let mut im = vec![0.0f64; n];
    for (k, &v) in x.iter().enumerate() {
        let w = 0.5 - 0.5 * (std::f64::consts::TAU * k as f64 / x.len() as f64).cos();
        re[k] = w * v as f64;
    }
    fft(&mut re, &mut im);
    let bin = FS as f64 / n as f64;
    let ps: Vec<f64> = (0..n / 2).map(|k| re[k] * re[k] + im[k] * im[k]).collect();
    (bin, ps)
}

pub fn band_power(bin: f64, ps: &[f64], lo: f64, hi: f64) -> f64 {
    let a = (lo / bin).ceil() as usize;
    let b = ((hi / bin).floor() as usize).min(ps.len().saturating_sub(1));
    if a >= b {
        return 0.0;
    }
    ps[a..=b].iter().sum()
}

pub fn spectral_centroid(bin: f64, ps: &[f64], lo: f64, hi: f64) -> f64 {
    let a = (lo / bin).ceil() as usize;
    let b = ((hi / bin).floor() as usize).min(ps.len().saturating_sub(1));
    let mut num = 0.0;
    let mut den = 0.0;
    for k in a..=b {
        num += k as f64 * bin * ps[k];
        den += ps[k];
    }
    if den > 0.0 {
        num / den
    } else {
        0.0
    }
}

pub fn assert_all_finite(x: &[f32]) {
    for (i, &v) in x.iter().enumerate() {
        assert!(v.is_finite(), "non-finite sample {v} at index {i}");
    }
}
