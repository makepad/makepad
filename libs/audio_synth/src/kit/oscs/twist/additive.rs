// Rust port of the twist engine's additive model (engine 4):
// dsp/engine/additive_engine.h + .cc and dsp/oscillator/harmonic_oscillator.h
// of the MIT eurorack macro-oscillator. Original notices:
//
// Copyright 2016 Emilie Gillet. (dsp/engine/additive_engine.h, dsp/engine/additive_engine.cc,
//                                dsp/oscillator/harmonic_oscillator.h)
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
use super::osc_base::{sine, sine_no_wrap};
use super::stm::*;

// ------------------------------------------------ harmonic_oscillator.h

/// Harmonic oscillator based on Chebyshev polynomials: `N` consecutive
/// harmonics starting at `FIRST` (the recurrence restarts from two directly
/// computed harmonics when `FIRST` > 1).
#[derive(Clone, Copy, Debug)]
pub struct HarmonicOscillator<const N: usize> {
    phase: f32,
    frequency: f32,
    amplitude: [f32; N],
}

impl<const N: usize> Default for HarmonicOscillator<N> {
    fn default() -> Self {
        HarmonicOscillator { phase: 0.0, frequency: 0.0, amplitude: [0.0; N] }
    }
}

impl<const N: usize> HarmonicOscillator<N> {
    pub fn init(&mut self) {
        *self = Self::default();
    }

    pub fn render<const FIRST: usize>(&mut self, mut frequency: f32, amplitudes: &[f32], out: &mut [f32]) {
        if frequency >= 0.5 {
            frequency = 0.5;
        }
        let size = out.len();
        // One linear ramp per harmonic (the original's array of interpolators).
        let mut am_value = self.amplitude;
        let mut am_inc = [0.0f32; N];
        for i in 0..N {
            let mut f = frequency * (FIRST + i) as f32;
            if f >= 0.5 {
                f = 0.5;
            }
            am_inc[i] = (amplitudes[i] * (1.0 - f * 2.0) - am_value[i]) / size as f32;
        }
        let mut phase = self.phase;
        {
            let mut fm = ParameterInterpolator::new(&mut self.frequency, frequency, size);
            for o in out.iter_mut() {
                phase += fm.next();
                if phase >= 1.0 {
                    phase -= 1.0;
                }
                let two_x = 2.0 * sine_no_wrap(phase);
                let (mut previous, mut current);
                if FIRST == 1 {
                    previous = 1.0;
                    current = two_x * 0.5;
                } else {
                    let k = FIRST as f32;
                    previous = sine(phase * (k - 1.0) + 0.25);
                    current = sine(phase * k);
                }
                let mut sum = 0.0;
                for i in 0..N {
                    am_value[i] += am_inc[i];
                    sum += am_value[i] * current;
                    let temp = current;
                    current = two_x * current - previous;
                    previous = temp;
                }
                if FIRST == 1 {
                    *o = sum;
                } else {
                    *o += sum;
                }
            }
        }
        self.phase = phase;
        self.amplitude = am_value;
    }
}

// ------------------------------------------------------- additive_engine

const HARMONIC_BATCH_SIZE: usize = 12;
const NUM_HARMONICS: usize = 36;
const NUM_HARMONIC_OSCILLATORS: usize = NUM_HARMONICS / HARMONIC_BATCH_SIZE;

const INTEGER_HARMONICS: [usize; 24] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23];
const ORGAN_HARMONICS: [usize; 8] = [0, 1, 2, 3, 5, 7, 9, 11];

/// Additive synthesis with 24 + 8 partials.
pub struct AdditiveEngine {
    harmonic_oscillator: [HarmonicOscillator<HARMONIC_BATCH_SIZE>; NUM_HARMONIC_OSCILLATORS],
    amplitudes: [f32; NUM_HARMONICS],
}

impl AdditiveEngine {
    pub fn new() -> Self {
        let mut e = AdditiveEngine {
            harmonic_oscillator: [HarmonicOscillator::default(); NUM_HARMONIC_OSCILLATORS],
            amplitudes: [0.0; NUM_HARMONICS],
        };
        for o in e.harmonic_oscillator.iter_mut() {
            o.init();
        }
        e
    }

    fn update_amplitudes(centroid: f32, slope: f32, bumps: f32, amplitudes: &mut [f32], harmonic_indices: &[usize]) {
        let num_harmonics = harmonic_indices.len();
        let n = num_harmonics as f32 - 1.0;
        let margin = (1.0 / slope - 1.0) / (1.0 + bumps);
        let center = centroid * (n + margin) - 0.5 * margin;
        let mut sum = 0.001f32;
        for (i, &j) in harmonic_indices.iter().enumerate() {
            let order = (i as f32 - center).abs() * slope;
            let mut gain = 1.0 - order;
            gain += gain.abs();
            gain *= gain;
            let b = 0.25 + order * bumps;
            let bump_factor = 1.0 + sine(b);
            gain *= bump_factor;
            gain *= gain;
            gain *= gain;
            // Not a proper low-pass because of the normalisation, but the
            // original keeps it this way on purpose.
            one_pole(&mut amplitudes[j], gain, 0.001);
            sum += amplitudes[j];
        }
        sum = 1.0 / sum;
        for &j in harmonic_indices {
            amplitudes[j] *= sum;
        }
    }
}

impl Engine for AdditiveEngine {
    fn reset(&mut self) {
        self.amplitudes = [0.0; NUM_HARMONICS];
    }

    fn render(
        &mut self,
        p: &EngineParameters,
        out: &mut [f32],
        aux: &mut [f32],
        _already_enveloped: &mut bool,
        _rng: &mut Random,
    ) {
        let f0 = note_to_frequency(p.note);
        let centroid = p.timbre;
        let raw_bumps = p.harmonics;
        let raw_slope = (1.0 - 0.6 * raw_bumps) * p.morph;
        let slope = 0.01 + 1.99 * raw_slope * raw_slope * raw_slope;
        let bumps = 16.0 * raw_bumps * raw_bumps;
        Self::update_amplitudes(centroid, slope, bumps, &mut self.amplitudes[0..24], &INTEGER_HARMONICS);
        self.harmonic_oscillator[0].render::<1>(f0, &self.amplitudes[0..12], out);
        self.harmonic_oscillator[1].render::<13>(f0, &self.amplitudes[12..24], out);
        Self::update_amplitudes(centroid, slope, bumps, &mut self.amplitudes[24..36], &ORGAN_HARMONICS);
        self.harmonic_oscillator[2].render::<1>(f0, &self.amplitudes[24..36], aux);
    }
}
