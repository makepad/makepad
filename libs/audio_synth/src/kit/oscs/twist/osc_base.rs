// Rust port of the twist engine's shared oscillators: dsp/oscillator/oscillator.h
// (band-limited single-waveform oscillator) and dsp/oscillator/sine_oscillator.h
// (table sine + magic-circle sine) of the MIT eurorack macro-oscillator.
// Original notices:
//
// Copyright 2016 Emilie Gillet. (dsp/oscillator/oscillator.h, dsp/oscillator/sine_oscillator.h)
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

#![allow(dead_code)]

use super::stm::*;
use super::tables::tables;

// ------------------------------------------------------------ oscillator.h

pub const OSCILLATOR_SHAPE_IMPULSE_TRAIN: u8 = 0;
pub const OSCILLATOR_SHAPE_SAW: u8 = 1;
pub const OSCILLATOR_SHAPE_TRIANGLE: u8 = 2;
pub const OSCILLATOR_SHAPE_SLOPE: u8 = 3;
pub const OSCILLATOR_SHAPE_SQUARE: u8 = 4;
pub const OSCILLATOR_SHAPE_SQUARE_BRIGHT: u8 = 5;
pub const OSCILLATOR_SHAPE_SQUARE_DARK: u8 = 6;
pub const OSCILLATOR_SHAPE_SQUARE_TRIANGLE: u8 = 7;

pub const MAX_FREQUENCY: f32 = 0.25;
pub const MIN_FREQUENCY: f32 = 0.000001;

#[derive(Clone, Copy, Debug)]
pub struct Oscillator {
    phase: f32,
    next_sample: f32,
    lp_state: f32,
    hp_state: f32,
    high: bool,
    frequency: f32,
    pw: f32,
}

impl Default for Oscillator {
    fn default() -> Self {
        let mut o = Oscillator { phase: 0.0, next_sample: 0.0, lp_state: 0.0, hp_state: 0.0, high: false, frequency: 0.0, pw: 0.0 };
        o.init();
        o
    }
}

impl Oscillator {
    pub fn init(&mut self) {
        self.phase = 0.5;
        self.next_sample = 0.0;
        self.lp_state = 1.0;
        self.hp_state = 0.0;
        self.high = true;
        self.frequency = 0.001;
        self.pw = 0.5;
    }

    /// `Render<shape>(frequency, pw, out, size)`.
    #[inline]
    pub fn render<const SHAPE: u8>(&mut self, frequency: f32, pw: f32, out: &mut [f32]) {
        self.render_full::<SHAPE, false, false>(frequency, pw, None, out);
    }

    /// `Render<shape>(frequency, pw, fm, out, size)`: through-zero FM when `fm` is given.
    #[inline]
    pub fn render_fm<const SHAPE: u8>(&mut self, frequency: f32, pw: f32, fm: Option<&[f32]>, out: &mut [f32]) {
        if fm.is_none() {
            self.render_full::<SHAPE, false, false>(frequency, pw, None, out);
        } else {
            self.render_full::<SHAPE, true, true>(frequency, pw, fm, out);
        }
    }

    pub fn render_full<const SHAPE: u8, const EXT_FM: bool, const TZ_FM: bool>(
        &mut self,
        mut frequency: f32,
        mut pw: f32,
        external_fm: Option<&[f32]>,
        out: &mut [f32],
    ) {
        let size = out.len();
        if !EXT_FM {
            if !TZ_FM {
                frequency = constrain(frequency, MIN_FREQUENCY, MAX_FREQUENCY);
            } else {
                frequency = constrain(frequency, -MAX_FREQUENCY, MAX_FREQUENCY);
            }
            pw = constrain(pw, frequency.abs() * 2.0, 1.0 - 2.0 * frequency.abs());
        }
        let mut phase = self.phase;
        let mut high = self.high;
        let mut lp_state = self.lp_state;
        let mut hp_state = self.hp_state;
        let mut next_sample = self.next_sample;
        {
            let mut fm = ParameterInterpolator::new(&mut self.frequency, frequency, size);
            let mut pwm = ParameterInterpolator::new(&mut self.pw, pw, size);
            for i in 0..size {
                let mut this_sample = next_sample;
                next_sample = 0.0;
                let mut frequency = fm.next();
                if EXT_FM {
                    if let Some(e) = external_fm {
                        frequency *= 1.0 + e[i];
                    }
                    if !TZ_FM {
                        frequency = constrain(frequency, MIN_FREQUENCY, MAX_FREQUENCY);
                    } else {
                        frequency = constrain(frequency, -MAX_FREQUENCY, MAX_FREQUENCY);
                    }
                }
                let mut pw = if SHAPE == OSCILLATOR_SHAPE_SQUARE_TRIANGLE || SHAPE == OSCILLATOR_SHAPE_TRIANGLE {
                    0.5
                } else {
                    pwm.next()
                };
                if EXT_FM {
                    pw = constrain(pw, frequency.abs() * 2.0, 1.0 - 2.0 * frequency.abs());
                }
                phase += frequency;

                if SHAPE <= OSCILLATOR_SHAPE_SAW {
                    if phase >= 1.0 {
                        phase -= 1.0;
                        let t = phase / frequency;
                        this_sample -= this_blep_sample(t);
                        next_sample -= next_blep_sample(t);
                    } else if TZ_FM && phase < 0.0 {
                        let t = phase / frequency;
                        phase += 1.0;
                        this_sample += this_blep_sample(t);
                        next_sample += next_blep_sample(t);
                    }
                    next_sample += phase;
                    if SHAPE == OSCILLATOR_SHAPE_SAW {
                        out[i] = 2.0 * this_sample - 1.0;
                    } else {
                        lp_state += 0.25 * ((hp_state - this_sample) - lp_state);
                        out[i] = 4.0 * lp_state;
                        hp_state = this_sample;
                    }
                } else if SHAPE <= OSCILLATOR_SHAPE_SLOPE {
                    let mut slope_up = 2.0;
                    let mut slope_down = 2.0;
                    if SHAPE == OSCILLATOR_SHAPE_SLOPE {
                        slope_up = 1.0 / pw;
                        slope_down = 1.0 / (1.0 - pw);
                    }
                    if high ^ (phase < pw) {
                        let t = (phase - pw) / frequency;
                        let mut discontinuity = (slope_up + slope_down) * frequency;
                        if TZ_FM && frequency < 0.0 {
                            discontinuity = -discontinuity;
                        }
                        this_sample -= this_integrated_blep_sample(t) * discontinuity;
                        next_sample -= next_integrated_blep_sample(t) * discontinuity;
                        high = phase < pw;
                    }
                    if phase >= 1.0 {
                        phase -= 1.0;
                        let t = phase / frequency;
                        let discontinuity = (slope_up + slope_down) * frequency;
                        this_sample += this_integrated_blep_sample(t) * discontinuity;
                        next_sample += next_integrated_blep_sample(t) * discontinuity;
                        high = true;
                    } else if TZ_FM && phase < 0.0 {
                        let t = phase / frequency;
                        phase += 1.0;
                        let discontinuity = (slope_up + slope_down) * frequency;
                        this_sample -= this_integrated_blep_sample(t) * discontinuity;
                        next_sample -= next_integrated_blep_sample(t) * discontinuity;
                        high = false;
                    }
                    next_sample += if high { phase * slope_up } else { 1.0 - (phase - pw) * slope_down };
                    out[i] = 2.0 * this_sample - 1.0;
                } else {
                    if high ^ (phase >= pw) {
                        let t = (phase - pw) / frequency;
                        let mut discontinuity = 1.0;
                        if TZ_FM && frequency < 0.0 {
                            discontinuity = -discontinuity;
                        }
                        this_sample += this_blep_sample(t) * discontinuity;
                        next_sample += next_blep_sample(t) * discontinuity;
                        high = phase >= pw;
                    }
                    if phase >= 1.0 {
                        phase -= 1.0;
                        let t = phase / frequency;
                        this_sample -= this_blep_sample(t);
                        next_sample -= next_blep_sample(t);
                        high = false;
                    } else if TZ_FM && phase < 0.0 {
                        let t = phase / frequency;
                        phase += 1.0;
                        this_sample += this_blep_sample(t);
                        next_sample += next_blep_sample(t);
                        high = true;
                    }
                    next_sample += if phase < pw { 0.0 } else { 1.0 };

                    if SHAPE == OSCILLATOR_SHAPE_SQUARE_TRIANGLE {
                        let integrator_coefficient = frequency * 0.0625;
                        this_sample = 128.0 * (this_sample - 0.5);
                        lp_state += integrator_coefficient * (this_sample - lp_state);
                        out[i] = lp_state;
                    } else if SHAPE == OSCILLATOR_SHAPE_SQUARE_DARK {
                        let integrator_coefficient = frequency * 2.0;
                        this_sample = 4.0 * (this_sample - 0.5);
                        lp_state += integrator_coefficient * (this_sample - lp_state);
                        out[i] = lp_state;
                    } else if SHAPE == OSCILLATOR_SHAPE_SQUARE_BRIGHT {
                        let integrator_coefficient = frequency * 2.0;
                        this_sample = 2.0 * this_sample - 1.0;
                        lp_state += integrator_coefficient * (this_sample - lp_state);
                        out[i] = (this_sample - lp_state) * 0.5;
                    } else {
                        this_sample = 2.0 * this_sample - 1.0;
                        out[i] = this_sample;
                    }
                }
            }
        }
        self.phase = phase;
        self.high = high;
        self.lp_state = lp_state;
        self.hp_state = hp_state;
        self.next_sample = next_sample;
    }
}

// ------------------------------------------------------- sine_oscillator.h

pub const SINE_LUT_SIZE: f32 = 512.0;
pub const SINE_LUT_QUADRATURE: usize = 128;
pub const SINE_LUT_BITS: u32 = 9;

/// Safe for phase >= 0, wraps.
#[inline]
pub fn sine(phase: f32) -> f32 {
    interpolate_wrap(&tables().sine, phase, SINE_LUT_SIZE)
}

/// Unsafe beyond phase 1.25.
#[inline]
pub fn sine_no_wrap(phase: f32) -> f32 {
    interpolate(&tables().sine, phase, SINE_LUT_SIZE)
}

/// Phase modulation of up to +-32 cycles.
#[inline]
pub fn sine_pm(phase: u32, pm: f32) -> f32 {
    let max_uint32 = 4294967296.0f32;
    let max_index: i32 = 32;
    let offset = max_index as f32;
    let scale = max_uint32 / (max_index * 2) as f32;
    let phase = phase.wrapping_add((((pm + offset) * scale) as u32).wrapping_mul((max_index * 2) as u32));
    let integral = (phase >> (32 - SINE_LUT_BITS)) as usize;
    let fractional = (phase << SINE_LUT_BITS) as f32 / max_uint32;
    let t = &tables().sine;
    let a = t[integral];
    let b = t[integral + 1];
    a + (b - a) * fractional
}

/// Direct lookup without interpolation.
#[inline]
pub fn sine_raw(phase: u32) -> f32 {
    tables().sine[(phase >> (32 - SINE_LUT_BITS)) as usize]
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SineOscillator {
    phase: f32,
    frequency: f32,
    amplitude: f32,
}

impl SineOscillator {
    pub fn init(&mut self) {
        self.phase = 0.0;
        self.frequency = 0.0;
        self.amplitude = 0.0;
    }

    #[inline]
    pub fn next(&mut self, mut frequency: f32) -> f32 {
        if frequency >= 0.5 {
            frequency = 0.5;
        }
        self.phase += frequency;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        sine_no_wrap(self.phase)
    }

    /// Returns (sin, cos) scaled by `amplitude`.
    #[inline]
    pub fn next_quadrature(&mut self, mut frequency: f32, amplitude: f32) -> (f32, f32) {
        if frequency >= 0.5 {
            frequency = 0.5;
        }
        self.phase += frequency;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        (amplitude * sine_no_wrap(self.phase), amplitude * sine_no_wrap(self.phase + 0.25))
    }

    /// Additive render (`out += amplitude * sine`).
    pub fn render_add(&mut self, frequency: f32, amplitude: f32, out: &mut [f32]) {
        self.render_internal::<true>(frequency, amplitude, out);
    }

    pub fn render(&mut self, frequency: f32, out: &mut [f32]) {
        self.render_internal::<false>(frequency, 1.0, out);
    }

    fn render_internal<const ADDITIVE: bool>(&mut self, mut frequency: f32, amplitude: f32, out: &mut [f32]) {
        if frequency >= 0.5 {
            frequency = 0.5;
        }
        let size = out.len();
        let mut phase = self.phase;
        {
            let mut fm = ParameterInterpolator::new(&mut self.frequency, frequency, size);
            let mut am = ParameterInterpolator::new(&mut self.amplitude, amplitude, size);
            for o in out.iter_mut() {
                phase += fm.next();
                if phase >= 1.0 {
                    phase -= 1.0;
                }
                let s = sine_no_wrap(phase);
                if ADDITIVE {
                    *o += am.next() * s;
                } else {
                    *o = s;
                }
            }
        }
        self.phase = phase;
    }
}

const FAST_SINE_NORMAL: u8 = 0;
const FAST_SINE_ADDITIVE: u8 = 1;
const FAST_SINE_QUADRATURE: u8 = 2;

#[derive(Clone, Copy, Debug)]
pub struct FastSineOscillator {
    x: f32,
    y: f32,
    epsilon: f32,
    amplitude: f32,
}

impl Default for FastSineOscillator {
    fn default() -> Self {
        FastSineOscillator { x: 1.0, y: 0.0, epsilon: 0.0, amplitude: 0.0 }
    }
}

impl FastSineOscillator {
    pub fn init(&mut self) {
        self.x = 1.0;
        self.y = 0.0;
        self.epsilon = 0.0;
        self.amplitude = 0.0;
    }

    /// 2 sin(pi f) by a corrected 3rd-order polynomial.
    #[inline]
    pub fn fast_2_sin(f: f32) -> f32 {
        let f_pi = f * std::f32::consts::PI;
        f_pi * (2.0 - (2.0 * 0.96 / 6.0) * f_pi * f_pi)
    }

    pub fn render(&mut self, frequency: f32, out: &mut [f32]) {
        self.render_internal::<FAST_SINE_NORMAL>(frequency, 1.0, out, None);
    }

    pub fn render_add(&mut self, frequency: f32, amplitude: f32, out: &mut [f32]) {
        self.render_internal::<FAST_SINE_ADDITIVE>(frequency, amplitude, out, None);
    }

    pub fn render_quadrature(&mut self, frequency: f32, amplitude: f32, x: &mut [f32], y: &mut [f32]) {
        self.render_internal::<FAST_SINE_QUADRATURE>(frequency, amplitude, x, Some(y));
    }

    fn render_internal<const MODE: u8>(
        &mut self,
        mut frequency: f32,
        mut amplitude: f32,
        out: &mut [f32],
        mut out_2: Option<&mut [f32]>,
    ) {
        if frequency >= 0.25 {
            frequency = 0.25;
            amplitude = 0.0;
        } else {
            amplitude *= 1.0 - frequency * 4.0;
        }
        let size = out.len();
        let mut x = self.x;
        let mut y = self.y;
        let norm = x * x + y * y;
        if norm <= 0.5 || norm >= 2.0 {
            let scale = fast_rsqrt_carmack(norm);
            x *= scale;
            y *= scale;
        }
        {
            let mut epsilon = ParameterInterpolator::new(&mut self.epsilon, Self::fast_2_sin(frequency), size);
            let mut am = ParameterInterpolator::new(&mut self.amplitude, amplitude, size);
            for i in 0..size {
                let e = epsilon.next();
                x += e * y;
                y -= e * x;
                match MODE {
                    FAST_SINE_ADDITIVE => out[i] += am.next() * x,
                    FAST_SINE_NORMAL => out[i] = x,
                    _ => {
                        let a = am.next();
                        out[i] = x * a;
                        if let Some(o2) = out_2.as_deref_mut() {
                            o2[i] = y * a;
                        }
                    }
                }
            }
        }
        self.x = x;
        self.y = y;
    }
}
