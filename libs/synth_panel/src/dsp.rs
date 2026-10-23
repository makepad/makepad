//! The numbers behind the live displays: a filter's response, an
//! envelope's shape, an oscillator's cycle, an LFO, a spectrum in bands and
//! meter ballistics. Every curve is sampled into a row of [`ROW`] values
//! for the displays.

use makepad_math::complex::{fft_f32_recursive_pow2_forward, ComplexF32};
use std::f32::consts::TAU;

/// Samples in one display row.
pub const ROW: usize = 256;

/// |H(f)| of a panel's filter, in dB, at `f` Hz: a four-pole ladder or a
/// two-pole state variable (low, band, high pass, notch) with quality `q`.
pub fn filter_db(mode: usize, cutoff: f32, q: f32, f: f32) -> f32 {
    let w = f / cutoff.max(1.0);
    let q = q.clamp(0.3, 30.0);
    let mag = match mode {
        // Ladder: four one-pole sections in a loop, feedback k.
        0 => {
            let k = ((q - 0.5) / 12.0).clamp(0.0, 1.0) * 3.9;
            // (1 + jw)^4 + k, by hand.
            let (mut re, mut im) = (1.0f32, 0.0f32);
            for _ in 0..4 {
                let (r, i) = (re - im * w, im + re * w);
                re = r;
                im = i;
            }
            (1.0 + k) / ((re + k).hypot(im)).max(1e-6)
        }
        _ => {
            let den = ((1.0 - w * w).powi(2) + (w / q).powi(2)).sqrt().max(1e-6);
            match mode {
                1 => 1.0 / den,
                2 => (w / q) / den,
                3 => w * w / den,
                _ => (1.0 - w * w).abs() / den,
            }
        }
    };
    20.0 * mag.max(1e-6).log10()
}

/// The response over 20 Hz..20 kHz (log), as display heights 0..1 with
/// 0 dB at `zero` and `range` dB over the whole height.
pub fn filter_row(out: &mut [f32], mode: usize, cutoff: f32, q: f32) {
    let n = out.len().max(2);
    for (i, o) in out.iter_mut().enumerate() {
        let f = 20.0 * 1000f32.powf(i as f32 / (n - 1) as f32);
        let db = filter_db(mode, cutoff, q, f);
        *o = ((db + 36.0) / 54.0).clamp(0.0, 1.0);
    }
}

/// Where a frequency sits on the filter display, 0..1.
pub fn freq_x(f: f32) -> f32 {
    ((f.max(20.0) / 20.0).ln() / 1000f32.ln()).clamp(0.0, 1.0)
}

/// An envelope's shape and where its corners fall on the display:
/// `(attack end, decay end, release start)` as 0..1 of the width.
pub fn adsr_row(out: &mut [f32], a: f32, d: f32, s: f32, r: f32) -> [f32; 3] {
    // The sustain gets a fixed stretch; each time is drawn on a gentle log
    // so a 2 ms attack and a 4 s release are both visible.
    let t = |x: f32| (1.0 + x.max(0.0) * 40.0).ln();
    let (ta, td, tr) = (t(a).max(0.02), t(d).max(0.02), t(r).max(0.02));
    let ts = (ta + td + tr) * 0.35;
    let total = ta + td + ts + tr;
    let s = s.clamp(0.0, 1.0);
    let n = out.len().max(2);
    for (i, o) in out.iter_mut().enumerate() {
        let x = i as f32 / (n - 1) as f32 * total;
        *o = if x < ta {
            let u = x / ta;
            1.0 - (1.0 - u).powf(1.8)
        } else if x < ta + td {
            let u = (x - ta) / td;
            s + (1.0 - s) * (-4.5 * u).exp() * (1.0 - u)
        } else if x < ta + td + ts {
            s
        } else {
            let u = ((x - ta - td - ts) / tr).min(1.0);
            s * (-4.5 * u).exp() * (1.0 - u)
        };
    }
    [ta / total, (ta + td) / total, (ta + td + ts) / total]
}

/// Inverse of the envelope display's time stretch: seconds for a stretch.
pub fn adsr_secs(stretch: f32) -> f32 {
    (stretch.exp() - 1.0) / 40.0
}

pub fn adsr_stretch(secs: f32) -> f32 {
    (1.0 + secs.max(0.0) * 40.0).ln()
}

/// One sample of a wave shape at phase `p` (0..1).
pub fn wave(shape: &str, p: f32, width: f32) -> f32 {
    let p = p - p.floor();
    match shape {
        "sine" => (p * TAU).sin(),
        "tri" | "triangle" => 1.0 - 4.0 * (p - 0.5).abs(),
        "square" => if p < 0.5 { 1.0 } else { -1.0 },
        "pulse" => if p < width.clamp(0.05, 0.95) { 1.0 } else { -1.0 },
        "softsaw" => {
            let s = 2.0 * p - 1.0;
            (s * 2.2).tanh() / 2.2f32.tanh() * if p > 0.96 { (1.0 - p) / 0.04 } else { 1.0 }
        }
        "random" => {
            let k = (p * 8.0).floor();
            let h = ((k * 12.9898).sin() * 43758.547).fract();
            h * 2.0 - 1.0
        }
        _ => 2.0 * p - 1.0,
    }
}

/// Two cycles of an oscillator, `-1..1` mapped to `0..1`. `fm` is
/// `(ratio, index)` for a two-operator FM carrier.
pub fn wave_row(out: &mut [f32], shape: &str, width: f32, fm: Option<(f32, f32)>) {
    let n = out.len().max(2);
    for (i, o) in out.iter_mut().enumerate() {
        let p = i as f32 / (n - 1) as f32 * 2.0;
        let v = match fm {
            Some((ratio, index)) => (p * TAU + index * (p * ratio * TAU).sin()).sin(),
            None => wave(shape, p, width),
        };
        *o = v * 0.45 + 0.5;
    }
}

/// An LFO scrolling past: `cycles` across the display, advanced by `phase`.
pub fn lfo_row(out: &mut [f32], shape: &str, depth: f32, cycles: f32, phase: f32) {
    let n = out.len().max(2);
    let amp = 0.12 + 0.33 * depth.clamp(0.0, 1.0);
    for (i, o) in out.iter_mut().enumerate() {
        let p = i as f32 / (n - 1) as f32 * cycles + phase;
        *o = 0.5 + amp * wave(shape, p, 0.5);
    }
}

/// A scope trace over `out.len() * stride` samples (one display sample
/// per `stride`), triggered on a rising zero crossing in the older part so
/// a steady tone stands still, `-1..1` mapped to `0..1`.
pub fn scope_row(out: &mut [f32], samples: &[f32], stride: usize, gain: f32) {
    let stride = stride.max(1);
    let span = out.len() * stride;
    if samples.len() < span + 2 {
        out.fill(0.5);
        return;
    }
    let search = samples.len() - span;
    let mut start = search;
    for i in (1..search).rev() {
        if samples[i - 1] <= 0.0 && samples[i] > 0.0 {
            start = i;
            break;
        }
    }
    for (i, o) in out.iter_mut().enumerate() {
        let at = start + i * stride;
        // The mean of the samples the display sample stands for.
        let v = samples[at..at + stride].iter().sum::<f32>() / stride as f32;
        *o = (v * gain * 0.5 + 0.5).clamp(0.02, 0.98);
    }
}

/// A spectrum in `out.len()` log-spaced bands from 30 Hz to 18 kHz, as
/// display heights (−72..0 dB), eased: fast up, slow down.
pub struct Spectrum {
    buf: Vec<ComplexF32>,
    scratch: Vec<ComplexF32>,
    window: Vec<f32>,
}

impl Spectrum {
    pub fn new(size: usize) -> Spectrum {
        let size = size.next_power_of_two();
        let window = (0..size).map(|i| 0.5 - 0.5 * (TAU * i as f32 / size as f32).cos()).collect();
        Spectrum { buf: vec![ComplexF32 { re: 0.0, im: 0.0 }; size], scratch: vec![ComplexF32 { re: 0.0, im: 0.0 }; size], window }
    }

    pub fn analyse(&mut self, samples: &[f32], rate: f32, out: &mut [f32], dt: f32) {
        let size = self.buf.len();
        if samples.len() < size {
            return;
        }
        let tail = &samples[samples.len() - size..];
        for i in 0..size {
            self.buf[i] = ComplexF32 { re: tail[i] * self.window[i], im: 0.0 };
        }
        fft_f32_recursive_pow2_forward(&mut self.buf, &mut self.scratch);
        let bands = out.len().max(1);
        let bin_hz = rate / size as f32;
        for (b, o) in out.iter_mut().enumerate() {
            let f0 = 30.0 * 600f32.powf(b as f32 / bands as f32);
            let f1 = 30.0 * 600f32.powf((b + 1) as f32 / bands as f32);
            let (i0, i1) = ((f0 / bin_hz) as usize, ((f1 / bin_hz) as usize).max((f0 / bin_hz) as usize + 1));
            let mut peak = 0.0f32;
            for i in i0.max(1)..i1.min(size / 2) {
                let c = self.buf[i];
                peak = peak.max((c.re * c.re + c.im * c.im).sqrt());
            }
            let db = 20.0 * (peak * 4.0 / size as f32).max(1e-6).log10();
            let target = ((db + 72.0) / 72.0).clamp(0.0, 1.0);
            *o = if target > *o { target } else { (*o - dt * 0.9).max(target) };
        }
    }
}

/// Peak and RMS ballistics for a meter: instant attack, ~1.5 s fall for
/// the peak, a quicker one for the body.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Ballistics {
    pub peak: f32,
    pub rms: f32,
}

impl Ballistics {
    pub fn feed(&mut self, peak: f32, rms: f32, dt: f32) {
        let fall = |v: f32, rate: f32| v * (-dt * rate).exp();
        self.peak = peak.max(fall(self.peak, 1.6));
        self.rms = rms.max(fall(self.rms, 5.0));
    }

    /// A level 0..1 on a meter reading −48..+3 dB.
    pub fn height(level: f32) -> f32 {
        let db = 20.0 * level.max(1e-5).log10();
        ((db + 48.0) / 51.0).clamp(0.0, 1.0)
    }
}
