// Rust port of the twist engine's grain / formant model (engine 3):
// dsp/engine/grain_engine.h + .cc and its oscillators
// dsp/oscillator/grainlet_oscillator.h, dsp/oscillator/z_oscillator.h and
// dsp/oscillator/vosim_oscillator.h (unused by the engine, as in the
// original) of the MIT eurorack macro-oscillator. Original notices:
//
// Copyright 2016 Emilie Gillet. (dsp/engine/grain_engine.h, dsp/engine/grain_engine.cc,
//                                dsp/oscillator/grainlet_oscillator.h,
//                                dsp/oscillator/z_oscillator.h,
//                                dsp/oscillator/vosim_oscillator.h)
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

use super::engine::{note_to_frequency, Engine, EngineParameters};
use super::osc_base::{sine, MAX_FREQUENCY};
use super::stm::*;

// ------------------------------------------------ grainlet_oscillator.h

/// A phase-distorted single cycle sine times a continuously running sine,
/// synced to a main oscillator.
#[derive(Clone, Copy, Debug, Default)]
pub struct GrainletOscillator {
    carrier_phase: f32,
    formant_phase: f32,
    next_sample: f32,
    carrier_frequency: f32,
    formant_frequency: f32,
    carrier_shape: f32,
    carrier_bleed: f32,
}

impl GrainletOscillator {
    pub fn init(&mut self) {
        *self = GrainletOscillator::default();
    }

    pub fn render(
        &mut self,
        mut carrier_frequency: f32,
        mut formant_frequency: f32,
        carrier_shape: f32,
        carrier_bleed: f32,
        out: &mut [f32],
    ) {
        if carrier_frequency >= MAX_FREQUENCY * 0.5 {
            carrier_frequency = MAX_FREQUENCY * 0.5;
        }
        if formant_frequency >= MAX_FREQUENCY {
            formant_frequency = MAX_FREQUENCY;
        }
        let size = out.len();
        let mut carrier_phase = self.carrier_phase;
        let mut formant_phase = self.formant_phase;
        let mut next_sample = self.next_sample;
        {
            let mut cfm = ParameterInterpolator::new(&mut self.carrier_frequency, carrier_frequency, size);
            let mut ffm = ParameterInterpolator::new(&mut self.formant_frequency, formant_frequency, size);
            let mut csm = ParameterInterpolator::new(&mut self.carrier_shape, carrier_shape, size);
            let mut cbm = ParameterInterpolator::new(&mut self.carrier_bleed, carrier_bleed, size);
            for o in out.iter_mut() {
                let mut this_sample = next_sample;
                next_sample = 0.0;
                let f0 = cfm.next();
                let f1 = ffm.next();
                carrier_phase += f0;
                let reset = carrier_phase >= 1.0;
                if reset {
                    carrier_phase -= 1.0;
                    let reset_time = carrier_phase / f0;
                    let before = Self::grainlet(
                        1.0,
                        formant_phase + (1.0 - reset_time) * f1,
                        csm.subsample(1.0 - reset_time),
                        cbm.subsample(1.0 - reset_time),
                    );
                    let after = Self::grainlet(0.0, 0.0, csm.subsample(1.0), cbm.subsample(1.0));
                    let discontinuity = after - before;
                    this_sample += discontinuity * this_blep_sample(reset_time);
                    next_sample += discontinuity * next_blep_sample(reset_time);
                    formant_phase = reset_time * f1;
                } else {
                    formant_phase += f1;
                    if formant_phase >= 1.0 {
                        formant_phase -= 1.0;
                    }
                }
                next_sample += Self::grainlet(carrier_phase, formant_phase, csm.next(), cbm.next());
                *o = this_sample;
            }
        }
        self.carrier_phase = carrier_phase;
        self.formant_phase = formant_phase;
        self.next_sample = next_sample;
    }

    #[inline]
    fn carrier(mut phase: f32, shape: f32) -> f32 {
        let (shape_integral, shape_fractional) = integral_fractional(shape * 3.0);
        let mut t = 1.0 - shape_fractional;
        if shape_integral == 0 {
            phase *= 1.0 + t * t * t * 15.0;
            if phase >= 1.0 {
                phase = 1.0;
            }
            phase += 0.75;
        } else if shape_integral == 1 {
            let breakpoint = 0.001 + 0.499 * t * t * t;
            if phase < breakpoint {
                phase *= 0.5 / breakpoint;
            } else {
                phase = 0.5 + (phase - breakpoint) * 0.5 / (1.0 - breakpoint);
            }
            phase += 0.75;
        } else {
            t = 1.0 - t;
            phase = 0.25 + phase * (0.5 + t * t * t * 14.5);
            if phase >= 0.75 {
                phase = 0.75;
            }
        }
        (sine(phase) + 1.0) * 0.25
    }

    #[inline]
    fn grainlet(carrier_phase: f32, formant_phase: f32, shape: f32, bleed: f32) -> f32 {
        let carrier = Self::carrier(carrier_phase, shape);
        let formant = sine(formant_phase);
        carrier * (formant + bleed) / (1.0 + bleed)
    }
}

// ------------------------------------------------------ z_oscillator.h

/// Sine multiplied by and synced to a carrier.
#[derive(Clone, Copy, Debug, Default)]
pub struct ZOscillator {
    carrier_phase: f32,
    discontinuity_phase: f32,
    formant_phase: f32,
    next_sample: f32,
    carrier_frequency: f32,
    formant_frequency: f32,
    carrier_shape: f32,
    mode: f32,
}

impl ZOscillator {
    pub fn init(&mut self) {
        *self = ZOscillator::default();
    }

    pub fn render(
        &mut self,
        mut carrier_frequency: f32,
        mut formant_frequency: f32,
        carrier_shape: f32,
        mode: f32,
        out: &mut [f32],
    ) {
        if carrier_frequency >= MAX_FREQUENCY * 0.5 {
            carrier_frequency = MAX_FREQUENCY * 0.5;
        }
        if formant_frequency >= MAX_FREQUENCY {
            formant_frequency = MAX_FREQUENCY;
        }
        let size = out.len();
        let mut carrier_phase = self.carrier_phase;
        let mut discontinuity_phase = self.discontinuity_phase;
        let mut formant_phase = self.formant_phase;
        let mut next_sample = self.next_sample;
        {
            let mut cfm = ParameterInterpolator::new(&mut self.carrier_frequency, carrier_frequency, size);
            let mut ffm = ParameterInterpolator::new(&mut self.formant_frequency, formant_frequency, size);
            let mut csm = ParameterInterpolator::new(&mut self.carrier_shape, carrier_shape, size);
            let mut mm = ParameterInterpolator::new(&mut self.mode, mode, size);
            for o in out.iter_mut() {
                let mut this_sample = next_sample;
                next_sample = 0.0;
                let f0 = cfm.next();
                let f1 = ffm.next();
                discontinuity_phase += 2.0 * f0;
                carrier_phase += f0;
                let reset = discontinuity_phase >= 1.0;
                if reset {
                    discontinuity_phase -= 1.0;
                    let reset_time = discontinuity_phase / (2.0 * f0);
                    let carrier_phase_before = if carrier_phase >= 1.0 { 1.0 } else { 0.5 };
                    let carrier_phase_after = if carrier_phase >= 1.0 { 0.0 } else { 0.5 };
                    let before = Self::z(
                        carrier_phase_before,
                        1.0,
                        formant_phase + (1.0 - reset_time) * f1,
                        csm.subsample(1.0 - reset_time),
                        mm.subsample(1.0 - reset_time),
                    );
                    let after = Self::z(carrier_phase_after, 0.0, 0.0, csm.subsample(1.0), mm.subsample(1.0));
                    let discontinuity = after - before;
                    this_sample += discontinuity * this_blep_sample(reset_time);
                    next_sample += discontinuity * next_blep_sample(reset_time);
                    formant_phase = reset_time * f1;
                    if carrier_phase > 1.0 {
                        carrier_phase = discontinuity_phase * 0.5;
                    }
                } else {
                    formant_phase += f1;
                    if formant_phase >= 1.0 {
                        formant_phase -= 1.0;
                    }
                }
                if carrier_phase >= 1.0 {
                    carrier_phase -= 1.0;
                }
                next_sample += Self::z(carrier_phase, discontinuity_phase, formant_phase, csm.next(), mm.next());
                *o = this_sample;
            }
        }
        self.carrier_phase = carrier_phase;
        self.discontinuity_phase = discontinuity_phase;
        self.formant_phase = formant_phase;
        self.next_sample = next_sample;
    }

    #[inline]
    fn z(c: f32, d: f32, f: f32, mut shape: f32, mode: f32) -> f32 {
        let mut ramp_down = 0.5 * (1.0 + sine(0.5 * d + 0.25));
        let offset;
        let phase_shift;
        if mode < 0.333 {
            offset = 1.0;
            phase_shift = 0.25 + mode * 1.50;
        } else if mode < 0.666 {
            phase_shift = 0.7495 - (mode - 0.33) * 0.75;
            offset = -sine(phase_shift);
        } else {
            phase_shift = 0.7495 - (mode - 0.33) * 0.75;
            offset = 0.001;
        }
        let discontinuity = sine(f + phase_shift);
        let contour = if shape < 0.5 {
            shape *= 2.0;
            if c >= 0.5 {
                ramp_down *= shape;
            }
            1.0 + (sine(c + 0.25) - 1.0) * shape
        } else {
            sine(c + shape * 0.5)
        };
        (ramp_down * (offset + discontinuity) - offset) * contour
    }
}

// -------------------------------------------------- vosim_oscillator.h

/// Two sines multiplied by and synced to a carrier (kept for completeness;
/// the grain engine leaves it out, like the original).
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VosimOscillator {
    carrier_phase: f32,
    formant_1_phase: f32,
    formant_2_phase: f32,
    carrier_frequency: f32,
    formant_1_frequency: f32,
    formant_2_frequency: f32,
    carrier_shape: f32,
}

#[allow(dead_code)]
impl VosimOscillator {
    pub fn init(&mut self) {
        *self = VosimOscillator::default();
    }

    pub fn render(
        &mut self,
        mut carrier_frequency: f32,
        mut formant_frequency_1: f32,
        mut formant_frequency_2: f32,
        carrier_shape: f32,
        out: &mut [f32],
    ) {
        if carrier_frequency >= MAX_FREQUENCY {
            carrier_frequency = MAX_FREQUENCY;
        }
        if formant_frequency_1 >= MAX_FREQUENCY {
            formant_frequency_1 = MAX_FREQUENCY;
        }
        if formant_frequency_2 >= MAX_FREQUENCY {
            formant_frequency_2 = MAX_FREQUENCY;
        }
        let size = out.len();
        let mut carrier_phase = self.carrier_phase;
        let mut f1p = self.formant_1_phase;
        let mut f2p = self.formant_2_phase;
        {
            let mut f0m = ParameterInterpolator::new(&mut self.carrier_frequency, carrier_frequency, size);
            let mut f1m = ParameterInterpolator::new(&mut self.formant_1_frequency, formant_frequency_1, size);
            let mut f2m = ParameterInterpolator::new(&mut self.formant_2_frequency, formant_frequency_2, size);
            let mut csm = ParameterInterpolator::new(&mut self.carrier_shape, carrier_shape, size);
            for o in out.iter_mut() {
                let f0 = f0m.next();
                let f1 = f1m.next();
                let f2 = f2m.next();
                // The original advances by the block's target frequency, not
                // the interpolated one.
                carrier_phase += carrier_frequency;
                if carrier_phase >= 1.0 {
                    carrier_phase -= 1.0;
                    let reset_time = carrier_phase / f0;
                    f1p = reset_time * f1;
                    f2p = reset_time * f2;
                } else {
                    f1p += f1;
                    if f1p >= 1.0 {
                        f1p -= 1.0;
                    }
                    f2p += f2;
                    if f2p >= 1.0 {
                        f2p -= 1.0;
                    }
                }
                let carrier = sine(carrier_phase * 0.5 + 0.25) + 1.0;
                let reset_phase = 0.75 - 0.25 * csm.next();
                let reset_amplitude = sine(reset_phase);
                let formant_0 = sine(f1p + reset_phase) - reset_amplitude;
                let formant_1 = sine(f2p + reset_phase) - reset_amplitude;
                *o = carrier * (formant_0 + formant_1) * 0.25 + reset_amplitude;
            }
        }
        self.carrier_phase = carrier_phase;
        self.formant_1_phase = f1p;
        self.formant_2_phase = f2p;
    }
}

// --------------------------------------------------------- grain_engine

/// Windowed sine segments.
pub struct GrainEngine {
    grainlet: [GrainletOscillator; 2],
    z_oscillator: ZOscillator,
    dc_blocker: [OnePole; 2],
}

impl GrainEngine {
    pub fn new() -> Self {
        let mut e = GrainEngine {
            grainlet: [GrainletOscillator::default(); 2],
            z_oscillator: ZOscillator::default(),
            dc_blocker: [OnePole::default(); 2],
        };
        e.grainlet[0].init();
        e.grainlet[1].init();
        e.z_oscillator.init();
        e.dc_blocker[0].init();
        e.dc_blocker[1].init();
        e
    }
}

impl Engine for GrainEngine {
    fn reset(&mut self) {}

    fn render(
        &mut self,
        p: &EngineParameters,
        out: &mut [f32],
        aux: &mut [f32],
        _already_enveloped: &mut bool,
        _rng: &mut Random,
    ) {
        let root = p.note;
        let f0 = note_to_frequency(root);
        let f1 = note_to_frequency(24.0 + 84.0 * p.timbre);
        let ratio = semitones_to_ratio(-24.0 + 48.0 * p.harmonics);
        let carrier_bleed = if p.harmonics < 0.5 { 1.0 - 2.0 * p.harmonics } else { 0.0 };
        let carrier_bleed_fixed = carrier_bleed * (2.0 - carrier_bleed);
        let carrier_shape = 0.33 + (p.morph - 0.33) * (1.0 - f0 * 24.0).max(0.0);

        self.grainlet[0].render(f0, f1, carrier_shape, carrier_bleed_fixed, out);
        self.grainlet[1].render(f0, f1 * ratio, carrier_shape, carrier_bleed_fixed, aux);
        self.dc_blocker[0].set_f::<FREQUENCY_DIRTY>(0.3 * f0);
        for i in 0..out.len() {
            out[i] = self.dc_blocker[0].process::<FILTER_MODE_HIGH_PASS>(out[i] + aux[i]);
        }

        let cutoff = note_to_frequency(root + 96.0 * p.timbre);
        self.z_oscillator.render(f0, cutoff, p.morph, p.harmonics, aux);
        self.dc_blocker[1].set_f::<FREQUENCY_DIRTY>(0.3 * f0);
        self.dc_blocker[1].process_buffer::<FILTER_MODE_HIGH_PASS>(aux);
    }
}
