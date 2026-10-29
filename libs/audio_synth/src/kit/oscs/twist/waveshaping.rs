// Rust port of the twist engine's waveshaping model (engine 1):
// dsp/engine/waveshaping_engine.h + .cc of the MIT eurorack macro-oscillator.
// Original notices:
//
// Copyright 2016 Emilie Gillet. (dsp/engine/waveshaping_engine.h, .cc)
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


use super::engine::*;
use super::osc_base::*;
use super::stm::*;
use super::tables::tables;

/// Slope -> waveshaper -> wavefolder.
pub struct WaveshapingEngine {
    slope: Oscillator,
    triangle: Oscillator,
    previous_shape: f32,
    previous_wavefolder_gain: f32,
    previous_overtone_gain: f32,
}

impl WaveshapingEngine {
    pub fn new() -> Self {
        let mut e = WaveshapingEngine {
            slope: Oscillator::default(),
            triangle: Oscillator::default(),
            previous_shape: 0.0,
            previous_wavefolder_gain: 0.0,
            previous_overtone_gain: 0.0,
        };
        e.slope.init();
        e.triangle.init();
        e
    }
}

fn tame(f0: f32, harmonics: f32, order: f32) -> f32 {
    let f0 = f0 * harmonics;
    let max_f = 0.5 / order;
    let max_amount = constrain(1.0 - (f0 - max_f) / (0.5 - max_f), 0.0, 1.0);
    max_amount * max_amount * max_amount
}

impl Engine for WaveshapingEngine {
    fn reset(&mut self) {}

    fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32], _already_enveloped: &mut bool, _rng: &mut Random) {
        let size = out.len();
        let t = tables();
        let root = p.note;
        let f0 = note_to_frequency(root);
        let pw = p.morph * 0.45 + 0.5;

        // Start from a band-limited slope signal.
        self.slope.render::<OSCILLATOR_SHAPE_SLOPE>(f0, pw, out);
        self.triangle.render::<OSCILLATOR_SHAPE_SLOPE>(f0, 0.5, aux);

        // Estimate how rich the spectrum is; reduce the waveshaping range accordingly.
        let slope = 3.0 + (p.morph - 0.5).abs() * 5.0;
        let shape_amount = (p.harmonics - 0.5).abs() * 2.0;
        let shape_amount_attenuation = tame(f0, slope, 16.0);
        let wavefolder_gain = p.timbre;
        let wavefolder_gain_attenuation = tame(f0, slope * (3.0 + shape_amount * shape_amount_attenuation * 5.0), 12.0);

        let mut shape_modulation = ParameterInterpolator::new(
            &mut self.previous_shape,
            0.5 + (p.harmonics - 0.5) * shape_amount_attenuation,
            size,
        );
        let mut wf_gain_modulation = ParameterInterpolator::new(
            &mut self.previous_wavefolder_gain,
            0.03 + 0.46 * wavefolder_gain * wavefolder_gain_attenuation,
            size,
        );
        let overtone_gain = p.timbre * (2.0 - p.timbre);
        let mut overtone_gain_modulation =
            ParameterInterpolator::new(&mut self.previous_overtone_gain, overtone_gain * (2.0 - overtone_gain), size);

        let fold_t = &t.fold[1..];
        let fold_2_t = &t.fold_2[1..];
        for i in 0..size {
            let (shape_integral, shape_fractional) = integral_fractional(shape_modulation.next() * 3.9999);
            let shape_1 = &t.ws[shape_integral as usize];
            let shape_2 = &t.ws[shape_integral as usize + 1];

            let (wi, wf) = integral_fractional(127.0 * out[i] + 128.0);
            let wi = (wi & 255) as usize;

            let x0 = shape_1[wi] as f32 / 32768.0;
            let x1 = shape_1[wi + 1] as f32 / 32768.0;
            let x = x0 + (x1 - x0) * wf;

            let y0 = shape_2[wi] as f32 / 32768.0;
            let y1 = shape_2[wi + 1] as f32 / 32768.0;
            let y = y0 + (y1 - y0) * wf;

            let mix = x + (y - x) * shape_fractional;
            let index = mix * wf_gain_modulation.next() + 0.5;
            let fold = interpolate_hermite(fold_t, index, 512.0);
            let fold_2 = -interpolate_hermite(fold_2_t, index, 512.0);

            let sine = sine(aux[i] * 0.25 + 0.5);
            out[i] = fold;
            aux[i] = sine + (fold_2 - sine) * overtone_gain_modulation.next();
        }
    }
}
