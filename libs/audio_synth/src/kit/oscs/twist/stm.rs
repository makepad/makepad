// Rust port of the MIT stmlib helpers the twist engine uses: dsp/dsp.h,
// dsp/units.h + units.cc (tables generated from their formula), dsp/filter.h,
// dsp/cosine_oscillator.h, dsp/hysteresis_quantizer.h, dsp/limiter.h,
// dsp/parameter_interpolator.h, dsp/polyblep.h, dsp/rsqrt.h, dsp/delay_line.h,
// utils/dsp.h, utils/random.h + random.cc; `DelayLine` also stands in for the
// macro-oscillator's own dsp/physical_modelling/delay_line.h (the same code
// over a borrowed buffer). Original notices:
//
// Copyright 2012 Emilie Gillet. (dsp/dsp.h, utils/dsp.h, utils/random.h, utils/random.cc)
// Copyright 2014 Emilie Gillet. (dsp/units.h, dsp/units.cc, dsp/filter.h,
//                                dsp/cosine_oscillator.h, dsp/rsqrt.h, dsp/delay_line.h,
//                                macro-oscillator dsp/physical_modelling/delay_line.h)
// Copyright 2015 Emilie Gillet. (dsp/hysteresis_quantizer.h, dsp/limiter.h,
//                                dsp/parameter_interpolator.h)
// Copyright 2017 Emilie Gillet. (dsp/polyblep.h)
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
//
// Differences from the original: the random generator is an instance owned
// by the voice (passed down as `&mut Random`) instead of a global, so every
// twist engine is deterministic for its seed; templates on filter modes and
// frequency approximations become const generic `u8`s.

#![allow(dead_code)]

use std::f32::consts::PI;
use std::sync::OnceLock;

// ---------------------------------------------------------------- dsp/dsp.h

/// `MAKE_INTEGRAL_FRACTIONAL`: truncation toward zero, like `static_cast<int32_t>`.
#[inline(always)]
pub fn integral_fractional(x: f32) -> (i32, f32) {
    let i = x as i32;
    (i, x - i as f32)
}

#[inline]
pub fn interpolate(table: &[f32], index: f32, size: f32) -> f32 {
    let (i, f) = integral_fractional(index * size);
    let i = i as usize;
    let a = table[i];
    let b = table[i + 1];
    a + (b - a) * f
}

#[inline]
pub fn interpolate_hermite(table: &[f32], index: f32, size: f32) -> f32 {
    let (i, f) = integral_fractional(index * size);
    let i = i as usize;
    let xm1 = table[i - 1];
    let x0 = table[i];
    let x1 = table[i + 1];
    let x2 = table[i + 2];
    let c = (x1 - xm1) * 0.5;
    let v = x0 - x1;
    let w = c + v;
    let a = w + v + (x2 - x0) * 0.5;
    let b_neg = w + a;
    (((a * f) - b_neg) * f + c) * f + x0
}

#[inline]
pub fn interpolate_wrap(table: &[f32], index: f32, size: f32) -> f32 {
    let index = index - (index as i32) as f32;
    let (i, f) = integral_fractional(index * size);
    let i = i as usize;
    let a = table[i];
    let b = table[i + 1];
    a + (b - a) * f
}

#[inline]
pub fn smooth_step(v: f32) -> f32 {
    v * v * (3.0 - 2.0 * v)
}

/// `ONE_POLE(out, in, coefficient)`.
#[inline(always)]
pub fn one_pole(out: &mut f32, input: f32, coefficient: f32) {
    *out += coefficient * (input - *out);
}

/// `SLOPE(out, in, positive, negative)`.
#[inline(always)]
pub fn slope(out: &mut f32, input: f32, positive: f32, negative: f32) {
    let error = input - *out;
    *out += (if error > 0.0 { positive } else { negative }) * error;
}

/// `SLEW(out, in, delta)`.
#[inline(always)]
pub fn slew(out: &mut f32, input: f32, delta: f32) {
    let mut error = input - *out;
    if error > delta {
        error = delta;
    } else if error < -delta {
        error = -delta;
    }
    *out += error;
}

/// `CONSTRAIN(var, min, max)` (min wins on an inverted range, like the macro).
#[inline(always)]
pub fn constrain(v: f32, min: f32, max: f32) -> f32 {
    if v < min {
        min
    } else if v > max {
        max
    } else {
        v
    }
}

#[inline(always)]
pub fn constrain_i(v: i32, min: i32, max: i32) -> i32 {
    if v < min {
        min
    } else if v > max {
        max
    } else {
        v
    }
}

#[inline(always)]
pub fn crossfade(a: f32, b: f32, fade: f32) -> f32 {
    a + (b - a) * fade
}

#[inline(always)]
pub fn soft_limit(x: f32) -> f32 {
    x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
}

#[inline(always)]
pub fn soft_clip(x: f32) -> f32 {
    if x < -3.0 {
        -1.0
    } else if x > 3.0 {
        1.0
    } else {
        soft_limit(x)
    }
}

#[inline(always)]
pub fn clip16(x: i32) -> i32 {
    x.clamp(-32768, 32767)
}

#[inline(always)]
pub fn clip_u16(x: i32) -> i32 {
    x.clamp(0, 65535)
}

#[inline(always)]
pub fn soft_convert(x: f32) -> i16 {
    clip16((soft_limit(x * 0.5) * 32768.0) as i32) as i16
}

// ------------------------------------------------------- dsp/units.h + .cc

struct PitchTables {
    high: [f32; 257],
    low: [f32; 257],
}

fn pitch_tables() -> &'static PitchTables {
    static T: OnceLock<PitchTables> = OnceLock::new();
    T.get_or_init(|| {
        // lut_pitch_ratio_high[i] = 2^((i - 128) / 12),
        // lut_pitch_ratio_low[i] = 2^(i / 256 / 12).
        let mut high = [0.0f32; 257];
        let mut low = [0.0f32; 257];
        for i in 0..257 {
            high[i] = 2.0f64.powf((i as f64 - 128.0) / 12.0) as f32;
            low[i] = 2.0f64.powf(i as f64 / 256.0 / 12.0) as f32;
        }
        PitchTables { high, low }
    })
}

/// Forces the pitch tables (call from constructors so the render path never
/// initialises them).
pub fn init_tables() {
    let _ = pitch_tables();
}

/// Valid for semitones in [-128, 128].
#[inline]
pub fn semitones_to_ratio(semitones: f32) -> f32 {
    let t = pitch_tables();
    let (i, f) = integral_fractional(semitones + 128.0);
    let i = (i.clamp(0, 256)) as usize;
    let j = ((f * 256.0) as i32).clamp(0, 256) as usize;
    t.high[i] * t.low[j]
}

pub fn semitones_to_ratio_safe(mut semitones: f32) -> f32 {
    let mut scale = 1.0f32;
    while semitones > 120.0 {
        semitones -= 120.0;
        scale *= 1024.0;
    }
    while semitones < -120.0 {
        semitones += 120.0;
        scale *= 1.0 / 1024.0;
    }
    scale * semitones_to_ratio(semitones)
}

#[inline]
pub fn exp2_safe(value: f32) -> f32 {
    semitones_to_ratio_safe(value * 12.0)
}

// ------------------------------------------------------------ dsp/filter.h

pub const FILTER_MODE_LOW_PASS: u8 = 0;
pub const FILTER_MODE_BAND_PASS: u8 = 1;
pub const FILTER_MODE_BAND_PASS_NORMALIZED: u8 = 2;
pub const FILTER_MODE_HIGH_PASS: u8 = 3;

pub const FREQUENCY_EXACT: u8 = 0;
pub const FREQUENCY_ACCURATE: u8 = 1;
pub const FREQUENCY_FAST: u8 = 2;
pub const FREQUENCY_DIRTY: u8 = 3;

const PI_D: f64 = std::f64::consts::PI;
const PI_POW_3: f64 = PI_D * PI_D * PI_D;
const PI_POW_5: f64 = PI_POW_3 * PI_D * PI_D;
const PI_POW_7: f64 = PI_POW_5 * PI_D * PI_D;
const PI_POW_9: f64 = PI_POW_7 * PI_D * PI_D;
const PI_POW_11: f64 = PI_POW_9 * PI_D * PI_D;

/// `OnePole::tan<approximation>(f)`.
#[inline]
pub fn tan_approx<const A: u8>(f: f32) -> f32 {
    match A {
        FREQUENCY_EXACT => {
            let f = if f < 0.497 { f } else { 0.497 };
            (PI * f).tan()
        }
        FREQUENCY_DIRTY => {
            let a = (3.736e-01 * PI_POW_3) as f32;
            f * (PI + a * f * f)
        }
        FREQUENCY_FAST => {
            let a = (3.260e-01 * PI_POW_3) as f32;
            let b = (1.823e-01 * PI_POW_5) as f32;
            let f2 = f * f;
            f * (PI + f2 * (a + b * f2))
        }
        _ => {
            let a = (3.333314036e-01 * PI_POW_3) as f32;
            let b = (1.333923995e-01 * PI_POW_5) as f32;
            let c = (5.33740603e-02 * PI_POW_7) as f32;
            let d = (2.900525e-03 * PI_POW_9) as f32;
            let e = (9.5168091e-03 * PI_POW_11) as f32;
            let f2 = f * f;
            f * (PI + f2 * (a + f2 * (b + f2 * (c + f2 * (d + f2 * e)))))
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DcBlocker {
    pole: f32,
    x: f32,
    y: f32,
}

impl DcBlocker {
    pub fn init(&mut self, pole: f32) {
        self.x = 0.0;
        self.y = 0.0;
        self.pole = pole;
    }
    #[inline]
    pub fn process(&mut self, in_out: &mut [f32]) {
        let mut x = self.x;
        let mut y = self.y;
        let pole = self.pole;
        for s in in_out.iter_mut() {
            let old_x = x;
            x = *s;
            y = y * pole + x - old_x;
            *s = y;
        }
        self.x = x;
        self.y = y;
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OnePole {
    g: f32,
    gi: f32,
    state: f32,
}

impl OnePole {
    pub fn init(&mut self) {
        self.set_f::<FREQUENCY_DIRTY>(0.01);
        self.reset();
    }
    pub fn reset(&mut self) {
        self.state = 0.0;
    }
    #[inline]
    pub fn set_f<const A: u8>(&mut self, f: f32) {
        self.g = tan_approx::<A>(f);
        self.gi = 1.0 / (1.0 + self.g);
    }
    #[inline]
    pub fn process<const M: u8>(&mut self, input: f32) -> f32 {
        let lp = (self.g * input + self.state) * self.gi;
        self.state = self.g * (input - lp) + lp;
        match M {
            FILTER_MODE_LOW_PASS => lp,
            FILTER_MODE_HIGH_PASS => input - lp,
            _ => 0.0,
        }
    }
    #[inline]
    pub fn process_buffer<const M: u8>(&mut self, in_out: &mut [f32]) {
        for s in in_out.iter_mut() {
            *s = self.process::<M>(*s);
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Svf {
    g: f32,
    r: f32,
    h: f32,
    state_1: f32,
    state_2: f32,
}

#[inline(always)]
fn svf_pick<const M: u8>(hp: f32, bp: f32, lp: f32, r: f32) -> f32 {
    match M {
        FILTER_MODE_LOW_PASS => lp,
        FILTER_MODE_BAND_PASS => bp,
        FILTER_MODE_BAND_PASS_NORMALIZED => bp * r,
        _ => hp,
    }
}

impl Svf {
    pub fn init(&mut self) {
        self.set_f_q::<FREQUENCY_DIRTY>(0.01, 100.0);
        self.reset();
    }
    pub fn reset(&mut self) {
        self.state_1 = 0.0;
        self.state_2 = 0.0;
    }
    /// Copy settings from another filter.
    #[inline]
    pub fn set(&mut self, f: &Svf) {
        self.g = f.g;
        self.r = f.r;
        self.h = f.h;
    }
    #[inline]
    pub fn set_g_r_h(&mut self, g: f32, r: f32, h: f32) {
        self.g = g;
        self.r = r;
        self.h = h;
    }
    #[inline]
    pub fn set_g_r(&mut self, g: f32, r: f32) {
        self.g = g;
        self.r = r;
        self.h = 1.0 / (1.0 + r * g + g * g);
    }
    #[inline]
    pub fn set_g_q(&mut self, g: f32, resonance: f32) {
        self.g = g;
        self.r = 1.0 / resonance;
        self.h = 1.0 / (1.0 + self.r * g + g * g);
    }
    #[inline]
    pub fn set_f_q<const A: u8>(&mut self, f: f32, resonance: f32) {
        self.g = tan_approx::<A>(f);
        self.r = 1.0 / resonance;
        self.h = 1.0 / (1.0 + self.r * self.g + self.g * self.g);
    }
    #[inline(always)]
    fn tick(&mut self, input: f32) -> (f32, f32, f32) {
        let hp = (input - self.r * self.state_1 - self.g * self.state_1 - self.state_2) * self.h;
        let bp = self.g * hp + self.state_1;
        self.state_1 = self.g * hp + bp;
        let lp = self.g * bp + self.state_2;
        self.state_2 = self.g * bp + lp;
        (hp, bp, lp)
    }
    #[inline]
    pub fn process<const M: u8>(&mut self, input: f32) -> f32 {
        let (hp, bp, lp) = self.tick(input);
        svf_pick::<M>(hp, bp, lp, self.r)
    }
    /// Two outputs from one tick.
    #[inline]
    pub fn process2<const M1: u8, const M2: u8>(&mut self, input: f32) -> (f32, f32) {
        let (hp, bp, lp) = self.tick(input);
        (svf_pick::<M1>(hp, bp, lp, self.r), svf_pick::<M2>(hp, bp, lp, self.r))
    }
    #[inline]
    pub fn process_buffer<const M: u8>(&mut self, input: &[f32], out: &mut [f32]) {
        for (o, &i) in out.iter_mut().zip(input.iter()) {
            *o = self.process::<M>(i);
        }
    }
    /// In-place variant of `process_buffer`.
    #[inline]
    pub fn process_in_place<const M: u8>(&mut self, in_out: &mut [f32]) {
        for s in in_out.iter_mut() {
            *s = self.process::<M>(*s);
        }
    }
    #[inline]
    pub fn process_add<const M: u8>(&mut self, input: &[f32], out: &mut [f32], gain: f32) {
        for (o, &i) in out.iter_mut().zip(input.iter()) {
            *o += gain * self.process::<M>(i);
        }
    }
    pub fn process_multimode(&mut self, input: &[f32], out: &mut [f32], mode: f32) {
        let hp_gain = if mode < 0.5 { -mode * 2.0 } else { -2.0 + mode * 2.0 };
        let lp_gain = if mode < 0.5 { 1.0 - mode * 2.0 } else { 0.0 };
        let bp_gain = if mode < 0.5 { 0.0 } else { mode * 2.0 - 1.0 };
        for (o, &i) in out.iter_mut().zip(input.iter()) {
            let (hp, bp, lp) = self.tick(i);
            *o = hp_gain * hp + bp_gain * bp + lp_gain * lp;
        }
    }
    pub fn process_multimode_lp_to_hp(&mut self, input: &[f32], out: &mut [f32], mode: f32) {
        let hp_gain = (-mode * 2.0 + 1.0).min(0.0);
        let bp_gain = 1.0 - 2.0 * (mode - 0.5).abs();
        let lp_gain = (1.0 - mode * 2.0).max(0.0);
        for (o, &i) in out.iter_mut().zip(input.iter()) {
            let (hp, bp, lp) = self.tick(i);
            *o = hp_gain * hp + bp_gain * bp + lp_gain * lp;
        }
    }
    /// `Process(in, out_1, out_2, size, gain_1, gain_2)`: adds the filtered
    /// signal into two outputs with two gains.
    pub fn process_add2<const M: u8>(
        &mut self,
        input: &[f32],
        out_1: &mut [f32],
        out_2: &mut [f32],
        gain_1: f32,
        gain_2: f32,
    ) {
        for i in 0..input.len() {
            let v = self.process::<M>(input[i]);
            out_1[i] += v * gain_1;
            out_2[i] += v * gain_2;
        }
    }
    #[inline]
    pub fn g(&self) -> f32 {
        self.g
    }
    #[inline]
    pub fn r(&self) -> f32 {
        self.r
    }
    #[inline]
    pub fn h(&self) -> f32 {
        self.h
    }
}

/// Naive Chamberlin SVF.
#[derive(Clone, Copy, Debug, Default)]
pub struct NaiveSvf {
    f: f32,
    damp: f32,
    lp: f32,
    bp: f32,
}

impl NaiveSvf {
    pub fn init(&mut self) {
        self.set_f_q::<FREQUENCY_DIRTY>(0.01, 100.0);
        self.reset();
    }
    pub fn reset(&mut self) {
        self.lp = 0.0;
        self.bp = 0.0;
    }
    #[inline]
    pub fn set_f_q<const A: u8>(&mut self, f: f32, resonance: f32) {
        if A == FREQUENCY_EXACT {
            let f = if f < 0.497 { f } else { 0.497 };
            self.f = 2.0 * (PI * f).sin();
        } else {
            let f = if f < 0.158 { f } else { 0.158 };
            self.f = 2.0 * PI * f;
        }
        self.damp = 1.0 / resonance;
    }
    #[inline]
    pub fn process<const M: u8>(&mut self, input: f32) -> f32 {
        let bp_normalized = self.bp * self.damp;
        let notch = input - bp_normalized;
        self.lp += self.f * self.bp;
        let hp = notch - self.lp;
        self.bp += self.f * hp;
        match M {
            FILTER_MODE_LOW_PASS => self.lp,
            FILTER_MODE_BAND_PASS => self.bp,
            FILTER_MODE_BAND_PASS_NORMALIZED => bp_normalized,
            _ => hp,
        }
    }
    #[inline]
    pub fn lp(&self) -> f32 {
        self.lp
    }
    #[inline]
    pub fn bp(&self) -> f32 {
        self.bp
    }
    #[inline]
    pub fn process_buffer<const M: u8>(&mut self, input: &[f32], out: &mut [f32]) {
        for (o, &i) in out.iter_mut().zip(input.iter()) {
            *o = self.process::<M>(i);
        }
    }
    pub fn split(&mut self, input: &[f32], low: &mut [f32], high: &mut [f32]) {
        for i in 0..input.len() {
            let bp_normalized = self.bp * self.damp;
            let notch = input[i] - bp_normalized;
            self.lp += self.f * self.bp;
            let hp = notch - self.lp;
            self.bp += self.f * hp;
            low[i] = self.lp;
            high[i] = hp;
        }
    }
    /// `Process(in, out, size, decimate)`: writes one output every `decimate` inputs.
    pub fn process_decimate<const M: u8>(&mut self, input: &[f32], out: &mut [f32], decimate: usize) {
        let mut n = decimate - 1;
        let mut o = 0;
        for &x in input {
            let v = self.process::<M>(x);
            n += 1;
            if n == decimate {
                out[o] = v;
                o += 1;
                n = 0;
            }
        }
    }
}

/// Modified Chamberlin SVF (Duane K. Wise).
#[derive(Clone, Copy, Debug, Default)]
pub struct ModifiedSvf {
    f: f32,
    fq: f32,
    x: f32,
    lp: f32,
    bp: f32,
}

impl ModifiedSvf {
    pub fn init(&mut self) {
        self.reset();
    }
    pub fn reset(&mut self) {
        self.lp = 0.0;
        self.bp = 0.0;
    }
    pub fn set_f_fq(&mut self, f: f32, fq: f32) {
        self.f = f;
        self.fq = fq;
        self.x = 0.0;
    }
    pub fn process_buffer<const M: u8>(&mut self, input: &[f32], out: &mut [f32]) {
        let (mut lp, mut bp, mut x) = (self.lp, self.bp, self.x);
        let (fq, f) = (self.fq, self.f);
        for (o, &i) in out.iter_mut().zip(input.iter()) {
            lp += f * bp;
            bp += -fq * bp - f * lp + i;
            if M == FILTER_MODE_BAND_PASS || M == FILTER_MODE_BAND_PASS_NORMALIZED {
                bp += x;
            }
            x = i;
            *o = match M {
                FILTER_MODE_LOW_PASS => lp * f,
                FILTER_MODE_BAND_PASS => bp * f,
                FILTER_MODE_BAND_PASS_NORMALIZED => bp * fq,
                _ => x - lp * f - bp * fq,
            };
        }
        self.lp = lp;
        self.bp = bp;
        self.x = x;
    }
}

// ------------------------------------------------- dsp/cosine_oscillator.h

pub const COSINE_OSCILLATOR_APPROXIMATE: u8 = 0;
pub const COSINE_OSCILLATOR_EXACT: u8 = 1;

/// Cosine between 0 and 1 by a two-pole recursion.
#[derive(Clone, Copy, Debug, Default)]
pub struct CosineOscillator {
    y1: f32,
    y0: f32,
    iir_coefficient: f32,
    initial_amplitude: f32,
}

impl CosineOscillator {
    #[inline]
    pub fn init<const M: u8>(&mut self, frequency: f32) {
        if M == COSINE_OSCILLATOR_APPROXIMATE {
            self.init_approximate(frequency);
        } else {
            self.iir_coefficient = 2.0 * (2.0 * PI * frequency).cos();
            self.initial_amplitude = self.iir_coefficient * 0.25;
        }
        self.start();
    }
    #[inline]
    pub fn init_approximate(&mut self, mut frequency: f32) {
        let mut sign = 16.0f32;
        frequency -= 0.25;
        if frequency < 0.0 {
            frequency = -frequency;
        } else if frequency > 0.5 {
            frequency -= 0.5;
        } else {
            sign = -16.0;
        }
        self.iir_coefficient = sign * frequency * (1.0 - 2.0 * frequency);
        self.initial_amplitude = self.iir_coefficient * 0.25;
    }
    #[inline]
    pub fn start(&mut self) {
        self.y1 = self.initial_amplitude;
        self.y0 = 0.5;
    }
    #[inline]
    pub fn value(&self) -> f32 {
        self.y1 + 0.5
    }
    #[inline]
    pub fn next(&mut self) -> f32 {
        let temp = self.y0;
        self.y0 = self.iir_coefficient * self.y0 - self.y1;
        self.y1 = temp;
        temp + 0.5
    }
}

// ----------------------------------------------- dsp/hysteresis_quantizer.h

#[derive(Clone, Copy, Debug, Default)]
pub struct HysteresisQuantizer {
    quantized_value: i32,
}

impl HysteresisQuantizer {
    pub fn init(&mut self) {
        self.quantized_value = 0;
    }
    #[inline]
    pub fn process(&mut self, value: f32, num_steps: i32) -> i32 {
        self.process_h(0, value, num_steps, 0.25)
    }
    #[inline]
    pub fn process_h(&mut self, base: i32, value: f32, num_steps: i32, hysteresis: f32) -> i32 {
        let mut value = value * (num_steps - 1) as f32;
        value += base as f32;
        let fb = if value > self.quantized_value as f32 { -hysteresis } else { hysteresis };
        let q = constrain_i((value + fb + 0.5) as i32, 0, num_steps - 1);
        self.quantized_value = q;
        q
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HysteresisQuantizer2 {
    num_steps: i32,
    hysteresis: f32,
    scale: f32,
    offset: f32,
    quantized_value: i32,
}

impl HysteresisQuantizer2 {
    pub fn init(&mut self, num_steps: i32, hysteresis: f32, symmetric: bool) {
        self.num_steps = num_steps;
        self.hysteresis = hysteresis;
        self.scale = if symmetric { (num_steps - 1) as f32 } else { num_steps as f32 };
        self.offset = if symmetric { 0.0 } else { -0.5 };
        self.quantized_value = 0;
    }
    #[inline]
    pub fn process(&mut self, value: f32) -> i32 {
        self.process_base(0, value)
    }
    #[inline]
    pub fn process_base(&mut self, base: i32, value: f32) -> i32 {
        let mut value = value * self.scale;
        value += self.offset;
        value += base as f32;
        let sign = if value > self.quantized_value as f32 { -1.0 } else { 1.0 };
        let q = constrain_i((value + sign * self.hysteresis + 0.5) as i32, 0, self.num_steps - 1);
        self.quantized_value = q;
        q
    }
    #[inline]
    pub fn num_steps(&self) -> i32 {
        self.num_steps
    }
    #[inline]
    pub fn quantized_value(&self) -> i32 {
        self.quantized_value
    }
}

// ------------------------------------------------------------ dsp/limiter.h

#[derive(Clone, Copy, Debug, Default)]
pub struct Limiter {
    peak: f32,
}

impl Limiter {
    pub fn init(&mut self) {
        self.peak = 0.5;
    }
    pub fn process(&mut self, pre_gain: f32, in_out: &mut [f32]) {
        for x in in_out.iter_mut() {
            let s = *x * pre_gain;
            slope(&mut self.peak, s.abs(), 0.05, 0.00002);
            let gain = if self.peak <= 1.0 { 1.0 } else { 1.0 / self.peak };
            *x = s * gain * 0.8;
        }
    }
}

// --------------------------------------------- dsp/parameter_interpolator.h

/// Linear ramp of a parameter across a block; writes the reached value back
/// into its state when dropped (like the original's destructor).
pub struct ParameterInterpolator<'a> {
    state: &'a mut f32,
    value: f32,
    increment: f32,
}

impl<'a> ParameterInterpolator<'a> {
    #[inline]
    pub fn new(state: &'a mut f32, new_value: f32, size: usize) -> Self {
        let value = *state;
        let increment = (new_value - value) / size as f32;
        ParameterInterpolator { state, value, increment }
    }
    /// The `(state, new_value, step)` constructor.
    #[inline]
    pub fn with_step(state: &'a mut f32, new_value: f32, step: f32) -> Self {
        let value = *state;
        let increment = (new_value - value) * step;
        ParameterInterpolator { state, value, increment }
    }
    #[inline(always)]
    pub fn next(&mut self) -> f32 {
        self.value += self.increment;
        self.value
    }
    #[inline(always)]
    pub fn subsample(&self, t: f32) -> f32 {
        self.value + self.increment * t
    }
}

impl Drop for ParameterInterpolator<'_> {
    #[inline]
    fn drop(&mut self) {
        *self.state = self.value;
    }
}

// ----------------------------------------------------------- dsp/polyblep.h

#[inline(always)]
pub fn this_blep_sample(t: f32) -> f32 {
    0.5 * t * t
}

#[inline(always)]
pub fn next_blep_sample(t: f32) -> f32 {
    let t = 1.0 - t;
    -0.5 * t * t
}

#[inline(always)]
pub fn next_integrated_blep_sample(t: f32) -> f32 {
    let t1 = 0.5 * t;
    let t2 = t1 * t1;
    let t4 = t2 * t2;
    0.1875 - t1 + 1.5 * t2 - t4
}

#[inline(always)]
pub fn this_integrated_blep_sample(t: f32) -> f32 {
    next_integrated_blep_sample(1.0 - t)
}

// -------------------------------------------------------------- dsp/rsqrt.h

#[inline]
pub fn fast_rsqrt_carmack(x: f32) -> f32 {
    let i = 0x5f3759dfu32.wrapping_sub(x.to_bits() >> 1);
    let y = f32::from_bits(i);
    let x2 = x * 0.5;
    y * (1.5 - (x2 * y * y))
}

#[inline]
pub fn fast_rsqrt_accurate(fp0: f32) -> f32 {
    let min = 1.0e-38f32;
    let p15 = 1.5f32;
    let q = fp0.to_bits();
    let mut fp2 = f32::from_bits(0x5F3997BBu32.wrapping_sub((q >> 1) & 0x3FFFFFFF));
    let fp1 = p15 * fp0 - fp0;
    let mut fp3 = fp2 * fp2;
    if fp0 < min {
        return if fp0 > 0.0 { fp2 } else { 1000.0 };
    }
    fp3 = p15 - fp1 * fp3;
    fp2 *= fp3;
    fp3 = fp2 * fp2;
    fp3 = p15 - fp1 * fp3;
    fp2 *= fp3;
    fp3 = fp2 * fp2;
    fp3 = p15 - fp1 * fp3;
    fp2 * fp3
}

// --------------------------------------------------------- dsp/delay_line.h

/// Ring delay line of `N` samples (the template's `max_delay`), heap backed.
#[derive(Clone, Debug)]
pub struct DelayLine<const N: usize> {
    line: Box<[f32]>,
    write_ptr: usize,
    delay: usize,
}

impl<const N: usize> Default for DelayLine<N> {
    fn default() -> Self {
        DelayLine { line: vec![0.0; N].into_boxed_slice(), write_ptr: 0, delay: 1 }
    }
}

impl<const N: usize> DelayLine<N> {
    pub fn init(&mut self) {
        self.reset();
    }
    pub fn reset(&mut self) {
        self.line.iter_mut().for_each(|x| *x = 0.0);
        self.delay = 1;
        self.write_ptr = 0;
    }
    #[inline]
    pub fn set_delay(&mut self, delay: usize) {
        self.delay = delay;
    }
    #[inline]
    pub fn write(&mut self, sample: f32) {
        self.line[self.write_ptr] = sample;
        self.write_ptr = (self.write_ptr + N - 1) % N;
    }
    #[inline]
    pub fn allpass(&mut self, sample: f32, delay: usize, coefficient: f32) -> f32 {
        let read = self.line[(self.write_ptr + delay) % N];
        let write = sample + coefficient * read;
        self.write(write);
        -write * coefficient + read
    }
    #[inline]
    pub fn write_read(&mut self, sample: f32, delay: f32) -> f32 {
        self.write(sample);
        self.read_f(delay)
    }
    #[inline]
    pub fn read(&self) -> f32 {
        self.line[(self.write_ptr + self.delay) % N]
    }
    #[inline]
    pub fn read_n(&self, delay: usize) -> f32 {
        self.line[(self.write_ptr + delay) % N]
    }
    #[inline]
    pub fn read_f(&self, delay: f32) -> f32 {
        let (i, f) = integral_fractional(delay);
        let i = i as usize;
        let a = self.line[(self.write_ptr + i) % N];
        let b = self.line[(self.write_ptr + i + 1) % N];
        a + (b - a) * f
    }
    #[inline]
    pub fn read_hermite(&self, delay: f32) -> f32 {
        let (i, f) = integral_fractional(delay);
        let t = self.write_ptr + i as usize + N;
        let xm1 = self.line[(t - 1) % N];
        let x0 = self.line[t % N];
        let x1 = self.line[(t + 1) % N];
        let x2 = self.line[(t + 2) % N];
        let c = (x1 - xm1) * 0.5;
        let v = x0 - x1;
        let w = c + v;
        let a = w + v + (x2 - x0) * 0.5;
        let b_neg = w + a;
        (((a * f) - b_neg) * f + c) * f + x0
    }
}

// ------------------------------------------------------------ utils/dsp.h

#[inline]
pub fn interpolate824_i16(table: &[i16], phase: u32) -> i16 {
    let i = (phase >> 24) as usize;
    let a = table[i] as i32;
    let b = table[i + 1] as i32;
    (a + (((b - a) * ((phase >> 8) & 0xffff) as i32) >> 16)) as i16
}

#[inline]
pub fn interpolate824_u8(table: &[u8], phase: u32) -> i16 {
    let i = (phase >> 24) as usize;
    let a = table[i] as i32;
    let b = table[i + 1] as i32;
    ((a << 8) + (((b - a) * (phase & 0xffffff) as i32) >> 16) - 32768) as i16
}

#[inline]
pub fn interpolate88_i16(table: &[i16], index: u16) -> i16 {
    let i = (index >> 8) as usize;
    let a = table[i] as i32;
    let b = table[i + 1] as i32;
    (a + (((b - a) * (index & 0xff) as i32) >> 8)) as i16
}

#[inline]
pub fn interpolate1022_i16(table: &[i16], phase: u32) -> i16 {
    let i = (phase >> 22) as usize;
    let a = table[i] as i32;
    let b = table[i + 1] as i32;
    (a + (((b - a) * ((phase >> 6) & 0xffff) as i32) >> 16)) as i16
}

#[inline]
pub fn interpolate115_i16(table: &[i16], phase: u16) -> i16 {
    let i = (phase >> 5) as usize;
    let a = table[i] as i32;
    let b = table[i + 1] as i32;
    (a + (((b - a) * (phase & 0x1f) as i32) >> 5)) as i16
}

#[inline]
pub fn mix_i16(a: i16, b: i16, balance: u16) -> i16 {
    ((a as i32 * (65535 - balance as i32) + b as i32 * balance as i32) >> 16) as i16
}

#[inline]
pub fn crossfade_i16(table_a: &[i16], table_b: &[i16], phase: u32, balance: u16) -> i16 {
    let a = interpolate824_i16(table_a, phase) as i32;
    let b = interpolate824_i16(table_b, phase) as i32;
    (a + (((b - a) * balance as i32) >> 16)) as i16
}

#[inline]
pub fn crossfade1022_i16(table_a: &[i16], table_b: &[i16], phase: u32, balance: u16) -> i16 {
    let a = interpolate1022_i16(table_a, phase) as i32;
    let b = interpolate1022_i16(table_b, phase) as i32;
    (a + (((b - a) * balance as i32) >> 16)) as i16
}

#[inline]
pub fn crossfade115_i16(table_a: &[i16], table_b: &[i16], phase: u16, balance: u16) -> i16 {
    let a = interpolate115_i16(table_a, phase) as i32;
    let b = interpolate115_i16(table_b, phase) as i32;
    (a + (((b - a) * balance as i32) >> 16)) as i16
}

// --------------------------------------------------------- utils/random.h

/// The 32-bit linear congruential generator (instance instead of global).
#[derive(Clone, Copy, Debug)]
pub struct Random {
    state: u32,
}

impl Default for Random {
    fn default() -> Self {
        Random { state: 0x21 }
    }
}

impl Random {
    pub fn new(seed: u32) -> Random {
        Random { state: seed }
    }
    #[inline]
    pub fn state(&self) -> u32 {
        self.state
    }
    #[inline]
    pub fn seed(&mut self, seed: u32) {
        self.state = seed;
    }
    #[inline(always)]
    pub fn get_word(&mut self) -> u32 {
        self.state = self.state.wrapping_mul(1664525).wrapping_add(1013904223);
        self.state
    }
    #[inline(always)]
    pub fn get_sample(&mut self) -> i16 {
        (self.get_word() >> 16) as u16 as i16
    }
    /// Uniform in [0, 1).
    #[inline(always)]
    pub fn get_float(&mut self) -> f32 {
        // Top 24 bits: exact in f32, so the result never rounds up to 1.0
        // (the original's word-to-float cast could).
        (self.get_word() >> 8) as f32 * (1.0 / 16777216.0)
    }
}
