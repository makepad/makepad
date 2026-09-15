// Shared numerical analysis for the drum kit tests: rendering helpers, a
// radix-2 FFT, band energies, spectral centroid, peak picking. Written
// from first principles (no dependencies), f64 accumulation.
#![allow(dead_code)]

use makepad_drumkit_phys::{DrumKit, DrumVoice};

pub const FS: f32 = 48000.0;

/// A trigger at an absolute sample.
#[derive(Clone, Copy)]
pub struct Hit {
    pub at: usize,
    pub voice: DrumVoice,
    pub velocity: f32,
}

/// Renders `total` frames in blocks of `block`, firing hits at their exact
/// block boundary (hits must fall on multiples of every block size used).
pub fn render(kit: &mut DrumKit, hits: &[Hit], total: usize, block: usize) -> Vec<[f32; 2]> {
    let mut out = vec![[0.0f32; 2]; total];
    let mut pos = 0;
    while pos < total {
        let n = block.min(total - pos);
        for h in hits {
            if h.at == pos {
                kit.trigger(h.voice, h.velocity);
            }
        }
        kit.process(&mut out[pos..pos + n]);
        pos += n;
    }
    out
}

/// One hit rendered until the voice goes quiet or `max_s`.
pub fn render_hit(fs: f32, voice: DrumVoice, velocity: f32, max_s: f32) -> (Vec<f32>, f32) {
    let mut kit = DrumKit::new(fs);
    kit.trigger(voice, velocity);
    let mut out: Vec<f32> = Vec::new();
    let mut block = [[0.0f32; 2]; 256];
    let max = (max_s * fs) as usize;
    while kit.active() && out.len() < max {
        block.iter_mut().for_each(|f| *f = [0.0; 2]);
        kit.process(&mut block);
        out.extend(block.iter().map(|f| 0.5 * (f[0] + f[1])));
    }
    let life = out.len() as f32 / fs;
    (out, life)
}

pub fn mono(x: &[[f32; 2]]) -> Vec<f32> {
    x.iter().map(|f| 0.5 * (f[0] + f[1])).collect()
}

pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |a, v| a.max(v.abs()))
}

pub fn energy(x: &[f32]) -> f64 {
    x.iter().map(|&v| (v as f64) * (v as f64)).sum()
}

pub fn db(x: f64) -> f64 {
    10.0 * x.max(1e-30).log10()
}

/// In-place radix-2 complex FFT (n a power of two).
pub fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    assert!(n.is_power_of_two() && im.len() == n);
    let mut j = 0;
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
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f64::consts::PI / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        let mut i = 0;
        while i < n {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (ur, ui) = (re[i + k], im[i + k]);
                let (vr, vi) = (re[i + k + len / 2] * cr - im[i + k + len / 2] * ci, re[i + k + len / 2] * ci + im[i + k + len / 2] * cr);
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

/// Power spectrum (Hann window) of x[a..b], zero-padded to a power of two.
/// Returns (bin frequencies, power).
pub fn spectrum(x: &[f32], fs: f32, a: usize, b: usize) -> (Vec<f64>, Vec<f64>) {
    let b = b.min(x.len());
    let seg = &x[a.min(b)..b];
    let n = seg.len().next_power_of_two().max(64);
    let mut re = vec![0.0f64; n];
    let mut im = vec![0.0f64; n];
    let m = seg.len();
    for (i, &v) in seg.iter().enumerate() {
        let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / m as f64).cos();
        re[i] = v as f64 * w;
    }
    fft(&mut re, &mut im);
    let half = n / 2;
    let freqs: Vec<f64> = (0..half).map(|k| k as f64 * fs as f64 / n as f64).collect();
    let pw: Vec<f64> = (0..half).map(|k| re[k] * re[k] + im[k] * im[k]).collect();
    (freqs, pw)
}

/// x filtered to [f_lo, f_hi) Hz by FFT masking (zero phase, brickwall),
/// same length as x.
pub fn band_signal(x: &[f32], fs: f32, f_lo: f64, f_hi: f64) -> Vec<f32> {
    let n = x.len().next_power_of_two().max(64);
    let mut re = vec![0.0f64; n];
    let mut im = vec![0.0f64; n];
    for (i, &v) in x.iter().enumerate() {
        re[i] = v as f64;
    }
    fft(&mut re, &mut im);
    let df = fs as f64 / n as f64;
    for k in 0..n {
        let f = if k <= n / 2 { k as f64 * df } else { (n - k) as f64 * df };
        if f < f_lo || f >= f_hi {
            re[k] = 0.0;
            im[k] = 0.0;
        }
    }
    // inverse via conjugation
    for v in im.iter_mut() {
        *v = -*v;
    }
    fft(&mut re, &mut im);
    re.iter().take(x.len()).map(|v| (*v / n as f64) as f32).collect()
}

/// Energy of the [f_lo, f_hi) band of x in the window [t0, t1) seconds
/// (time-domain sum of the band-filtered signal, as the reference harness).
pub fn band_energy(x: &[f32], fs: f32, t0: f32, t1: f32, f_lo: f64, f_hi: f64) -> f64 {
    let b = band_signal(x, fs, f_lo, f_hi);
    let (a, e) = ((t0 * fs) as usize, ((t1 * fs) as usize).min(b.len()));
    energy(&b[a.min(e)..e])
}

/// Spectral centroid (Hz) of x in [t0, t1) above 20 Hz.
pub fn centroid(x: &[f32], fs: f32, t0: f32, t1: f32) -> f64 {
    let (f, p) = spectrum(x, fs, (t0 * fs) as usize, (t1 * fs) as usize);
    let (mut num, mut den) = (0.0, 0.0);
    for (fr, v) in f.iter().zip(&p) {
        if *fr >= 20.0 {
            num += fr * v;
            den += v;
        }
    }
    num / den.max(1e-30)
}

/// Strongest spectral peak (Hz) below f_max in [t0, t1), with parabolic
/// interpolation.
pub fn strongest_partial(x: &[f32], fs: f32, t0: f32, t1: f32, f_max: f64) -> f64 {
    let (f, p) = spectrum(x, fs, (t0 * fs) as usize, (t1 * fs) as usize);
    let lim = f.iter().position(|v| *v > f_max).unwrap_or(f.len());
    let mut best = 1;
    for i in 1..lim.saturating_sub(1) {
        if p[i] > p[best] {
            best = i;
        }
    }
    let (y0, y1, y2) = (p[best - 1].max(1e-30).ln(), p[best].max(1e-30).ln(), p[best + 1].max(1e-30).ln());
    let d = 0.5 * (y0 - y2) / (y0 - 2.0 * y1 + y2 + 1e-30);
    f[best] + d * (f[1] - f[0])
}

/// Mean instantaneous frequency (Hz) from positive-going zero crossings of
/// the < f_max band of x within [t0, t1) — the reference harness's tracker.
pub fn zc_frequency(x: &[f32], fs: f32, f_min: f64, f_max: f64, t0: f32, t1: f32) -> f64 {
    let lp = band_signal(x, fs, f_min, f_max);
    let (a, b) = ((t0 * fs) as usize, ((t1 * fs) as usize).min(lp.len()));
    let mut crossings = Vec::new();
    for i in a.max(1)..b {
        if lp[i - 1] < 0.0 && lp[i] >= 0.0 {
            let frac = -lp[i - 1] / (lp[i] - lp[i - 1]);
            crossings.push(i as f64 - 1.0 + frac as f64);
        }
    }
    if crossings.len() < 2 {
        return f64::NAN;
    }
    let periods = crossings.len() as f64 - 1.0;
    fs as f64 * periods / (crossings[crossings.len() - 1] - crossings[0])
}

/// Cowbell-ness of a cymbal: in the [t0, t1) window, spectral peaks in
/// [f_lo, f_hi] with > 3 dB prominence over a 40 Hz neighbourhood; returns
/// (number of peaks, max over peaks of level minus the median level of its
/// 6 nearest peaks, in dB). A fixed-ratio cluster scores > 45 dB on the
/// second number; the reference cymbals score 22-31 dB.
pub fn peakiness(x: &[f32], fs: f32, t0: f32, t1: f32, f_lo: f64, f_hi: f64) -> (usize, f64) {
    let (f, p) = spectrum(x, fs, (t0 * fs) as usize, (t1 * fs) as usize);
    let s: Vec<f64> = p.iter().map(|v| db(*v)).collect();
    let df = f[1] - f[0];
    let w = (40.0 / df).max(1.0) as usize;
    let mut peaks: Vec<(f64, f64)> = Vec::new();
    for i in (w + 1)..(s.len() - w - 1) {
        if f[i] < f_lo || f[i] > f_hi {
            continue;
        }
        if s[i] > s[i - 1] && s[i] >= s[i + 1] {
            let lo = s[i - w..i].iter().cloned().fold(f64::INFINITY, f64::min);
            let hi = s[i + 1..i + 1 + w].iter().cloned().fold(f64::INFINITY, f64::min);
            if s[i] - lo.max(hi) > 3.0 {
                peaks.push((f[i], s[i]));
            }
        }
    }
    if peaks.len() < 8 {
        return (peaks.len(), f64::NAN);
    }
    let mut worst = f64::MIN;
    for k in 0..peaks.len() {
        let mut d: Vec<(f64, f64)> = peaks.iter().enumerate().filter(|(j, _)| *j != k).map(|(_, q)| ((q.0 - peaks[k].0).abs(), q.1)).collect();
        d.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let mut lv: Vec<f64> = d.iter().take(6).map(|q| q.1).collect();
        lv.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = 0.5 * (lv[2] + lv[3]);
        worst = worst.max(peaks[k].1 - med);
    }
    (peaks.len(), worst)
}

/// Time (s) of the peak of the > f_lo Hz energy, in `win` s windows.
pub fn high_band_peak_time(x: &[f32], fs: f32, f_lo: f64, win: f32, total: f32) -> f32 {
    let mut best = (0.0f32, -1.0f64);
    let mut t = 0.0f32;
    while t + win <= total {
        let e = band_energy(x, fs, t, t + win, f_lo, 24000.0);
        if e > best.1 {
            best = (t, e);
        }
        t += win;
    }
    best.0 + 0.5 * win
}

pub fn all_finite(x: &[[f32; 2]]) -> bool {
    x.iter().all(|f| f[0].is_finite() && f[1].is_finite())
}
