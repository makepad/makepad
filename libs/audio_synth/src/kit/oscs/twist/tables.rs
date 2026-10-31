// Lookup tables of the twist engine, generated at first use from the
// formulas of the MIT generator scripts resources/lookup_tables.py and
// resources/wavetables.py of the eurorack macro-oscillator (the tables the
// original ships pre-computed in resources.cc; the six-operator FM patch banks
// are not needed). Bank 3 of the wavetables starts from source waves kept as
// data in `wave_data.rs`. Original notices:
//
// Copyright 2016 Emilie Gillet. (resources/lookup_tables.py, resources/wavetables.py, resources.cc)
//
// Author: Emilie Gillet (emilie.o.gillet@gmail.com)
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.
//
// See http://creativecommons.org/licenses/MIT/ for more information.
//
// -----------------------------------------------------------------------------

use std::f64::consts::PI;
use std::sync::OnceLock;

use super::wave_data::BANK3_SOURCE;

pub const SINE_SIZE: usize = 641;
pub const WAVETABLE_SIZE: usize = 128;
/// Samples per integrated wave (128 + 4 guard samples).
pub const INTEGRATED_WAVE_SIZE: usize = 132;
pub const NUM_WAVES: usize = 192;

/// Every table of the engine family. `ws` holds the six waveshaper tables
/// (inverse tan, inverse sin, linear, bump, double bump, double bump again as
/// the sentinel), 257 entries each.
pub struct Tables {
    pub sine: Vec<f32>,
    pub fm_frequency_quantizer: Vec<f32>,
    pub fold: Vec<f32>,
    pub fold_2: Vec<f32>,
    pub stiffness: Vec<f32>,
    pub svf_shift: Vec<f32>,
    pub downsampler_fir: [f32; 4],
    pub ws: [Vec<i16>; 6],
    pub lpc_excitation_pulse: Vec<i8>,
    pub integrated_waves: Vec<i16>,
}

/// The shared tables (built once; constructors call this so a render path
/// only ever reads the initialised cell).
#[inline]
pub fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(build)
}

fn round_i(x: f64) -> i64 {
    x.round_ties_even() as i64
}

fn build() -> Tables {
    Tables {
        sine: sine_table(),
        fm_frequency_quantizer: fm_frequency_quantizer(),
        fold: fold(),
        fold_2: fold_2(),
        stiffness: stiffness(),
        svf_shift: svf_shift(),
        downsampler_fir: [0.02442415, 0.09297315, 0.16712938, 0.21547332],
        ws: waveshapers(),
        lpc_excitation_pulse: lpc_excitation_pulse(),
        integrated_waves: integrated_waves(),
    }
}

// ------------------------------------------------------------ lookup tables

fn sine_table() -> Vec<f32> {
    // 512-sample cycle plus a quarter (cosine reads) plus one guard sample.
    (0..SINE_SIZE).map(|i| (2.0 * PI * i as f64 / 512.0).sin() as f32).collect()
}

fn fm_frequency_quantizer() -> Vec<f32> {
    let s16 = 2f64.powf(16.0 / 1200.0);
    let ratios = [
        0.5,
        0.5 * s16,
        2f64.sqrt() / 2.0,
        PI / 4.0,
        1.0,
        1.0 * s16,
        2f64.sqrt(),
        PI / 2.0,
        7.0 / 4.0,
        2.0,
        2.0 * s16,
        9.0 / 4.0,
        11.0 / 4.0,
        2.0 * 2f64.sqrt(),
        3.0,
        PI,
        3f64.sqrt() * 2.0,
        4.0,
        2f64.sqrt() * 3.0,
        PI * 3.0 / 2.0,
        5.0,
        2f64.sqrt() * 4.0,
        8.0,
    ];
    let mut scale: Vec<f64> = Vec::new();
    for r in ratios {
        let r = 12.0 * r.log2();
        scale.extend([r, r, r]);
    }
    let target = (scale.len() as f64).log2().ceil().exp2() as usize;
    while scale.len() < target {
        // First largest gap (argmax keeps the first maximum).
        let mut gap = 0;
        let mut best = f64::NEG_INFINITY;
        for i in 0..scale.len() - 1 {
            let d = scale[i + 1] - scale[i];
            if d > best {
                best = d;
                gap = i;
            }
        }
        let mid = (scale[gap] + scale[gap + 1]) / 2.0;
        scale.insert(gap + 1, mid);
    }
    let last = *scale.last().unwrap();
    scale.push(last);
    scale.push(last);
    scale.into_iter().map(|x| x as f32).collect()
}

fn waveshapers() -> [Vec<i16>; 6] {
    const N: usize = 128;
    let x: Vec<f64> = (0..=N).map(|i| i as f64 / N as f64).collect();
    let scale = (8.0f64).atan(); // max of atan(8 cos(pi x)), at x = 0
    let linear: Vec<f64> = x.clone();
    let bump: Vec<f64> = x
        .iter()
        .map(|&x| {
            let fade_crop = (4.0 - 4.0 * x).min(1.0);
            (1.0 - (PI * x * 1.5).cos()) * (1.0 - (PI * fade_crop).cos()) / 4.5
        })
        .collect();
    let double_bump: Vec<f64> = x.iter().map(|&x| (PI * x * 1.5).sin()).collect();
    let inverse_sin: Vec<f64> = x.iter().map(|&x| (1.0 - 2.0 * x).acos() / PI).collect();
    let inverse_tan: Vec<f64> =
        x.iter().map(|&x| ((scale * (1.0 - 2.0 * x)).tan() / 8.0).acos() / PI).collect();
    let flip = |v: &Vec<f64>| -> Vec<i16> {
        let mut out: Vec<f64> = (1..=N).rev().map(|i| -v[i]).collect();
        out.extend(v.iter().copied());
        out.into_iter().map(|s| round_i(s * 32767.0) as i16).collect()
    };
    let db = flip(&double_bump);
    [flip(&inverse_tan), flip(&inverse_sin), flip(&linear), flip(&bump), db.clone(), db]
}

fn fold_x() -> Vec<f64> {
    const N: usize = 512;
    let mut x: Vec<f64> = (0..N + 4).map(|i| i as f64 / (N as f64 / 2.0) - 1.0).collect();
    let l = x.len();
    x[l - 1] = x[l - 2];
    x
}

fn fold() -> Vec<f32> {
    let x = fold_x();
    let v: Vec<f64> = x
        .iter()
        .map(|&x| {
            let sine = (8.0 * PI * x).sin();
            let window = (-x * x * 4.0).exp().powi(2);
            sine * window + (3.0 * x).atan() * (1.0 - window)
        })
        .collect();
    let m = v.iter().fold(0.0f64, |m, s| m.max(s.abs()));
    v.into_iter().map(|s| (s / m) as f32).collect()
}

fn deadband(v_in: f64, r_in: f64, r_load: f64) -> f64 {
    let r_fb = 150.0e3;
    let v_sat = 10.47;
    let mut v_out = -r_fb / r_in * v_in;
    if v_out >= v_sat {
        v_out = v_sat;
    }
    if v_out < -v_sat {
        v_out = -v_sat;
    }
    (v_in * r_fb * r_load + v_out * r_in * r_load) / (r_fb * r_load + r_in * r_load + r_in * r_fb)
}

fn fold_2() -> Vec<f32> {
    let x = fold_x();
    let diode = |x: f64| 0.7 * x / (0.3 + x.abs());
    let v: Vec<f64> = x
        .iter()
        .map(|&x| {
            let v_in = x * 12.0;
            let s1 = deadband(v_in, 10e3, 100e3);
            let s2 = deadband(v_in, 49.9e3, 44.2e3);
            let s3 = deadband(v_in, 91e3, 18e3);
            let s4 = deadband(v_in, 30e3, 71.4e3);
            let s5 = deadband(v_in, 68e3, 33.0e3);
            let s45 = -33.0 / 71.4 * s4 - 33.0 / 33.0 * s5 - 33.0 / 240.0 * v_in;
            diode(-150.0 / 100.0 * s1 - 150.0 / 44.2 * s2 - 150.0 / 18.0 * s3 - 150.0 / 33.0 * s45)
        })
        .collect();
    let m = v.iter().fold(f64::NEG_INFINITY, |m, &s| m.max(s));
    v.into_iter().map(|s| (s / m) as f32).collect()
}

fn stiffness() -> Vec<f32> {
    let mut out: Vec<f64> = (0..65)
        .map(|i| {
            let mut g = i as f64 / 64.0;
            if g < 0.25 {
                g = 0.25 - g;
                -g * 0.25
            } else if g < 0.3 {
                0.0
            } else if g < 0.9 {
                g -= 0.3;
                g /= 0.6;
                0.01 * 10f64.powf(g * 2.005) - 0.01
            } else {
                g -= 0.9;
                g /= 0.1;
                g *= g;
                1.5 - (g * PI).cos() / 2.0
            }
        })
        .collect();
    out[64] = 2.0;
    out[63] = 2.0;
    out.into_iter().map(|x| x as f32).collect()
}

fn svf_shift() -> Vec<f32> {
    (0..257)
        .map(|i| {
            let ratio = 2f64.powf(i as f64 / 12.0);
            (1.0 - 2.0 * (1.0 / ratio).atan() / (2.0 * PI)) as f32
        })
        .collect()
}

// ------------------------------------------------ LPC excitation pulse

#[derive(Clone, Copy, Default)]
struct C {
    re: f64,
    im: f64,
}

impl C {
    fn mul(self, o: C) -> C {
        C { re: self.re * o.re - self.im * o.im, im: self.re * o.im + self.im * o.re }
    }
    fn add(self, o: C) -> C {
        C { re: self.re + o.re, im: self.im + o.im }
    }
}

/// Plain DFT (init-time only). `inverse` applies the 1/N of numpy's ifft.
fn dft(x: &[C], inverse: bool) -> Vec<C> {
    let n = x.len();
    let sign = if inverse { 1.0 } else { -1.0 };
    let tw: Vec<C> = (0..n)
        .map(|k| {
            let a = sign * 2.0 * PI * k as f64 / n as f64;
            C { re: a.cos(), im: a.sin() }
        })
        .collect();
    let scale = if inverse { 1.0 / n as f64 } else { 1.0 };
    (0..n)
        .map(|k| {
            let mut acc = C::default();
            for (j, &v) in x.iter().enumerate() {
                acc = acc.add(v.mul(tw[(k * j) % n]));
            }
            C { re: acc.re * scale, im: acc.im * scale }
        })
        .collect()
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..60 {
        term *= q / (k as f64 * k as f64);
        sum += term;
    }
    sum
}

fn lpc_excitation_pulse() -> Vec<i8> {
    const PULSE: [f64; 40] = [
        42.0, -44.0, 50.0, -78.0, 18.0, 37.0, 20.0, 2.0, -31.0, -59.0, 2.0, 95.0, 90.0, 5.0, 15.0, 38.0,
        -4.0, -91.0, -91.0, -42.0, -35.0, -36.0, -4.0, 37.0, 43.0, 34.0, 33.0, 15.0, -1.0, -8.0, -18.0,
        -19.0, -17.0, -9.0, -10.0, -6.0, 0.0, 3.0, 2.0, 1.0,
    ];
    let n = PULSE.len();
    let ratio = 32;
    let target_length = 20;
    let big = n * ratio;
    // Band-limited 32x interpolation: irfft(rfft(pulse), 1280).
    let bins: Vec<C> = (0..=n / 2)
        .map(|k| {
            let mut acc = C::default();
            for (j, &p) in PULSE.iter().enumerate() {
                let a = -2.0 * PI * (k * j) as f64 / n as f64;
                acc = acc.add(C { re: p * a.cos(), im: p * a.sin() });
            }
            acc
        })
        .collect();
    let up: Vec<f64> = (0..big)
        .map(|t| {
            let mut s = bins[0].re;
            for (k, b) in bins.iter().enumerate().skip(1) {
                let a = 2.0 * PI * (k * t) as f64 / big as f64;
                s += 2.0 * (b.re * a.cos() - b.im * a.sin());
            }
            s / big as f64
        })
        .collect();
    // Kaiser window, beta = 1.
    let i0b = bessel_i0(1.0);
    let windowed: Vec<C> = up
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let r = 2.0 * i as f64 / (big - 1) as f64 - 1.0;
            let w = bessel_i0((1.0 - r * r).max(0.0).sqrt()) / i0b;
            C { re: s * w, im: 0.0 }
        })
        .collect();
    // Minimum-phase reconstruction through the real cepstrum.
    let xf = dft(&windowed, false);
    let log_mag: Vec<C> =
        xf.iter().map(|c| C { re: (1e-50 + (c.re * c.re + c.im * c.im).sqrt()).ln(), im: 0.0 }).collect();
    let mut cep: Vec<C> = dft(&log_mag, true).into_iter().map(|c| C { re: c.re, im: 0.0 }).collect();
    let half = big / 2;
    for c in cep.iter_mut().take(half).skip(1) {
        c.re *= 2.0;
    }
    for c in cep.iter_mut().skip(half + 1) {
        c.re = 0.0;
    }
    let spec: Vec<C> = dft(&cep, false)
        .into_iter()
        .map(|c| {
            let m = c.re.exp();
            C { re: m * c.im.cos(), im: m * c.im.sin() }
        })
        .collect();
    let min_phi: Vec<f64> = dft(&spec, true).into_iter().map(|c| c.re).collect();
    let len = ratio * target_length;
    let mut p: Vec<f64> = min_phi[..len].to_vec();
    let p0 = p[0];
    for v in p.iter_mut() {
        *v -= p0;
    }
    let fade = ratio * 4;
    for i in 0..fade {
        p[len - fade + i] *= 1.0 - i as f64 / (fade - 1) as f64;
    }
    let m = p.iter().fold(f64::NEG_INFINITY, |m, &v| m.max(v));
    p.into_iter().map(|v| round_i(v / m * 127.0) as i8).collect()
}

// ------------------------------------------------------------- wavetables

fn t(i: usize) -> f64 {
    i as f64 / WAVETABLE_SIZE as f64
}

type Wave = [f64; WAVETABLE_SIZE];

fn sine(frequency: f64) -> Wave {
    let mut w = [0.0; WAVETABLE_SIZE];
    if frequency >= (WAVETABLE_SIZE / 2) as f64 {
        return w;
    }
    for (i, s) in w.iter_mut().enumerate() {
        *s = (2.0 * PI * t(i) * frequency).sin();
    }
    w
}

fn add(a: &mut Wave, b: &Wave, g: f64) {
    for i in 0..WAVETABLE_SIZE {
        a[i] += b[i] * g;
    }
}

fn comb(n: usize) -> Wave {
    let mut x = [0.0; WAVETABLE_SIZE];
    for i in 0..n {
        add(&mut x, &sine((i + 1) as f64), 1.0);
    }
    x
}

fn pair(n: usize) -> Wave {
    let mut x = [0.0; WAVETABLE_SIZE];
    for i in 0..n {
        let g = (i as f64 + 0.5) / (n as f64 - 1.0);
        add(&mut x, &sine((i + 1) as f64), g);
        add(&mut x, &sine(((i + 1) * 4) as f64), g * 0.5);
    }
    x
}

fn tri(n: usize, f: f64) -> Wave {
    let mut x = [0.0; WAVETABLE_SIZE];
    for i in 0..n {
        let h = (2 * i + 1) as f64;
        add(&mut x, &sine(h * f), 1.0 / (h * h));
    }
    x
}

fn tri_stack(n: usize) -> Wave {
    let mut x = [0.0; WAVETABLE_SIZE];
    for i in 0..n {
        // The generator's `n / 3` is an integer division.
        add(&mut x, &tri(15 + 5 * n, (i + n / 3) as f64), 1.0);
    }
    x
}

fn saw(n: usize, f: f64) -> Wave {
    let mut x = [0.0; WAVETABLE_SIZE];
    for i in 0..n {
        add(&mut x, &sine((i + 1) as f64 * f), 1.0 / (i + 1) as f64);
    }
    x
}

fn quadra(n: usize) -> Wave {
    let mut x = [0.0; WAVETABLE_SIZE];
    for (h, a) in [1.0, 0.5, 1.0, 0.5].iter().enumerate() {
        add(&mut x, &sine((2 * n + 2 * h + 1) as f64), *a);
    }
    x
}

fn drawbars(bars: &str) -> Wave {
    let pipes = [1.0, 3.0, 2.0, 4.0, 6.0, 8.0, 10.0, 12.0, 16.0];
    let mut x = [0.0; WAVETABLE_SIZE];
    for (c, f) in bars.chars().zip(pipes.iter()) {
        let intensity = c.to_digit(10).unwrap() as f64;
        add(&mut x, &sine(*f), intensity / 8.0);
    }
    x
}

fn pulse(duty: f64) -> Wave {
    let mut w = [0.0; WAVETABLE_SIZE];
    for (i, s) in w.iter_mut().enumerate() {
        let tt = if i == WAVETABLE_SIZE - 1 { 0.0 } else { t(i) };
        *s = if tt < duty { 1.0 } else { -1.0 };
    }
    w
}

fn burst(duty: f64) -> Wave {
    let d = duty.sqrt();
    let s = sine(1.0 / duty);
    let mut w = [0.0; WAVETABLE_SIZE];
    for i in 0..WAVETABLE_SIZE {
        let tt = if i == WAVETABLE_SIZE - 1 { 0.0 } else { t(i) };
        let v = if tt < d { -1.0 } else { 0.0 };
        w[i] = -v * s[i];
    }
    w
}

fn trisaw(harmonic: f64) -> Wave {
    let mut x = tri(80, 1.0);
    let g = if harmonic != 1.0 { 1.0 } else { 0.25 };
    add(&mut x, &saw(80, harmonic), g * 0.5);
    x
}

fn sawtri(harmonic: f64) -> Wave {
    let mut x = [0.0; WAVETABLE_SIZE];
    add(&mut x, &saw(80, 1.0), 0.5);
    let g = if harmonic != 1.0 { 1.0 } else { 0.25 };
    add(&mut x, &tri(80, harmonic), g);
    x
}

fn bandpass_formant(ratio: f64) -> Wave {
    let s = sine(ratio * 1.5);
    let mut w = [0.0; WAVETABLE_SIZE];
    for i in 0..WAVETABLE_SIZE {
        w[i] = s[i] * (1.0 - t(i)) * 0.5;
    }
    w
}

fn sine_power(power: i32) -> Wave {
    let mut x = sine(1.0);
    add(&mut x, &saw(16, 1.0), 1.0);
    let p = 2f64.powi(power);
    for v in x.iter_mut() {
        *v = v.signum() * v.abs().powf(p);
        if v.is_nan() {
            *v = 0.0;
        }
    }
    x
}

fn center(f: &mut Wave) {
    let mx = f.iter().fold(f64::NEG_INFINITY, |m, &v| m.max(v));
    let mn = f.iter().fold(f64::INFINITY, |m, &v| m.min(v));
    let c = (mx + mn) / 2.0;
    for v in f.iter_mut() {
        *v -= c;
    }
}

fn formant_f(index: usize, digital: bool) -> Wave {
    let f1 = 3.9 * (index + 1) as f64 / 8.0;
    let f2 = f1 * (1.0 - (f1 * PI * 0.8).cos());
    let distort = |x: f64| (x * 8.0).atan() / PI;
    let (s3, s1, s2) = if digital {
        (sine(1.0 + 2.9 * (f2 + f1)), sine(1.0 + 3.2 * f1), sine(1.0 + 4.1 * f2))
    } else {
        (sine(1.0 + 2.8 * (f2 + f1)), sine(1.0 + 3.0 * f1), sine(1.0 + 4.0 * f2))
    };
    let mut w = [0.0; WAVETABLE_SIZE];
    for i in 0..WAVETABLE_SIZE {
        let a1 = (1.0 - t(i)).powf(0.2) * (-4.0 * t(i)).exp();
        let a2 = (1.0 - t(i)).powf(0.2) * (-2.0 * t(i)).exp();
        w[i] = if digital {
            distort(s3[i]) * a2 * 0.7 + distort(s1[i]) * a1 + distort(s2[i]) * a2 * 0.7
        } else {
            s3[i] * a2 * 1.7 + s1[i] * a1 + s2[i] * a2 * 1.5
        };
    }
    center(&mut w);
    w
}

/// Bank 3 wave `k` (0..63): the source cycle, optionally rebuilt from its
/// magnitude spectrum with all partials in sine phase (and no DC / Nyquist).
fn bank3_wave(k: usize, fix: bool) -> Wave {
    let src = &BANK3_SOURCE[k * WAVETABLE_SIZE..(k + 1) * WAVETABLE_SIZE];
    let mut s = [0.0; WAVETABLE_SIZE];
    for i in 0..WAVETABLE_SIZE {
        s[i] = src[i] as f64 - 128.0;
    }
    if !fix {
        return s;
    }
    let n = WAVETABLE_SIZE;
    let mut w = [0.0; WAVETABLE_SIZE];
    for kk in 1..n / 2 {
        let (mut re, mut im) = (0.0, 0.0);
        for (j, &v) in s.iter().enumerate() {
            let a = -2.0 * PI * (kk * j) as f64 / n as f64;
            re += v * a.cos();
            im += v * a.sin();
        }
        let mag = (re * re + im * im).sqrt();
        for (j, o) in w.iter_mut().enumerate() {
            *o += 2.0 * mag * (2.0 * PI * (kk * j) as f64 / n as f64).sin() / n as f64;
        }
    }
    w
}

fn all_waves() -> Vec<Wave> {
    let mut v: Vec<Wave> = Vec::with_capacity(NUM_WAVES);
    // Bank 1: mild and additive.
    for f in [1, 2, 3, 4, 5, 6, 7, 8] {
        v.push(sine(f as f64));
    }
    for f in [2, 3, 4, 6, 8, 12, 16, 24] {
        v.push(sine(f as f64));
    }
    for n in [2, 3, 4, 6, 8, 12, 16, 24] {
        v.push(quadra(n));
    }
    for n in [2, 3, 5, 8, 13, 21, 34, 55] {
        v.push(comb(n));
    }
    for n in [2, 4, 6, 8, 10, 12, 14, 16] {
        v.push(pair(n));
    }
    for n in [2, 4, 6, 8, 10, 12, 14, 16] {
        v.push(tri_stack(n));
    }
    for b in ["688600000", "686040000", "666806000", "655550600", "665560060", "688500888", "660000888", "060000046"] {
        v.push(drawbars(b));
    }
    for b in ["867000006", "888876788", "668744354", "448644054", "327645222", "204675300", "002478500", "002050321"] {
        v.push(drawbars(b));
    }
    // Bank 2: formantish.
    for h in [1.0, 1.5, 2.0, 3.0, 4.0, 4.5, 5.0, 8.0] {
        v.push(trisaw(h));
    }
    for h in [1.0, 1.5, 2.0, 3.0, 4.0, 4.5, 5.0, 8.0] {
        v.push(sawtri(h));
    }
    let duties = [0.5, 0.4, 1.0 / 3.0, 0.25, 0.2, 0.125, 1.0 / 16.0, 1.0 / 32.0];
    for d in duties {
        v.push(burst(d));
    }
    for r in [2.0, 3.0, 4.0, 6.0, 8.0, 9.0, 10.0, 16.0] {
        v.push(bandpass_formant(r));
    }
    for i in 0..8 {
        v.push(formant_f(i, false));
    }
    for i in 0..8 {
        v.push(formant_f(i, true));
    }
    for d in duties {
        v.push(pulse(d));
    }
    for p in 0..8 {
        v.push(sine_power(p));
    }
    // Bank 3: source waves (the last 16 kept as they are).
    for k in 0..64 {
        v.push(bank3_wave(k, k < 48));
    }
    v
}

/// First-order integrated wavetables (linear interpolation, K = 1, N = 1):
/// each wave doubled, zero-mean, peak-normalised, integrated, zero-mean,
/// scaled by 4 * 32768 / 128; the last 132 samples are kept.
fn integrated_waves() -> Vec<i16> {
    let n = WAVETABLE_SIZE;
    let mut data = Vec::with_capacity(NUM_WAVES * INTEGRATED_WAVE_SIZE);
    for w in all_waves() {
        let mut x: Vec<f64> = w.iter().chain(w.iter()).copied().collect();
        let mean = x.iter().sum::<f64>() / x.len() as f64;
        x.iter_mut().for_each(|v| *v -= mean);
        let peak = x.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        x.iter_mut().for_each(|v| *v /= peak);
        let mut acc = 0.0;
        for v in x.iter_mut() {
            acc += *v;
            *v = acc;
        }
        let mean = x.iter().sum::<f64>() / x.len() as f64;
        let scale = 4.0 * 32768.0 / n as f64;
        for v in &x[x.len() - n - 4..] {
            data.push(round_i((v - mean) * scale) as i16);
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close_f(name: &str, t: &[f32], spots: &[(usize, f32)], tol: f32) {
        for &(i, v) in spots {
            assert!((t[i] - v).abs() <= tol, "{name}[{i}] = {} want {v}", t[i]);
        }
    }

    fn close_i<T: Copy + Into<i32>>(name: &str, t: &[T], spots: &[(usize, i32)], tol: i32) {
        for &(i, v) in spots {
            let got: i32 = t[i].into();
            assert!((got - v).abs() <= tol, "{name}[{i}] = {got} want {v}");
        }
    }

    // Spot values of the original's pre-computed tables.
    #[test]
    fn generated_tables_match_original_spots() {
        let t = tables();
        assert_eq!(t.sine.len(), 641);
        close_f("sine", &t.sine, &[(49, 0.5657318108), (154, 0.9495281806), (374, -0.9924795346), (548, 0.4275550934)], 1e-6);
        assert_eq!(t.fm_frequency_quantizer.len(), 130);
        close_f("fmq", &t.fm_frequency_quantizer, &[(9, -8.92), (14, -6.0), (17, -5.091023223), (22, -3.659290641), (54, 9.688259065), (107, 26.83750356), (111, 27.86313714), (129, 36.0)], 1e-4);
        assert_eq!(t.fold.len(), 516);
        close_f("fold", &t.fold, &[(60, -0.9216381294), (92, -0.8278939413), (246, -0.6576794884), (434, 0.8630788161)], 1e-5);
        close_f("fold_2", &t.fold_2, &[(47, 0.9458826201), (120, -0.5650004958), (226, 0.103512801), (429, -0.8093172692)], 1e-5);
        close_f("stiffness", &t.stiffness, &[(6, -0.0390625), (23, 0.00579106635), (39, 0.09810060511), (63, 2.0)], 1e-6);
        close_f("svf_shift", &t.svf_shift, &[(30, 0.944305623), (49, 0.9812439545), (160, 0.9999691598)], 1e-6);
        close_i("ws_inverse_tan", &t.ws[0], &[(41, -17130), (124, -10019), (175, 15856), (253, 23661)], 1);
        close_i("ws_inverse_sin", &t.ws[1], &[(37, -20926), (84, -13069), (214, 20043), (250, 28215)], 1);
        close_i("ws_linear", &t.ws[2], &[(20, -27647), (160, 8192), (254, 32255)], 1);
        close_i("ws_bump", &t.ws[3], &[(31, -27736), (138, 976), (228, 26025), (242, 8750)], 1);
        close_i("ws_double_bump", &t.ws[4], &[(11, 30117), (86, -32757), (236, -24279), (252, -32412)], 1);
        assert_eq!(t.lpc_excitation_pulse.len(), 640);
        close_i("lpc", &t.lpc_excitation_pulse, &[(60, 126), (132, -24), (223, -26), (253, -69), (294, -21), (400, 31), (407, 35), (508, 9)], 1);
        assert_eq!(t.integrated_waves.len(), 25344);
        close_i(
            "waves",
            &t.integrated_waves,
            &[
                (134, -10322), (395, -6941), (1769, 1119), (2039, 766), (2206, 1479), (2640, 1114), (2719, -63),
                (3392, 3613), (3602, 1119), (4112, -141), (4486, -8015), (4773, 104), (4945, 177), (4957, 176),
                (5318, 2556), (5451, 1903), (5774, 1012), (5975, -261), (6245, 206), (6840, -30), (7561, 1158),
                (7600, 3430), (7645, -3574), (8609, 1102), (9104, -12888), (9123, -8158), (9238, -13735),
                (10440, -4811), (11142, -3434), (11756, -453), (12099, -941), (12466, 730), (12857, 456),
                (12914, -1222), (13043, -672), (13073, -1004), (13121, 105), (13161, -608), (13608, -148),
                (13728, -6257), (14107, -1208), (14438, 500), (14718, 205), (14963, 18944), (15778, -62),
                (15891, 14261), (16891, -1650), (17517, 2532), (18004, 827), (18029, 214), (18326, 206),
                (18557, -62), (19304, -341), (19982, 1194), (20237, 6786), (20784, 2320), (21461, -1145),
                (21578, -6961), (22157, 1454), (22301, -1488), (22371, 3774), (22626, 11118), (23147, 9902),
                (24241, 13461),
            ],
            1,
        );
    }
}
