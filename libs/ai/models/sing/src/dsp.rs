//! Signal conventions and the fixed (parameter-free) DSP of Cantor: the STFT
//! and its inverse with their adjoints, the mel filterbank, the
//! harmonic-plus-noise source, f0 extraction, resampling and WAV I/O.

use crate::fft::Fft;

pub const SR: u32 = 48_000;
/// 10 ms: 100 frames per second.
pub const HOP: usize = 480;
pub const N_FFT: usize = 2048;
pub const WIN: usize = 1920;
pub const BINS: usize = N_FFT / 2 + 1;
pub const N_MEL: usize = 128;
pub const MEL_FMIN: f32 = 40.0;
pub const MEL_FMAX: f32 = 16_000.0;
/// The highest harmonic the source makes.
pub const HARMONIC_TOP_HZ: f32 = 20_000.0;
pub const LOG_FLOOR: f32 = 1e-5;
/// f0 is carried as ln(f0 / F0_REF).
pub const F0_REF: f32 = 220.0;

pub fn midi_to_hz(m: f32) -> f32 {
    440.0 * 2f32.powf((m - 69.0) / 12.0)
}

pub fn hz_to_midi(hz: f32) -> f32 {
    69.0 + 12.0 * (hz / 440.0).log2()
}

/// A periodic Hann window of `win` samples, centred in `n_fft` (zeros around).
pub fn window(win: usize, n_fft: usize) -> Vec<f32> {
    let mut w = vec![0.0; n_fft];
    let off = (n_fft - win) / 2;
    for i in 0..win {
        w[off + i] = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / win as f32).cos();
    }
    w
}

/// The STFT geometry: frames centred on `t * hop`, zeros outside the signal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stft {
    pub n_fft: usize,
    pub hop: usize,
    pub win: usize,
}

impl Stft {
    pub const MAIN: Stft = Stft { n_fft: N_FFT, hop: HOP, win: WIN };

    pub fn bins(&self) -> usize {
        self.n_fft / 2 + 1
    }

    pub fn frames(&self, len: usize) -> usize {
        len / self.hop + 1
    }

    fn frame_start(&self, t: usize) -> isize {
        (t * self.hop) as isize - (self.n_fft / 2) as isize
    }

    /// Complex spectrum, `[frames, bins]` row-major re and im.
    pub fn forward(&self, x: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let fft = Fft::plan(self.n_fft);
        let w = window(self.win, self.n_fft);
        let frames = self.frames(x.len());
        let bins = self.bins();
        let mut re = vec![0.0; frames * bins];
        let mut im = vec![0.0; frames * bins];
        let mut buf = vec![0.0; self.n_fft];
        for t in 0..frames {
            let s = self.frame_start(t);
            for n in 0..self.n_fft {
                let i = s + n as isize;
                buf[n] = if i >= 0 && (i as usize) < x.len() { x[i as usize] * w[n] } else { 0.0 };
            }
            fft.rfft(&buf, &mut re[t * bins..(t + 1) * bins], &mut im[t * bins..(t + 1) * bins]);
        }
        (re, im)
    }

    /// Adjoint of `forward` (given d/d re and d/d im, the gradient on x).
    pub fn forward_adjoint(&self, dre: &[f32], dim: &[f32], len: usize) -> Vec<f32> {
        let fft = Fft::plan(self.n_fft);
        let w = window(self.win, self.n_fft);
        let frames = self.frames(len);
        let bins = self.bins();
        let n = self.n_fft as f32;
        let mut dx = vec![0.0; len];
        let mut buf = vec![0.0; self.n_fft];
        let mut zr = vec![0.0; bins];
        let mut zi = vec![0.0; bins];
        for t in 0..frames {
            // d/da[n] = sum_f dRe_f cos(2pi f n/N) - dIm_f sin(2pi f n/N) = Re sum_f Z_f e^{+i..}
            // with Z = dRe + i dIm; irfft computes (1/N)(Z0 + ZN/2 (-1)^n + 2 Re sum ...).
            for f in 0..bins {
                let c = if f == 0 || f == bins - 1 { n } else { n / 2.0 };
                zr[f] = dre[t * bins + f] * c;
                zi[f] = dim[t * bins + f] * c;
            }
            fft.irfft(&zr, &zi, &mut buf);
            let s = self.frame_start(t);
            for k in 0..self.n_fft {
                let i = s + k as isize;
                if i >= 0 && (i as usize) < len {
                    dx[i as usize] += buf[k] * w[k];
                }
            }
        }
        dx
    }

    pub fn overlap_norm(&self, frames: usize, len: usize) -> Vec<f32> {
        let w = window(self.win, self.n_fft);
        let mut d = vec![0.0; len];
        for t in 0..frames {
            let s = self.frame_start(t);
            for k in 0..self.n_fft {
                let i = s + k as isize;
                if i >= 0 && (i as usize) < len {
                    d[i as usize] += w[k] * w[k];
                }
            }
        }
        for v in &mut d {
            *v = 1.0 / v.max(1e-3);
        }
        d
    }

    /// Weighted overlap-add inverse: exact inverse of `forward` inside the signal.
    pub fn inverse(&self, re: &[f32], im: &[f32], frames: usize, len: usize) -> Vec<f32> {
        let fft = Fft::plan(self.n_fft);
        let w = window(self.win, self.n_fft);
        let bins = self.bins();
        let norm = self.overlap_norm(frames, len);
        let mut y = vec![0.0; len];
        let mut buf = vec![0.0; self.n_fft];
        for t in 0..frames {
            fft.irfft(&re[t * bins..(t + 1) * bins], &im[t * bins..(t + 1) * bins], &mut buf);
            let s = self.frame_start(t);
            for k in 0..self.n_fft {
                let i = s + k as isize;
                if i >= 0 && (i as usize) < len {
                    y[i as usize] += buf[k] * w[k];
                }
            }
        }
        for (v, n) in y.iter_mut().zip(&norm) {
            *v *= n;
        }
        y
    }

    /// Adjoint of `inverse`: the gradient on re and im given d/dy.
    pub fn inverse_adjoint(&self, dy: &[f32], frames: usize) -> (Vec<f32>, Vec<f32>) {
        let fft = Fft::plan(self.n_fft);
        let w = window(self.win, self.n_fft);
        let bins = self.bins();
        let len = dy.len();
        let norm = self.overlap_norm(frames, len);
        let n = self.n_fft as f32;
        let mut dre = vec![0.0; frames * bins];
        let mut dim = vec![0.0; frames * bins];
        let mut buf = vec![0.0; self.n_fft];
        let mut gr = vec![0.0; bins];
        let mut gi = vec![0.0; bins];
        for t in 0..frames {
            let s = self.frame_start(t);
            for k in 0..self.n_fft {
                let i = s + k as isize;
                buf[k] = if i >= 0 && (i as usize) < len { dy[i as usize] * norm[i as usize] * w[k] } else { 0.0 };
            }
            // irfft(Y)[k] = (1/N)(c_f Re(Y_f e^{+i})) summed: d/dRe_f = (c_f/N) Re(rfft(g)_f),
            // d/dIm_f = (c_f/N) Im(rfft(g)_f) (zero at DC and Nyquist).
            fft.rfft(&buf, &mut gr, &mut gi);
            for f in 0..bins {
                let edge = f == 0 || f == bins - 1;
                let c = if edge { 1.0 / n } else { 2.0 / n };
                dre[t * bins + f] = gr[f] * c;
                dim[t * bins + f] = if edge { 0.0 } else { gi[f] * c };
            }
        }
        (dre, dim)
    }
}

/// Triangular mel filters on the HTK mel scale, area-normalised: `[N_MEL, BINS]`.
pub fn mel_filters() -> Vec<f32> {
    mel_filters_for(N_MEL, N_FFT, SR as f32, MEL_FMIN, MEL_FMAX)
}

pub fn mel_filters_for(n_mel: usize, n_fft: usize, sr: f32, fmin: f32, fmax: f32) -> Vec<f32> {
    let bins = n_fft / 2 + 1;
    let mel = |f: f32| 2595.0 * (1.0 + f / 700.0).log10();
    let hz = |m: f32| 700.0 * (10f32.powf(m / 2595.0) - 1.0);
    let (m0, m1) = (mel(fmin), mel(fmax));
    let pts: Vec<f32> = (0..n_mel + 2).map(|i| hz(m0 + (m1 - m0) * i as f32 / (n_mel + 1) as f32)).collect();
    let mut w = vec![0.0; n_mel * bins];
    for m in 0..n_mel {
        let (lo, c, hi) = (pts[m], pts[m + 1], pts[m + 2]);
        let norm = 2.0 / (hi - lo);
        for b in 0..bins {
            let f = b as f32 * sr / n_fft as f32;
            let v = if f > lo && f <= c {
                (f - lo) / (c - lo)
            } else if f > c && f < hi {
                (hi - f) / (hi - c)
            } else {
                0.0
            };
            w[m * bins + b] = v * norm * 200.0;
        }
    }
    w
}

/// Log-mel spectrogram of 48 kHz audio: `[frames, N_MEL]` row-major.
pub fn log_mel(x: &[f32]) -> Vec<f32> {
    let st = Stft::MAIN;
    let (re, im) = st.forward(x);
    let frames = st.frames(x.len());
    let fb = mel_filters();
    let mut out = vec![0.0; frames * N_MEL];
    for t in 0..frames {
        let row = t * BINS;
        for m in 0..N_MEL {
            let mut acc = 0.0;
            let f = &fb[m * BINS..(m + 1) * BINS];
            for b in 0..BINS {
                if f[b] != 0.0 {
                    acc += f[b] * (re[row + b] * re[row + b] + im[row + b] * im[row + b]).sqrt();
                }
            }
            out[t * N_MEL + m] = acc.max(LOG_FLOOR).ln();
        }
    }
    out
}

/// A small deterministic RNG (SplitMix64) for noise, init and sampling.
#[derive(Clone, Debug)]
pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn normal(&mut self) -> f32 {
        let u1 = self.unit().max(1e-7);
        let u2 = self.unit();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
}

/// Level of every harmonic and of the noise in the source (the vocoder's
/// filter sets the real levels).
pub const SOURCE_LEVEL: f32 = 0.1;

/// Per-sample f0 (Hz, 0 unvoiced) from frame f0: log-linear between voiced
/// frames, and a voicing amplitude that ramps over one hop.
pub fn upsample_f0(f0: &[f32], len: usize) -> (Vec<f32>, Vec<f32>) {
    let mut hz = vec![0.0; len];
    let mut amp = vec![0.0; len];
    for i in 0..len {
        let pos = i as f32 / HOP as f32;
        let t0 = (pos.floor() as usize).min(f0.len().saturating_sub(1));
        let t1 = (t0 + 1).min(f0.len().saturating_sub(1));
        let fr = pos - t0 as f32;
        let (a, b) = (f0[t0], f0[t1]);
        let va = if a > 0.0 { 1.0 } else { 0.0 };
        let vb = if b > 0.0 { 1.0 } else { 0.0 };
        amp[i] = va * (1.0 - fr) + vb * fr;
        hz[i] = match (a > 0.0, b > 0.0) {
            (true, true) => (a.ln() * (1.0 - fr) + b.ln() * fr).exp(),
            (true, false) => a,
            (false, true) => b,
            _ => 0.0,
        };
    }
    (hz, amp)
}

/// Per-sample source controls for an f0 curve: phase of the fundamental
/// (wrapped to [0, 2pi)), frequency, voicing amplitude.
pub fn source_controls(f0: &[f32], len: usize) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let (hz, amp) = upsample_f0(f0, len);
    let sr = SR as f64;
    let mut ph = vec![0.0; len];
    let mut phase = 0.0f64;
    let two_pi = 2.0 * std::f64::consts::PI;
    for i in 0..len {
        if hz[i] > 0.0 {
            phase += two_pi * hz[i] as f64 / sr;
            if phase >= two_pi {
                phase -= two_pi * (phase / two_pi).floor();
            }
        }
        ph[i] = phase as f32;
    }
    (ph, hz, amp)
}

/// The sum of harmonics for one sample (the source's inner loop; the GPU
/// kernel computes the same).
#[inline]
pub fn harmonic_sample(ph: f32, f: f32, amp: f32) -> f32 {
    if amp <= 0.0 || f <= 0.0 {
        return 0.0;
    }
    let taper_lo = HARMONIC_TOP_HZ * 0.8;
    let count = (HARMONIC_TOP_HZ / f) as usize;
    // sin(k ph) by the Chebyshev recurrence.
    let c2 = 2.0 * ph.cos();
    let (mut s_prev, mut s) = (0.0f32, ph.sin());
    let mut acc = 0.0;
    for k in 1..=count {
        let fk = k as f32 * f;
        let g = if fk > taper_lo { (HARMONIC_TOP_HZ - fk) / (HARMONIC_TOP_HZ - taper_lo) } else { 1.0 };
        acc += s * g;
        let next = c2 * s - s_prev;
        s_prev = s;
        s = next;
    }
    acc * SOURCE_LEVEL * amp
}

/// The harmonic source: every harmonic of f0 below `HARMONIC_TOP_HZ` at equal
/// level, phase-continuous, so the output pitch is exactly f0.
pub fn harmonic_source(f0: &[f32], len: usize) -> Vec<f32> {
    let (ph, hz, amp) = source_controls(f0, len);
    (0..len).map(|i| harmonic_sample(ph[i], hz[i], amp[i])).collect()
}

pub fn noise_source(len: usize, seed: u64) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    (0..len).map(|_| rng.normal() * SOURCE_LEVEL).collect()
}

/// f0 by YIN on a 16 kHz copy: one value per 10 ms frame (0 = unvoiced).
pub fn f0_yin(x48: &[f32], fmin: f32, fmax: f32) -> Vec<f32> {
    let mut x = resample(x48, SR, 16_000);
    let sr = 16_000.0f32;
    // Two one-pole low-passes at 1.2 kHz: YIN's integer lags misjudge very
    // bright (pulse-like) sources, and the fundamental is all it needs.
    let a = (-2.0 * std::f32::consts::PI * 1200.0 / sr).exp();
    for _ in 0..2 {
        let mut z = 0.0;
        for v in x.iter_mut() {
            z = (1.0 - a) * *v + a * z;
            *v = z;
        }
    }
    let frames = x48.len() / HOP + 1;
    let hop = 160usize;
    let w = 640usize;
    let max_lag = (sr / fmin) as usize;
    let min_lag = (sr / fmax).max(2.0) as usize;
    let mut out = vec![0.0; frames];
    let mut d = vec![0.0f32; max_lag + 2];
    let mut buf: Vec<f32> = Vec::new();
    for t in 0..frames {
        let c = t * hop;
        let start = c as isize - (w / 2) as isize;
        // The frame and its lagged continuation, zero-padded, in one slice.
        let span = w + max_lag + 2;
        buf.clear();
        buf.extend((0..span).map(|k| {
            let i = start + k as isize;
            if i >= 0 && (i as usize) < x.len() { x[i as usize] } else { 0.0 }
        }));
        let energy: f32 = buf[..w].iter().map(|v| v * v).sum::<f32>() / w as f32;
        if energy < 1e-7 {
            continue;
        }
        d[0] = 0.0;
        let mut cum = 0.0;
        let mut best = None;
        let a = &buf[..w];
        for lag in 1..=max_lag + 1 {
            let b = &buf[lag..lag + w];
            let mut s = 0.0;
            for k in 0..w {
                let e = a[k] - b[k];
                s += e * e;
            }
            cum += s;
            d[lag] = if cum > 0.0 { s * lag as f32 / cum } else { 1.0 };
        }
        for lag in min_lag..=max_lag {
            if d[lag] < 0.15 {
                let mut l = lag;
                while l < max_lag && d[l + 1] < d[l] {
                    l += 1;
                }
                best = Some(l);
                break;
            }
        }
        if let Some(l) = best {
            let (a, b, cc) = (d[l - 1], d[l], d[l + 1]);
            let den = a - 2.0 * b + cc;
            let shift = if den.abs() > 1e-9 { 0.5 * (a - cc) / den } else { 0.0 };
            out[t] = sr / (l as f32 + shift.clamp(-1.0, 1.0));
        }
    }
    median_voiced(&out, 2)
}

/// A median filter over voiced frames (radius r), leaving unvoiced ones.
fn median_voiced(f0: &[f32], r: usize) -> Vec<f32> {
    let mut out = f0.to_vec();
    for t in 0..f0.len() {
        if f0[t] <= 0.0 {
            continue;
        }
        let mut v: Vec<f32> = f0[t.saturating_sub(r)..(t + r + 1).min(f0.len())].iter().copied().filter(|x| *x > 0.0).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        out[t] = v[v.len() / 2];
    }
    out
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Windowed-sinc resampling (Hann-windowed, 24 zero crossings), any ratio.
/// Rational ratios with few phases use a precomputed polyphase table.
pub fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to {
        return x.to_vec();
    }
    let ratio = to as f64 / from as f64;
    let out_len = (x.len() as f64 * ratio).round() as usize;
    let cutoff = ratio.min(1.0);
    let zc = 24.0;
    let half = (zc / cutoff).ceil() as isize;
    let taps = (2 * half) as usize;
    let kernel = |t: f64| -> f64 {
        // t in input samples from the output position.
        let u = t * cutoff;
        if u.abs() >= zc {
            return 0.0;
        }
        let sinc = if u.abs() < 1e-9 { 1.0 } else { (std::f64::consts::PI * u).sin() / (std::f64::consts::PI * u) };
        sinc * (0.5 + 0.5 * (std::f64::consts::PI * u / zc).cos()) * cutoff
    };
    let g = gcd(from as u64, to as u64);
    let phases = (to as u64 / g) as usize;
    let step = from as u64 / g;
    let mut out = vec![0.0; out_len];
    if phases <= 4096 {
        // Output j sits at input position j*step/phases: integer part and phase.
        let mut table = vec![0.0f32; phases * taps];
        for p in 0..phases {
            let frac = p as f64 / phases as f64;
            for k in 0..taps {
                let i = k as isize - half + 1; // relative input index
                table[p * taps + k] = kernel(frac - i as f64) as f32;
            }
        }
        for (j, o) in out.iter_mut().enumerate() {
            let num = j as u64 * step;
            let c = (num / phases as u64) as isize;
            let p = (num % phases as u64) as usize;
            let row = &table[p * taps..(p + 1) * taps];
            let mut acc = 0.0f32;
            let start = c - half + 1;
            if start >= 0 && (start as usize + taps) <= x.len() {
                let xs = &x[start as usize..start as usize + taps];
                for k in 0..taps {
                    acc += xs[k] * row[k];
                }
            } else {
                for k in 0..taps {
                    let i = start + k as isize;
                    if i >= 0 && (i as usize) < x.len() {
                        acc += x[i as usize] * row[k];
                    }
                }
            }
            *o = acc;
        }
        return out;
    }
    for (j, o) in out.iter_mut().enumerate() {
        let pos = j as f64 / ratio;
        let c = pos.floor() as isize;
        let mut acc = 0.0f64;
        for i in (c - half + 1)..=(c + half) {
            if i < 0 || i as usize >= x.len() {
                continue;
            }
            acc += x[i as usize] as f64 * kernel(pos - i as f64);
        }
        *o = acc as f32;
    }
    out
}

/// Mono 16-bit PCM WAV.
pub fn wav_bytes(rate: u32, x: &[f32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(44 + x.len() * 2);
    let data = (x.len() * 2) as u32;
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for v in x {
        b.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    b
}

/// Any PCM (16/24/32-bit int, 32-bit float) WAV, mixed to mono: (samples, rate).
pub fn read_wav(bytes: &[u8]) -> Option<(Vec<f32>, u32)> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
    let mut i = 12;
    let (mut fmt, mut chans, mut rate, mut bits) = (0u16, 0u16, 0u32, 0u16);
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let size = u32_at(i + 4) as usize;
        let body = i + 8;
        if id == b"fmt " {
            fmt = u16_at(body);
            chans = u16_at(body + 2);
            rate = u32_at(body + 4);
            bits = u16_at(body + 14);
            if fmt == 0xFFFE && size >= 26 {
                fmt = u16_at(body + 24);
            }
        } else if id == b"data" {
            let end = (body + size).min(bytes.len());
            let data = &bytes[body..end];
            let bps = (bits / 8) as usize;
            let ch = chans.max(1) as usize;
            if bps == 0 {
                return None;
            }
            let frames = data.len() / (bps * ch);
            let mut out = Vec::with_capacity(frames);
            for f in 0..frames {
                let mut acc = 0.0;
                for c in 0..ch {
                    let o = (f * ch + c) * bps;
                    let s = &data[o..o + bps];
                    acc += match (fmt, bits) {
                        (1, 16) => i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0,
                        (1, 24) => (((s[0] as i32) << 8 | (s[1] as i32) << 16 | (s[2] as i32) << 24) >> 8) as f32 / 8_388_608.0,
                        (1, 32) => i32::from_le_bytes(s.try_into().unwrap()) as f32 / 2_147_483_648.0,
                        (3, 32) => f32::from_le_bytes(s.try_into().unwrap()),
                        _ => return None,
                    };
                }
                out.push(acc / ch as f32);
            }
            return Some((out, rate));
        }
        i = body + size + (size & 1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn istft_inverts_stft() {
        let mut rng = Rng::new(1);
        let x: Vec<f32> = (0..9000).map(|_| rng.normal() * 0.3).collect();
        let st = Stft::MAIN;
        let (re, im) = st.forward(&x);
        let y = st.inverse(&re, &im, st.frames(x.len()), x.len());
        let err = x.iter().zip(&y).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(err < 1e-4, "istft error {err}");
    }

    #[test]
    fn adjoints_match_dot_products() {
        // <A x, y> == <x, A^T y> for the STFT and the iSTFT.
        let st = Stft { n_fft: 256, hop: 64, win: 240 };
        let mut rng = Rng::new(2);
        let len = 1000;
        let frames = st.frames(len);
        let bins = st.bins();
        let x: Vec<f32> = (0..len).map(|_| rng.normal()).collect();
        let yr: Vec<f32> = (0..frames * bins).map(|_| rng.normal()).collect();
        let yi: Vec<f32> = (0..frames * bins).map(|_| rng.normal()).collect();
        let (xr, xi) = st.forward(&x);
        let lhs: f64 = xr.iter().zip(&yr).chain(xi.iter().zip(&yi)).map(|(a, b)| (*a * *b) as f64).sum();
        let at = st.forward_adjoint(&yr, &yi, len);
        let rhs: f64 = x.iter().zip(&at).map(|(a, b)| (*a * *b) as f64).sum();
        assert!((lhs - rhs).abs() < 1e-2 * lhs.abs().max(1.0), "stft adjoint {lhs} vs {rhs}");

        let mut yi_edge = yi.clone();
        for t in 0..frames {
            yi_edge[t * bins] = 0.0;
            yi_edge[t * bins + bins - 1] = 0.0;
        }
        let inv = st.inverse(&yr, &yi_edge, frames, len);
        let lhs: f64 = inv.iter().zip(&x).map(|(a, b)| (*a * *b) as f64).sum();
        let (ar, ai) = st.inverse_adjoint(&x, frames);
        let rhs: f64 = ar.iter().zip(&yr).chain(ai.iter().zip(&yi_edge)).map(|(a, b)| (*a * *b) as f64).sum();
        assert!((lhs - rhs).abs() < 1e-2 * lhs.abs().max(1.0), "istft adjoint {lhs} vs {rhs}");
    }

    #[test]
    fn harmonic_source_has_exact_pitch_and_yin_finds_it() {
        let f0 = vec![233.08; 101];
        let x = harmonic_source(&f0, 48_000);
        let est = f0_yin(&x, 55.0, 1400.0);
        let mid = &est[20..80];
        for v in mid {
            let cents = 1200.0 * (v / 233.08).log2();
            assert!(cents.abs() < 5.0, "yin {v} Hz");
        }
    }

    #[test]
    fn resampling_keeps_a_sine() {
        for (from, to) in [(24_000u32, 48_000u32), (44_100, 48_000), (48_000, 16_000)] {
            let f = 440.0;
            let x: Vec<f32> = (0..from as usize / 2).map(|i| (2.0 * std::f32::consts::PI * f * i as f32 / from as f32).sin()).collect();
            let y = resample(&x, from, to);
            let mid = y.len() / 2;
            let err = (mid - 500..mid + 500)
                .map(|i| (y[i] - (2.0 * std::f32::consts::PI * f * i as f32 / to as f32).sin()).abs())
                .fold(0.0f32, f32::max);
            assert!(err < 2e-3, "{from}->{to}: error {err}");
        }
    }

    #[test]
    fn wav_round_trip_and_resample_length() {
        let x: Vec<f32> = (0..480).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let (y, rate) = read_wav(&wav_bytes(48_000, &x)).unwrap();
        assert_eq!(rate, 48_000);
        assert!(x.iter().zip(&y).all(|(a, b)| (a - b).abs() < 1e-4));
        assert_eq!(resample(&x, 24_000, 48_000).len(), 960);
    }
}
