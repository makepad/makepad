//! What a sound measures as: brightness, attack, decay, where its energy
//! sits, how loud it is, and the faults a patch gate refuses (clipping,
//! DC, non-finite samples, a harsh 2-5 kHz spike).

/// One rendered sound, measured.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Features {
    /// Spectral centroid of the sounding part (Hz).
    pub centroid: f32,
    /// Onset to within 3 dB of the peak (ms).
    pub attack_ms: f32,
    /// Peak to 20 dB down (ms); the whole render when it never gets there.
    pub decay_ms: f32,
    /// Share of the energy below 250 Hz, 250 Hz-4 kHz and above 4 kHz (sums to 1).
    pub low: f32,
    pub mid: f32,
    pub high: f32,
    /// How loud it hits: the loudest 400 ms block, BS.1770 K-weighted (momentary LUFS).
    pub lufs: f32,
    /// Sample peak (dBFS).
    pub peak_db: f32,
    /// |mean| / RMS: 0 for a centred signal.
    pub dc: f32,
    /// The loudest 2-5 kHz third-octave against the 500 Hz-1.6 kHz thirds (dB per Hz): positive is a presence spike.
    pub harsh_db: f32,
    /// Any NaN or infinite sample.
    pub nonfinite: bool,
}

impl Features {
    pub const COLUMNS: [&'static str; 11] = ["centroid", "attack_ms", "decay_ms", "low", "mid", "high", "lufs", "peak_db", "dc", "harsh_db", "nonfinite"];

    pub fn values(&self) -> [f32; 11] {
        [self.centroid, self.attack_ms, self.decay_ms, self.low, self.mid, self.high, self.lufs, self.peak_db, self.dc, self.harsh_db, self.nonfinite as u8 as f32]
    }

    pub fn from_values(v: &[f32]) -> Option<Features> {
        if v.len() < 11 {
            return None;
        }
        Some(Features {
            centroid: v[0], attack_ms: v[1], decay_ms: v[2], low: v[3], mid: v[4], high: v[5], lufs: v[6], peak_db: v[7], dc: v[8], harsh_db: v[9], nonfinite: v[10] != 0.0,
        })
    }

    /// The gate every patch and kit piece passes: why it fails, if it does.
    pub fn faults(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.nonfinite {
            out.push("non-finite samples".into());
        }
        if self.peak_db > -0.1 {
            out.push(format!("clips (peak {:.1} dBFS)", self.peak_db));
        }
        if self.dc > 0.05 {
            out.push(format!("DC offset ({:.2} of RMS)", self.dc));
        }
        if self.harsh_db > 6.0 {
            out.push(format!("harsh 2-5 kHz spike (+{:.1} dB)", self.harsh_db));
        }
        if self.lufs < -36.0 {
            out.push(format!("too quiet ({:.1} LUFS)", self.lufs));
        }
        if self.lufs > -6.0 {
            out.push(format!("too loud ({:.1} LUFS)", self.lufs));
        }
        out
    }
}

/// Measure a stereo render (the sound starts near frame 0).
pub fn measure(left: &[f32], right: &[f32], rate: f32) -> Features {
    let n = left.len().min(right.len());
    let mono: Vec<f32> = (0..n).map(|i| 0.5 * (left[i] + right[i])).collect();
    let nonfinite = left[..n].iter().chain(&right[..n]).any(|x| !x.is_finite());
    let clean = |x: f32| if x.is_finite() { x } else { 0.0 };
    let peak = left[..n].iter().chain(&right[..n]).fold(0.0f32, |m, x| m.max(clean(*x).abs()));
    let mut f = Features { nonfinite, peak_db: db(peak), ..Features::default() };
    if n == 0 || peak <= 1e-7 {
        f.lufs = -120.0;
        return f;
    }
    // Envelope: RMS over 5 ms windows.
    let win = ((rate * 0.005) as usize).max(1);
    let env: Vec<f32> = mono.chunks(win).map(|c| (c.iter().map(|x| clean(*x) * clean(*x)).sum::<f32>() / c.len() as f32).sqrt()).collect();
    let env_peak = env.iter().fold(0.0f32, |m, v| m.max(*v));
    // The first window at the peak (a steady tone's later windows only wobble).
    let peak_at = env.iter().position(|v| *v >= env_peak * 0.97).unwrap_or(0);
    let onset = env.iter().position(|v| *v > env_peak * 0.0316).unwrap_or(0); // -30 dB
    let near = env.iter().position(|v| *v >= env_peak * 0.708).unwrap_or(peak_at); // -3 dB
    f.attack_ms = (near.saturating_sub(onset) * win) as f32 / rate * 1000.0;
    let down = env[peak_at..].iter().position(|v| *v < env_peak * 0.1).map(|i| i + peak_at).unwrap_or(env.len());
    f.decay_ms = ((down - peak_at) * win) as f32 / rate * 1000.0;
    // DC.
    let mean = mono.iter().map(|x| clean(*x) as f64).sum::<f64>() / n as f64;
    let rms = (mono.iter().map(|x| (clean(*x) as f64).powi(2)).sum::<f64>() / n as f64).sqrt();
    f.dc = if rms > 1e-9 { (mean.abs() / rms) as f32 } else { 0.0 };
    // Spectrum over the sounding part (to 40 dB below the peak).
    let end = env.iter().rposition(|v| *v > env_peak * 0.01).map(|i| (i + 1) * win).unwrap_or(n).min(n);
    let start = onset * win;
    let power = average_spectrum(&mono[start.min(end)..end], 2048);
    let bin_hz = rate / 2048.0;
    let band = |lo: f32, hi: f32| -> (f64, usize) {
        let (a, b) = (((lo / bin_hz).ceil() as usize).max(1), ((hi / bin_hz).floor() as usize).min(power.len() - 1));
        if b < a {
            return (0.0, 1);
        }
        (power[a..=b].iter().sum::<f64>(), b - a + 1)
    };
    let total = band(20.0, rate * 0.5).0.max(1e-30);
    f.low = (band(20.0, 250.0).0 / total) as f32;
    f.high = (band(4000.0, rate * 0.5).0 / total) as f32;
    f.mid = (1.0 - f.low - f.high).max(0.0);
    let (mut num, mut den) = (0.0f64, 0.0f64);
    for (i, p) in power.iter().enumerate().skip(1) {
        num += i as f64 * bin_hz as f64 * p;
        den += p;
    }
    f.centroid = if den > 0.0 { (num / den) as f32 } else { 0.0 };
    // Third-octave band levels (dB per Hz), so a lone harmonic does not
    // read as a spike: the loudest third in 2-5 kHz against the mean of
    // the 500 Hz-1.6 kHz thirds. Natural spectra fall with frequency; a
    // presence band standing above the mids is what sounds harsh.
    let third = |centre: f32| {
        let (e, bins) = band(centre / 1.122, centre * 1.122);
        10.0 * ((e / bins as f64).max(1e-30)).log10()
    };
    let mids = [500.0f32, 630.0, 800.0, 1000.0, 1250.0, 1600.0].iter().map(|c| third(*c)).sum::<f64>() / 6.0;
    f.harsh_db = ([2000.0f32, 2500.0, 3150.0, 4000.0, 5000.0].iter().map(|c| third(*c)).fold(f64::MIN, f64::max) - mids) as f32;
    f.lufs = lufs(left, right, rate);
    f
}

pub fn db(x: f32) -> f32 {
    20.0 * x.max(1e-6).log10()
}

/// Mean power spectrum of Hann frames of `size` (half overlap).
fn average_spectrum(x: &[f32], size: usize) -> Vec<f64> {
    let mut acc = vec![0.0f64; size / 2 + 1];
    let window: Vec<f32> = (0..size).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / size as f32).cos()).collect();
    let mut frames = 0;
    let mut start = 0;
    loop {
        let mut re = vec![0.0f32; size];
        let mut im = vec![0.0f32; size];
        for i in 0..size {
            let v = x.get(start + i).copied().unwrap_or(0.0);
            re[i] = if v.is_finite() { v * window[i] } else { 0.0 };
        }
        fft(&mut re, &mut im);
        for (k, a) in acc.iter_mut().enumerate() {
            *a += (re[k] as f64).powi(2) + (im[k] as f64).powi(2);
        }
        frames += 1;
        start += size / 2;
        if start + size / 2 >= x.len() {
            break;
        }
    }
    acc.iter_mut().for_each(|a| *a /= frames as f64);
    acc
}

/// In-place radix-2 FFT (`re.len()` a power of two).
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
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
        let ang = -std::f64::consts::TAU / len as f64;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (s, c) = (ang * k as f64).sin_cos();
                let (a, b) = (start + k, start + k + len / 2);
                let (xr, xi) = (re[b] as f64 * c - im[b] as f64 * s, re[b] as f64 * s + im[b] as f64 * c);
                re[b] = (re[a] as f64 - xr) as f32;
                im[b] = (im[a] as f64 - xi) as f32;
                re[a] = (re[a] as f64 + xr) as f32;
                im[a] = (im[a] as f64 + xi) as f32;
            }
        }
        len <<= 1;
    }
}

/// BS.1770 momentary loudness at its loudest: the K-weighted power of the
/// loudest 400 ms block (100 ms apart). A pluck and a pad compare by how
/// loud they hit, not by how long they last.
pub fn lufs(left: &[f32], right: &[f32], rate: f32) -> f32 {
    let k = |x: &[f32]| -> Vec<f64> {
        // The K-filter's two stages (shelf, then high-pass), designed at `rate`.
        let shelf = shelf(rate as f64);
        let hp = highpass(rate as f64);
        let mut a = Biquad::default();
        let mut b = Biquad::default();
        x.iter().map(|v| b.run(&hp, a.run(&shelf, if v.is_finite() { *v as f64 } else { 0.0 }))).collect()
    };
    let (l, r) = (k(left), k(right));
    let n = l.len().min(r.len());
    let block = (rate * 0.4) as usize;
    let hop = (rate * 0.1) as usize;
    let mut powers = Vec::new();
    let mut start = 0;
    while start < n {
        let end = (start + block).min(n);
        let p = (start..end).map(|i| l[i] * l[i] + r[i] * r[i]).sum::<f64>() / block as f64;
        powers.push(p);
        if end == n {
            break;
        }
        start += hop;
    }
    let loudest = powers.into_iter().fold(0.0f64, f64::max);
    (-0.691 + 10.0 * loudest.max(1e-30).log10()).max(-120.0) as f32
}

#[derive(Default)]
struct Biquad {
    z1: f64,
    z2: f64,
}

struct Coefs {
    b: [f64; 3],
    a: [f64; 2],
}

impl Biquad {
    fn run(&mut self, c: &Coefs, x: f64) -> f64 {
        let y = c.b[0] * x + self.z1;
        self.z1 = c.b[1] * x - c.a[0] * y + self.z2;
        self.z2 = c.b[2] * x - c.a[1] * y;
        y
    }
}

fn shelf(rate: f64) -> Coefs {
    let (f0, g, q) = (1681.974450955533, 3.999843853973347, 0.7071752369554196);
    let k = (std::f64::consts::PI * f0 / rate).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    Coefs { b: [(vh + vb * k / q + k * k) / a0, 2.0 * (k * k - vh) / a0, (vh - vb * k / q + k * k) / a0], a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0] }
}

fn highpass(rate: f64) -> Coefs {
    let (f0, q) = (38.13547087602444, 0.5003270373238773);
    let k = (std::f64::consts::PI * f0 / rate).tan();
    let a0 = 1.0 + k / q + k * k;
    Coefs { b: [1.0, -2.0, 1.0], a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0] }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sine_measures_as_a_sine() {
        let rate = 48_000.0;
        let x: Vec<f32> = (0..48_000).map(|i| 0.5 * (std::f32::consts::TAU * 1000.0 * i as f32 / rate).sin()).collect();
        let f = measure(&x, &x, rate);
        assert!((f.centroid - 1000.0).abs() < 60.0, "{f:?}");
        assert!(f.mid > 0.98 && f.attack_ms < 10.0 && f.decay_ms > 900.0, "{f:?}");
        // Mean square 0.125 a channel, two channels: -6.0 dB, -0.691, +0.7 dB of K-weighting at 1 kHz.
        assert!((f.lufs + 6.0).abs() < 0.5, "{}", f.lufs);
        assert!(f.harsh_db.is_finite() && f.dc < 0.01, "{f:?}");
    }
}
