// Rust port of the twist engine's variable saw oscillator:
// dsp/oscillator/variable_saw_oscillator.h of the MIT eurorack macro-oscillator.
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

pub const VARIABLE_SAW_NOTCH_DEPTH: f32 = 0.2;

/// Saw with variable slope or notch.
#[derive(Clone, Copy, Debug, Default)]
pub struct VariableSawOscillator {
    phase: f32,
    next_sample: f32,
    previous_pw: f32,
    high: bool,
    frequency: f32,
    pw: f32,
    waveshape: f32,
}

impl VariableSawOscillator {
    pub fn init(&mut self) {
        self.phase = 0.0;
        self.next_sample = 0.0;
        self.previous_pw = 0.5;
        self.high = false;
        self.frequency = 0.01;
        self.pw = 0.5;
        self.waveshape = 0.0;
    }

    #[inline]
    fn compute_naive_sample(phase: f32, pw: f32, slope_up: f32, slope_down: f32, triangle_amount: f32, notch_amount: f32) -> f32 {
        let notch_saw = if phase < pw { phase } else { 1.0 + VARIABLE_SAW_NOTCH_DEPTH };
        let triangle = if phase < pw { phase * slope_up } else { 1.0 - (phase - pw) * slope_down };
        notch_saw * notch_amount + triangle * triangle_amount
    }

    pub fn render(&mut self, mut frequency: f32, mut pw: f32, waveshape: f32, out: &mut [f32]) {
        let size = out.len();
        if frequency >= MAX_FREQUENCY {
            frequency = MAX_FREQUENCY;
        }
        if frequency >= 0.25 {
            pw = 0.5;
        } else {
            pw = constrain(pw, frequency * 2.0, 1.0 - 2.0 * frequency);
        }
        let mut phase = self.phase;
        let mut previous_pw = self.previous_pw;
        let mut high = self.high;
        let mut next_sample = self.next_sample;
        {
            let mut fm = ParameterInterpolator::new(&mut self.frequency, frequency, size);
            let mut pwm = ParameterInterpolator::new(&mut self.pw, pw, size);
            let mut waveshape_modulation = ParameterInterpolator::new(&mut self.waveshape, waveshape, size);
            for o in out.iter_mut() {
                let mut this_sample = next_sample;
                next_sample = 0.0;

                let frequency = fm.next();
                let pw = pwm.next();
                let waveshape = waveshape_modulation.next();
                let triangle_amount = waveshape;
                let notch_amount = 1.0 - waveshape;
                let slope_up = 1.0 / pw;
                let slope_down = 1.0 / (1.0 - pw);

                phase += frequency;

                if !high && phase >= pw {
                    let triangle_step = (slope_up + slope_down) * frequency * triangle_amount;
                    let notch = (VARIABLE_SAW_NOTCH_DEPTH + 1.0 - pw) * notch_amount;
                    let t = (phase - pw) / (previous_pw - pw + frequency);
                    this_sample += notch * this_blep_sample(t);
                    next_sample += notch * next_blep_sample(t);
                    this_sample -= triangle_step * this_integrated_blep_sample(t);
                    next_sample -= triangle_step * next_integrated_blep_sample(t);
                    high = true;
                } else if phase >= 1.0 {
                    phase -= 1.0;
                    let triangle_step = (slope_up + slope_down) * frequency * triangle_amount;
                    let notch = (VARIABLE_SAW_NOTCH_DEPTH + 1.0) * notch_amount;
                    let t = phase / frequency;
                    this_sample -= notch * this_blep_sample(t);
                    next_sample -= notch * next_blep_sample(t);
                    this_sample += triangle_step * this_integrated_blep_sample(t);
                    next_sample += triangle_step * next_integrated_blep_sample(t);
                    high = false;
                }

                next_sample += Self::compute_naive_sample(phase, pw, slope_up, slope_down, triangle_amount, notch_amount);
                previous_pw = pw;

                *o = (2.0 * this_sample - 1.0) / (1.0 + VARIABLE_SAW_NOTCH_DEPTH);
            }
        }
        self.phase = phase;
        self.previous_pw = previous_pw;
        self.high = high;
        self.next_sample = next_sample;
    }
}
