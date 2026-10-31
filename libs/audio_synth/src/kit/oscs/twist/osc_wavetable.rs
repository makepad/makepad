// Rust port of the twist engine's integrated-wavetable oscillator:
// dsp/oscillator/wavetable_oscillator.h of the MIT eurorack macro-oscillator.
// Original notice:
//
// Copyright 2016 Emilie Gillet.
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


use super::osc_base::MAX_FREQUENCY;
use super::stm::*;

/// One-pole smoothed differentiator (reads integrated wavetables).
#[derive(Clone, Copy, Debug, Default)]
pub struct Differentiator {
    lp: f32,
    previous: f32,
}

impl Differentiator {
    pub fn init(&mut self) {
        self.previous = 0.0;
        self.lp = 0.0;
    }
    #[inline]
    pub fn process(&mut self, coefficient: f32, s: f32) -> f32 {
        one_pole(&mut self.lp, s - self.previous, coefficient);
        self.previous = s;
        self.lp
    }
}

#[inline]
pub fn interpolate_wave(table: &[i16], index_integral: i32, index_fractional: f32) -> f32 {
    let i = index_integral as usize;
    let a = table[i] as f32;
    let b = table[i + 1] as f32;
    a + (b - a) * index_fractional
}

#[inline]
pub fn interpolate_wave_hermite(table: &[i16], index_integral: i32, index_fractional: f32) -> f32 {
    let i = index_integral as usize;
    let xm1 = table[i] as f32;
    let x0 = table[i + 1] as f32;
    let x1 = table[i + 2] as f32;
    let x2 = table[i + 3] as f32;
    let c = (x1 - xm1) * 0.5;
    let v = x0 - x1;
    let w = c + v;
    let a = w + v + (x2 - x0) * 0.5;
    let b_neg = w + a;
    let f = index_fractional;
    (((a * f) - b_neg) * f + c) * f + x0
}

/// Integrated-wavetable oscillator over `NUM_WAVES` waves of `SIZE` samples
/// (+ guard samples). A wave `k` is `waves[offsets[k]..]`.
#[derive(Clone, Copy, Debug, Default)]
pub struct WavetableOscillator<const SIZE: usize, const NUM_WAVES: usize, const APPROXIMATE_SCALE: bool, const ATTENUATE_HF: bool> {
    phase: f32,
    frequency: f32,
    amplitude: f32,
    waveform: f32,
    lp: f32,
    differentiator: Differentiator,
}

impl<const SIZE: usize, const NUM_WAVES: usize, const APPROXIMATE_SCALE: bool, const ATTENUATE_HF: bool>
    WavetableOscillator<SIZE, NUM_WAVES, APPROXIMATE_SCALE, ATTENUATE_HF>
{
    pub fn init(&mut self) {
        self.phase = 0.0;
        self.frequency = 0.0;
        self.amplitude = 0.0;
        self.waveform = 0.0;
        self.lp = 0.0;
        self.differentiator.init();
    }

    /// Adds into `out`.
    pub fn render(&mut self, frequency: f32, mut amplitude: f32, waveform: f32, waves: &[i16], offsets: &[usize], out: &mut [f32]) {
        let size = out.len();
        let frequency = constrain(frequency, 0.0000001, MAX_FREQUENCY);
        if ATTENUATE_HF {
            amplitude *= 1.0 - 2.0 * frequency;
        }
        if APPROXIMATE_SCALE {
            amplitude *= 1.0 / (frequency * 131072.0);
        }
        let mut lp = self.lp;
        let mut phase = self.phase;
        {
            let mut frequency_modulation = ParameterInterpolator::new(&mut self.frequency, frequency, size);
            let mut amplitude_modulation = ParameterInterpolator::new(&mut self.amplitude, amplitude, size);
            let mut waveform_modulation =
                ParameterInterpolator::new(&mut self.waveform, waveform * (NUM_WAVES as f32 - 1.0001), size);
            for o in out.iter_mut() {
                let f0 = frequency_modulation.next();
                let cutoff = (SIZE as f32 * f0).min(1.0);
                let scale = if APPROXIMATE_SCALE { 1.0 } else { 1.0 / (f0 * 131072.0) };

                phase += f0;
                if phase >= 1.0 {
                    phase -= 1.0;
                }

                let (wi, wf) = integral_fractional(waveform_modulation.next());
                let (pi, pf) = integral_fractional(phase * SIZE as f32);

                let x0 = interpolate_wave(&waves[offsets[wi as usize]..], pi, pf);
                let x1 = interpolate_wave(&waves[offsets[wi as usize + 1]..], pi, pf);

                let s = self.differentiator.process(cutoff, (x0 + (x1 - x0) * wf) * scale);
                one_pole(&mut lp, s, cutoff);
                *o += amplitude_modulation.next() * lp;
            }
        }
        self.lp = lp;
        self.phase = phase;
    }
}
