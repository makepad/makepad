// Rust port of the twist engine's virtual analog model (engine 0):
// dsp/engine/virtual_analog_engine.h + .cc of the MIT eurorack macro-oscillator.
// Original notices:
//
// Copyright 2016 Emilie Gillet. (dsp/engine/virtual_analog_engine.h, .cc)
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
use super::osc_variable_saw::VariableSawOscillator;
use super::osc_variable_shape::VariableShapeOscillator;
use super::stm::*;

const INTERVALS: [f32; 5] = [0.0, 7.01, 12.01, 19.01, 24.01];

#[inline]
fn squash(x: f32) -> f32 {
    x * x * (3.0 - 2.0 * x)
}

/// Two variable-shape oscillators with sync and crossfading (the original's
/// variant 2: OUT = variable square + variable saw, AUX = dual "monster" sync).
pub struct VirtualAnalogEngine {
    primary: VariableShapeOscillator,
    auxiliary: VariableShapeOscillator,
    sync: VariableShapeOscillator,
    variable_saw: VariableSawOscillator,
    auxiliary_amount: f32,
    xmod_amount: f32,
    temp_buffer: [f32; MAX_BLOCK_SIZE],
}

impl VirtualAnalogEngine {
    pub fn new() -> Self {
        let mut e = VirtualAnalogEngine {
            primary: VariableShapeOscillator::default(),
            auxiliary: VariableShapeOscillator::default(),
            sync: VariableShapeOscillator::default(),
            variable_saw: VariableSawOscillator::default(),
            auxiliary_amount: 0.0,
            xmod_amount: 0.0,
            temp_buffer: [0.0; MAX_BLOCK_SIZE],
        };
        e.primary.init();
        e.auxiliary.init();
        e.auxiliary.set_master_phase(0.25);
        e.sync.init();
        e.variable_saw.init();
        e
    }

    fn compute_detuning(detune: f32) -> f32 {
        let detune = constrain(2.05 * detune - 1.025, -1.0, 1.0);
        let sign = if detune < 0.0 { -1.0 } else { 1.0 };
        let detune = detune * sign * 3.9999;
        let (i, f) = integral_fractional(detune);
        let a = INTERVALS[i as usize];
        let b = INTERVALS[i as usize + 1];
        (a + (b - a) * squash(squash(f))) * sign
    }
}

impl Engine for VirtualAnalogEngine {
    fn reset(&mut self) {}

    fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32], _already_enveloped: &mut bool, _rng: &mut Random) {
        let size = out.len();
        let sync_amount = p.timbre * p.timbre;
        let auxiliary_detune = Self::compute_detuning(p.harmonics);
        let primary_f = note_to_frequency(p.note);
        let auxiliary_f = note_to_frequency(p.note + auxiliary_detune);
        let primary_sync_f = note_to_frequency(p.note + sync_amount * 48.0);
        let auxiliary_sync_f = note_to_frequency(p.note + auxiliary_detune + sync_amount * 48.0);

        let shape = constrain(p.morph * 1.5, 0.0, 1.0);
        let pw = constrain(0.5 + (p.morph - 0.66) * 1.46, 0.5, 0.995);

        // Monster sync to AUX.
        self.primary.render_sync(primary_f, primary_sync_f, pw, shape, out);
        self.auxiliary.render_sync(auxiliary_f, auxiliary_sync_f, pw, shape, aux);
        for i in 0..size {
            aux[i] = (aux[i] - out[i]) * 0.5;
        }

        // Double varishape to OUT.
        let square_pw = constrain(1.3 * p.timbre - 0.15, 0.005, 0.5);
        let square_sync_ratio = if p.timbre < 0.5 { 0.0 } else { (p.timbre - 0.5) * (p.timbre - 0.5) * 4.0 * 48.0 };
        let square_gain = (p.timbre * 8.0).min(1.0);

        let mut saw_pw = if p.morph < 0.5 { p.morph + 0.5 } else { 1.0 - (p.morph - 0.5) * 2.0 };
        saw_pw *= 1.1;
        let saw_pw = constrain(saw_pw, 0.005, 1.0);
        let saw_shape = constrain(10.0 - 21.0 * p.morph, 0.0, 1.0);
        let saw_gain = constrain(8.0 * (1.0 - p.morph), 0.02, 1.0);

        let square_sync_f = note_to_frequency(p.note + square_sync_ratio);

        let temp = &mut self.temp_buffer[..size];
        self.sync.render_sync(primary_f, square_sync_f, square_pw, 1.0, temp);
        self.variable_saw.render(auxiliary_f, saw_pw, saw_shape, out);

        let norm = 1.0 / square_gain.max(saw_gain);
        let mut square_gain_modulation = ParameterInterpolator::new(&mut self.auxiliary_amount, square_gain * 0.3 * norm, size);
        let mut saw_gain_modulation = ParameterInterpolator::new(&mut self.xmod_amount, saw_gain * 0.5 * norm, size);
        for i in 0..size {
            out[i] = out[i] * saw_gain_modulation.next() + square_gain_modulation.next() * temp[i];
        }
    }
}
