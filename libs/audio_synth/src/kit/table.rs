//! Wavetables: up to [`MAX_FRAMES`] single-cycle frames of [`FRAME`]
//! samples, each kept band-limited at [`LEVELS`] octaves (an oscillator
//! reads the level whose highest partial stays under Nyquist), and each
//! frame's partials (magnitude and phase) for the spectral oscillator.
//!
//! The built-in tables are made here from their recipe (additive spectra or
//! a drawn cycle), once, on first use; a score's own tables are made from
//! partial lists (`a_frames`) or cut from a sample (`@import`). Building
//! allocates and runs FFTs: never on the audio thread.

use std::f32::consts::{PI, TAU};
use std::sync::{Arc, OnceLock};

pub const FRAME: usize = 2048;
/// Band-limited copies: level `l` keeps partials up to `FRAME / 2 >> l`.
pub const LEVELS: usize = 11;
/// Partials kept per frame for spectral resynthesis.
pub const SPEC: usize = 128;
pub const MAX_FRAMES: usize = 64;
/// Frames of the built-in morphing tables.
const BUILT_FRAMES: usize = 32;

pub struct Table {
    pub frames: usize,
    /// `[level][frame][FRAME + 1]` (the last sample repeats the first).
    data: Vec<f32>,
    /// `[frame][SPEC]` partial magnitudes (partial 1 first) and phases (turns).
    pub mags: Vec<f32>,
    pub phases: Vec<f32>,
}

impl Table {
    /// The level to read for a phase step of `inc` cycles a sample.
    #[inline]
    pub fn level_for(inc: f32) -> usize {
        // Partials kept at level l: 1024 >> l; need 1024 >> l <= 0.5 / inc.
        let need = (FRAME as f32 * inc.abs()).max(1.0);
        (need.log2().ceil().max(0.0) as usize).min(LEVELS - 1)
    }

    #[inline]
    fn at(&self, level: usize, frame: usize, phase: f32) -> f32 {
        let base = (level * self.frames + frame) * (FRAME + 1);
        let x = phase * FRAME as f32;
        let i = (x as usize).min(FRAME - 1);
        let f = x - i as f32;
        let a = self.data[base + i];
        let b = self.data[base + i + 1];
        a + (b - a) * f
    }

    /// The table at `pos` (0..1 over its frames) and `phase` (0..1), blended
    /// between neighbouring frames when `smooth`.
    #[inline]
    pub fn read(&self, level: usize, pos: f32, smooth: bool, phase: f32) -> f32 {
        let phase = phase - phase.floor();
        if self.frames <= 1 {
            return self.at(level, 0, phase);
        }
        let fp = pos.clamp(0.0, 1.0) * (self.frames - 1) as f32;
        if !smooth {
            return self.at(level, fp.round() as usize, phase);
        }
        let i = (fp as usize).min(self.frames - 2);
        let t = fp - i as f32;
        let a = self.at(level, i, phase);
        if t <= 0.0 {
            return a;
        }
        a + (self.at(level, i + 1, phase) - a) * t
    }

    /// Partial `k` (0 = the fundamental) at `pos`: (magnitude, phase).
    #[inline]
    pub fn partial(&self, pos: f32, k: usize) -> (f32, f32) {
        if k >= SPEC {
            return (0.0, 0.0);
        }
        let fp = pos.clamp(0.0, 1.0) * (self.frames.max(1) - 1) as f32;
        let i = (fp as usize).min(self.frames.saturating_sub(2));
        let t = if self.frames > 1 { fp - i as f32 } else { 0.0 };
        let a = self.mags[i * SPEC + k];
        let pa = self.phases[i * SPEC + k];
        if self.frames <= 1 {
            return (a, pa);
        }
        let b = self.mags[(i + 1) * SPEC + k];
        (a + (b - a) * t, if t < 0.5 { pa } else { self.phases[(i + 1) * SPEC + k] })
    }

    /// One frame at full bandwidth, for pictures (`out.len()` points).
    pub fn picture(&self, pos: f32, out: &mut [f32]) {
        let n = out.len().max(1);
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.read(0, pos, true, i as f32 / n as f32);
        }
    }

    /// A table from time-domain cycles (any length; resampled to [`FRAME`]).
    pub fn from_cycles(cycles: &[Vec<f32>]) -> Table {
        let frames = cycles.len().clamp(1, MAX_FRAMES);
        let mut spectra = Vec::with_capacity(frames);
        for c in cycles.iter().take(frames) {
            let mut re = resample(c, FRAME);
            let mut im = vec![0.0; FRAME];
            fft(&mut re, &mut im, false);
            spectra.push((re, im));
        }
        if spectra.is_empty() {
            let re = vec![0.0; FRAME];
            let mut im = vec![0.0; FRAME];
            im[1] = -(FRAME as f32) / 2.0;
            spectra.push((re, im));
        }
        Table::from_spectra(spectra)
    }

    /// A table from partial amplitudes, one list a frame (partial 1 first;
    /// sine phase).
    pub fn from_partials(frames: &[Vec<f32>]) -> Table {
        let spectra = frames.iter().take(MAX_FRAMES).map(|amps| {
            let re = vec![0.0; FRAME];
            let mut im = vec![0.0; FRAME];
            for (k, a) in amps.iter().enumerate().take(FRAME / 2 - 1) {
                // A sine of amplitude a at bin k+1.
                im[k + 1] = -a * FRAME as f32 / 2.0;
                im[FRAME - k - 1] = a * FRAME as f32 / 2.0;
            }
            (re, im)
        }).collect::<Vec<_>>();
        if spectra.is_empty() {
            return Table::from_partials(&[vec![1.0]]);
        }
        Table::from_spectra(spectra)
    }

    /// Frames cut from a sample: `pitch` takes one period of the detected
    /// pitch per frame, else fixed [`FRAME`]-sample windows; spread over the
    /// sample, at most `MAX_FRAMES`.
    pub fn from_sample(sample: &[f32], rate: f32, pitch: bool) -> Table {
        if sample.len() < 64 {
            return Table::from_partials(&[vec![1.0]]);
        }
        let period = if pitch { detect_period(sample, rate) } else { FRAME };
        let period = period.clamp(32, sample.len());
        let count = (sample.len() / period).clamp(1, MAX_FRAMES);
        let step = if count > 1 { (sample.len() - period) / (count - 1) } else { 0 };
        let cycles: Vec<Vec<f32>> = (0..count).map(|i| {
            let start = i * step;
            // Each cut cross-faded with its own start so the cycle loops cleanly.
            let mut c: Vec<f32> = sample[start..start + period].to_vec();
            let fade = (period / 16).max(1);
            for j in 0..fade {
                let t = j as f32 / fade as f32;
                let tail = period - fade + j;
                c[tail] = c[tail] * (1.0 - t) + sample[start + j] * t;
            }
            c
        }).collect();
        Table::from_cycles(&cycles)
    }

    fn from_spectra(spectra: Vec<(Vec<f32>, Vec<f32>)>) -> Table {
        let frames = spectra.len();
        let mut data = vec![0.0f32; LEVELS * frames * (FRAME + 1)];
        let mut mags = vec![0.0f32; frames * SPEC];
        let mut phases = vec![0.0f32; frames * SPEC];
        // One gain for the whole table: its loudest full-band frame peaks at 1.
        let mut peak = 0.0f32;
        let mut full: Vec<Vec<f32>> = Vec::with_capacity(frames);
        for (re, im) in &spectra {
            let (mut r, mut i) = (re.clone(), im.clone());
            r[0] = 0.0;
            i[0] = 0.0;
            fft(&mut r, &mut i, true);
            peak = peak.max(r.iter().fold(0.0f32, |m, x| m.max(x.abs())));
            full.push(r);
        }
        let gain = if peak > 1e-9 { 1.0 / peak } else { 1.0 };
        for (f, (re, im)) in spectra.iter().enumerate() {
            for k in 0..SPEC {
                let (a, b) = (re[k + 1], im[k + 1]);
                mags[f * SPEC + k] = (a * a + b * b).sqrt() * 2.0 / FRAME as f32 * gain;
                // A sine at phase p: re = sin(p)·N/2·…; the oscillator plays sin(2π(k t + phase)).
                phases[f * SPEC + k] = (a.atan2(-b) / TAU).rem_euclid(1.0);
            }
            for l in 0..LEVELS {
                let keep = (FRAME / 2) >> l;
                let base = (l * frames + f) * (FRAME + 1);
                let out = &mut data[base..base + FRAME + 1];
                if l == 0 {
                    for (o, x) in out.iter_mut().zip(full[f].iter()) {
                        *o = x * gain;
                    }
                } else {
                    let (mut r, mut i) = (re.clone(), im.clone());
                    r[0] = 0.0;
                    i[0] = 0.0;
                    for k in keep..=FRAME / 2 {
                        r[k] = 0.0;
                        i[k] = 0.0;
                        r[(FRAME - k) % FRAME] = 0.0;
                        i[(FRAME - k) % FRAME] = 0.0;
                    }
                    fft(&mut r, &mut i, true);
                    for (o, x) in out.iter_mut().zip(r.iter()) {
                        *o = x * gain;
                    }
                }
                out[FRAME] = out[0];
            }
        }
        Table { frames, data, mags, phases }
    }
}

/// In-place radix-2 complex FFT (`re.len()` a power of two); the inverse
/// divides by the length.
pub fn fft(re: &mut [f32], im: &mut [f32], inverse: bool) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && im.len() == n);
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
    let sign = if inverse { 1.0f64 } else { -1.0 };
    let mut len = 2;
    while len <= n {
        let ang = sign * std::f64::consts::TAU / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        let mut i = 0;
        while i < n {
            let (mut cr, mut ci) = (1.0f64, 0.0f64);
            for k in 0..len / 2 {
                let (a, b) = (i + k, i + k + len / 2);
                let (xr, xi) = (re[b] as f64 * cr - im[b] as f64 * ci, re[b] as f64 * ci + im[b] as f64 * cr);
                re[b] = (re[a] as f64 - xr) as f32;
                im[b] = (im[a] as f64 - xi) as f32;
                re[a] = (re[a] as f64 + xr) as f32;
                im[a] = (im[a] as f64 + xi) as f32;
                let t = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = t;
            }
            i += len;
        }
        len <<= 1;
    }
    if inverse {
        let s = 1.0 / n as f32;
        for (r, i) in re.iter_mut().zip(im.iter_mut()) {
            *r *= s;
            *i *= s;
        }
    }
}

fn resample(c: &[f32], n: usize) -> Vec<f32> {
    if c.len() == n {
        return c.to_vec();
    }
    if c.is_empty() {
        return vec![0.0; n];
    }
    (0..n).map(|i| {
        let x = i as f32 * c.len() as f32 / n as f32;
        let a = x as usize % c.len();
        let b = (a + 1) % c.len();
        let t = x - x.floor();
        c[a] + (c[b] - c[a]) * t
    }).collect()
}

/// The strongest period (samples) of a sound, by autocorrelation over
/// 40..1000 Hz of its middle.
fn detect_period(s: &[f32], rate: f32) -> usize {
    let lo = (rate / 1000.0) as usize;
    let hi = ((rate / 40.0) as usize).min(s.len() / 2);
    if hi <= lo + 1 {
        return FRAME;
    }
    let mid = s.len() / 2;
    let win = (hi * 2).min(s.len() - mid).min(4096);
    let seg = &s[mid.saturating_sub(win / 2)..];
    let mut corr = Vec::with_capacity(hi - lo);
    for lag in lo..hi {
        let n = win.min(seg.len().saturating_sub(lag));
        if n < 16 {
            break;
        }
        let mut acc = 0.0f32;
        for i in 0..n {
            acc += seg[i] * seg[i + lag];
        }
        corr.push(acc / n as f32);
    }
    let top = corr.iter().fold(0.0f32, |m, c| m.max(*c));
    if top <= 0.0 {
        return FRAME;
    }
    // The shortest lag that is a peak near the best (not a multiple of it).
    for i in 1..corr.len().saturating_sub(1) {
        if corr[i] >= 0.9 * top && corr[i] >= corr[i - 1] && corr[i] >= corr[i + 1] {
            return lo + i;
        }
    }
    FRAME
}

// ---- the built-in tables -----------------------------------------------------

fn cycle(f: impl Fn(f32) -> f32) -> Vec<f32> {
    (0..FRAME).map(|i| f(i as f32 / FRAME as f32)).collect()
}

fn morph_cycles(keys: &[Vec<f32>], frames: usize) -> Vec<Vec<f32>> {
    (0..frames).map(|f| {
        let x = f as f32 / (frames - 1) as f32 * (keys.len() - 1) as f32;
        let i = (x as usize).min(keys.len() - 2);
        let t = x - i as f32;
        keys[i].iter().zip(keys[i + 1].iter()).map(|(a, b)| a + (b - a) * t).collect()
    }).collect()
}

/// A resonance's gain at `hz` (formant `f`, bandwidth `bw`).
fn formant_gain(hz: f32, f: f32, bw: f32) -> f32 {
    let d = (hz - f) / bw;
    1.0 / (1.0 + d * d)
}

fn vowel_partials(formants: [(f32, f32); 3], f0: f32) -> Vec<f32> {
    (1..=96).map(|n| {
        let hz = n as f32 * f0;
        let g: f32 = formants.iter().enumerate().map(|(i, (f, bw))| formant_gain(hz, *f, *bw) * [1.0, 0.6, 0.3][i]).sum();
        g / (n as f32).sqrt()
    }).collect()
}

fn lerp_formants(a: [(f32, f32); 3], b: [(f32, f32); 3], t: f32) -> [(f32, f32); 3] {
    let mut o = a;
    for i in 0..3 {
        o[i] = (a[i].0 + (b[i].0 - a[i].0) * t, a[i].1 + (b[i].1 - a[i].1) * t);
    }
    o
}

fn vowel_table(vowels: &[[(f32, f32); 3]], f0: f32) -> Table {
    let frames: Vec<Vec<f32>> = (0..BUILT_FRAMES).map(|f| {
        let x = f as f32 / (BUILT_FRAMES - 1) as f32 * (vowels.len() - 1) as f32;
        let i = (x as usize).min(vowels.len() - 2);
        vowel_partials(lerp_formants(vowels[i], vowels[i + 1], x - i as f32), f0)
    }).collect();
    Table::from_partials(&frames)
}

fn hash(mut x: u32) -> f32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    (x as f32 / u32::MAX as f32).clamp(0.0, 1.0)
}

fn make(index: usize) -> Table {
    let n = BUILT_FRAMES;
    match index {
        // basic: sine > triangle > saw > square
        0 => {
            let keys = vec![
                cycle(|p| (p * TAU).sin()),
                cycle(|p| 1.0 - 4.0 * ((p + 0.25).fract() - 0.5).abs()),
                cycle(|p| 1.0 - 2.0 * p),
                cycle(|p| if p < 0.5 { 1.0 } else { -1.0 }),
            ];
            Table::from_cycles(&morph_cycles(&keys, n))
        }
        // pwm: square to a thin pulse
        1 => Table::from_cycles(&(0..n).map(|f| {
            let w = 0.5 - 0.47 * f as f32 / (n - 1) as f32;
            cycle(move |p| if p < w { 1.0 } else { -1.0 })
        }).collect::<Vec<_>>()),
        // harmonic: one partial more every frame or two, saw-weighted
        2 => Table::from_partials(&(0..n).map(|f| {
            let count = 1 + f * 2;
            (1..=count).map(|k| 1.0 / k as f32).collect()
        }).collect::<Vec<_>>()),
        // formant: a > e > i > o > u
        3 => vowel_table(&[
            [(730.0, 90.0), (1090.0, 110.0), (2440.0, 160.0)],
            [(530.0, 70.0), (1840.0, 100.0), (2480.0, 160.0)],
            [(270.0, 60.0), (2290.0, 100.0), (3010.0, 180.0)],
            [(570.0, 80.0), (840.0, 90.0), (2410.0, 160.0)],
            [(300.0, 60.0), (870.0, 90.0), (2240.0, 150.0)],
        ], 110.0),
        // fm: a sine modulating itself, index 0 to 8
        4 => Table::from_cycles(&(0..n).map(|f| {
            let index = 8.0 * f as f32 / (n - 1) as f32;
            cycle(move |p| (p * TAU + index * (p * TAU).sin()).sin())
        }).collect::<Vec<_>>()),
        // sync: a saw hard-synced from 1 to 8 times its pitch, windowed
        5 => Table::from_cycles(&(0..n).map(|f| {
            let ratio = 1.0 + 7.0 * f as f32 / (n - 1) as f32;
            cycle(move |p| (1.0 - 2.0 * (p * ratio).fract()) * (0.5 - 0.5 * (p * TAU).cos()).sqrt())
        }).collect::<Vec<_>>()),
        // organ: drawbar registrations over a 16' fundamental
        6 => {
            const BARS: [usize; 9] = [1, 3, 2, 4, 6, 8, 10, 12, 16];
            let regs: [[f32; 9]; 5] = [
                [8., 8., 8., 0., 0., 0., 0., 0., 0.],
                [8., 0., 8., 8., 0., 0., 0., 0., 8.],
                [6., 8., 8., 6., 0., 0., 0., 0., 0.],
                [8., 8., 8., 8., 8., 8., 8., 8., 8.],
                [0., 0., 8., 8., 8., 0., 0., 8., 8.],
            ];
            Table::from_partials(&(0..n).map(|f| {
                let x = f as f32 / (n - 1) as f32 * 4.0;
                let i = (x as usize).min(3);
                let t = x - i as f32;
                let mut amps = vec![0.0; 16];
                for (b, h) in BARS.iter().enumerate() {
                    let v = regs[i][b] + (regs[i + 1][b] - regs[i][b]) * t;
                    amps[h - 1] += 10f32.powf((v - 8.0) * 3.0 / 20.0) * if v > 0.0 { 1.0 } else { 0.0 };
                }
                amps
            }).collect::<Vec<_>>())
        }
        // bell: clusters of high partials that move apart
        7 => Table::from_partials(&(0..n).map(|f| {
            let t = f as f32 / (n - 1) as f32;
            let mut amps = vec![0.0; 64];
            for (i, base) in [1.0f32, 2.0, 3.0, 4.2, 5.4, 6.8, 8.9, 11.0, 13.3].iter().enumerate() {
                let k = (base * (1.0 + t * 0.5 * (i as f32 / 8.0)) * 4.0).round() as usize;
                if k >= 1 && k <= 64 {
                    amps[k - 1] += 1.0 / (1.0 + i as f32 * 0.6);
                }
            }
            amps
        }).collect::<Vec<_>>()),
        // vocal: a choir's ah > oh > oo > ee (higher formants)
        8 => vowel_table(&[
            [(850.0, 120.0), (1220.0, 140.0), (2810.0, 200.0)],
            [(600.0, 100.0), (1040.0, 120.0), (2650.0, 200.0)],
            [(370.0, 80.0), (950.0, 110.0), (2670.0, 200.0)],
            [(310.0, 70.0), (2790.0, 160.0), (3310.0, 220.0)],
        ], 220.0),
        // digital: a sine in fewer and fewer steps
        9 => Table::from_cycles(&(0..n).map(|f| {
            let steps = 2f32.powf(6.0 - 5.0 * f as f32 / (n - 1) as f32);
            cycle(move |p| ((p * TAU).sin() * steps).round() / steps)
        }).collect::<Vec<_>>()),
        // noisy: random spectra, tilted, blended through four seeds
        10 => Table::from_partials(&(0..n).map(|f| {
            let x = f as f32 / (n - 1) as f32 * 3.0;
            let i = (x as u32).min(2);
            let t = x - i as f32;
            (1..=128u32).map(|k| {
                let a = hash(k * 31 + i * 977);
                let b = hash(k * 31 + (i + 1) * 977);
                (a + (b - a) * t) / (k as f32).powf(0.8)
            }).collect()
        }).collect::<Vec<_>>()),
        // growl: a sine folded harder and harder
        11 => Table::from_cycles(&(0..n).map(|f| {
            let gain = 1.0 + 6.0 * f as f32 / (n - 1) as f32;
            cycle(move |p| ((p * TAU).sin() * gain * PI * 0.5 + 0.3 * (p * 2.0 * TAU).sin() * gain).sin())
        }).collect::<Vec<_>>()),
        _ => Table::from_partials(&[vec![1.0]]),
    }
}

/// A built-in table by its index in [`super::schema::TABLES`] (0..12);
/// made once.
pub fn builtin(index: usize) -> Arc<Table> {
    const N: usize = 12;
    static TABLES: [OnceLock<Arc<Table>>; N] = [const { OnceLock::new() }; N];
    let i = index.min(N - 1);
    TABLES[i].get_or_init(|| Arc::new(make(i))).clone()
}

/// A single sine, for oscillators with no table yet.
pub fn sine() -> Arc<Table> {
    static SINE: OnceLock<Arc<Table>> = OnceLock::new();
    SINE.get_or_init(|| Arc::new(Table::from_partials(&[vec![1.0]]))).clone()
}
