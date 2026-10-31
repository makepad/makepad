// Rust port of the twist engine's variable-shape oscillator:
// dsp/oscillator/variable_shape_oscillator.h of the MIT eurorack macro-oscillator.
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

/// Triangle > saw > square with variable slope / pulse width, optionally
/// hard-synced to a master frequency, optionally outputting a phase.
#[derive(Clone, Copy, Debug, Default)]
pub struct VariableShapeOscillator {
    master_phase: f32,
    slave_phase: f32,
    next_sample: f32,
    previous_pw: f32,
    high: bool,
    master_frequency: f32,
    slave_frequency: f32,
    pw: f32,
    waveshape: f32,
    phase_modulation: f32,
}

impl VariableShapeOscillator {
    pub fn init(&mut self) {
        self.master_phase = 0.0;
        self.slave_phase = 0.0;
        self.next_sample = 0.0;
        self.previous_pw = 0.5;
        self.high = false;
        self.master_frequency = 0.0;
        self.slave_frequency = 0.01;
        self.pw = 0.5;
        self.waveshape = 0.0;
        self.phase_modulation = 0.0;
    }

    /// Free running.
    #[allow(dead_code)]
    #[inline]
    pub fn render(&mut self, frequency: f32, pw: f32, waveshape: f32, out: &mut [f32]) {
        self.render_full::<false, false>(0.0, frequency, pw, waveshape, 0.0, out);
    }

    /// Hard-synced to `master_frequency`.
    #[inline]
    pub fn render_sync(&mut self, master_frequency: f32, frequency: f32, pw: f32, waveshape: f32, out: &mut [f32]) {
        self.render_full::<true, false>(master_frequency, frequency, pw, waveshape, 0.0, out);
    }

    #[inline]
    fn compute_naive_sample(phase: f32, pw: f32, slope_up: f32, slope_down: f32, triangle_amount: f32, square_amount: f32) -> f32 {
        let mut saw = phase;
        let square = if phase < pw { 0.0 } else { 1.0 };
        let triangle = if phase < pw { phase * slope_up } else { 1.0 - (phase - pw) * slope_down };
        saw += (square - saw) * square_amount;
        saw += (triangle - saw) * triangle_amount;
        saw
    }

    pub fn render_full<const SYNC: bool, const OUTPUT_PHASE: bool>(
        &mut self,
        mut master_frequency: f32,
        mut frequency: f32,
        mut pw: f32,
        waveshape: f32,
        phase_modulation_amount: f32,
        out: &mut [f32],
    ) {
        let size = out.len();
        if master_frequency >= MAX_FREQUENCY {
            master_frequency = MAX_FREQUENCY;
        }
        if frequency >= MAX_FREQUENCY {
            frequency = MAX_FREQUENCY;
        }
        if frequency >= 0.25 {
            pw = 0.5;
        } else {
            pw = constrain(pw, frequency * 2.0, 1.0 - 2.0 * frequency);
        }

        let mut master_phase = self.master_phase;
        let mut slave_phase = self.slave_phase;
        let mut previous_pw = self.previous_pw;
        let mut high = self.high;
        let mut next_sample = self.next_sample;
        {
            let mut master_fm = ParameterInterpolator::new(&mut self.master_frequency, master_frequency, size);
            let mut fm = ParameterInterpolator::new(&mut self.slave_frequency, frequency, size);
            let mut pwm = ParameterInterpolator::new(&mut self.pw, pw, size);
            let mut waveshape_modulation = ParameterInterpolator::new(&mut self.waveshape, waveshape, size);
            let mut phase_modulation =
                ParameterInterpolator::new(&mut self.phase_modulation, phase_modulation_amount, size);

            for o in out.iter_mut() {
                let mut reset = false;
                let mut transition_during_reset = false;
                let mut reset_time = 0.0;

                let mut this_sample = next_sample;
                next_sample = 0.0;

                let master_frequency = master_fm.next();
                let slave_frequency = fm.next();
                let pw = pwm.next();
                let waveshape = waveshape_modulation.next();

                let square_amount = (waveshape - 0.5).max(0.0) * 2.0;
                let triangle_amount = (1.0 - waveshape * 2.0).max(0.0);

                let slope_up = 1.0 / pw;
                let slope_down = 1.0 / (1.0 - pw);

                if SYNC {
                    master_phase += master_frequency;
                    if master_phase >= 1.0 {
                        master_phase -= 1.0;
                        reset_time = master_phase / master_frequency;

                        let mut slave_phase_at_reset = slave_phase + (1.0 - reset_time) * slave_frequency;
                        reset = true;
                        if slave_phase_at_reset >= 1.0 {
                            slave_phase_at_reset -= 1.0;
                            transition_during_reset = true;
                        }
                        if !high && slave_phase_at_reset >= pw {
                            transition_during_reset = true;
                        }
                        let value = Self::compute_naive_sample(
                            slave_phase_at_reset,
                            pw,
                            slope_up,
                            slope_down,
                            triangle_amount,
                            square_amount,
                        );
                        this_sample -= value * this_blep_sample(reset_time);
                        next_sample -= value * next_blep_sample(reset_time);
                    }
                } else if OUTPUT_PHASE {
                    master_phase += master_frequency;
                    if master_phase >= 1.0 {
                        master_phase -= 1.0;
                    }
                }

                slave_phase += slave_frequency;
                while transition_during_reset || !reset {
                    if !high {
                        if slave_phase < pw {
                            break;
                        }
                        let t = (slave_phase - pw) / (previous_pw - pw + slave_frequency);
                        let mut triangle_step = (slope_up + slope_down) * slave_frequency;
                        triangle_step *= triangle_amount;

                        this_sample += square_amount * this_blep_sample(t);
                        next_sample += square_amount * next_blep_sample(t);
                        this_sample -= triangle_step * this_integrated_blep_sample(t);
                        next_sample -= triangle_step * next_integrated_blep_sample(t);
                        high = true;
                    }

                    if high {
                        if slave_phase < 1.0 {
                            break;
                        }
                        slave_phase -= 1.0;
                        let t = slave_phase / slave_frequency;
                        let mut triangle_step = (slope_up + slope_down) * slave_frequency;
                        triangle_step *= triangle_amount;

                        this_sample -= (1.0 - triangle_amount) * this_blep_sample(t);
                        next_sample -= (1.0 - triangle_amount) * next_blep_sample(t);
                        this_sample += triangle_step * this_integrated_blep_sample(t);
                        next_sample += triangle_step * next_integrated_blep_sample(t);
                        high = false;
                    }
                }

                if SYNC && reset {
                    slave_phase = reset_time * slave_frequency;
                    high = false;
                }

                next_sample += Self::compute_naive_sample(slave_phase, pw, slope_up, slope_down, triangle_amount, square_amount);
                previous_pw = pw;

                if OUTPUT_PHASE {
                    let mut phasor = master_phase;
                    if SYNC {
                        // A trick to prevent discontinuities when the phase wraps around.
                        let w = 4.0 * (1.0 - master_phase) * master_phase;
                        this_sample *= w * (2.0 - w);
                        // Some asymmetry on the main phasor too.
                        let p2 = phasor * phasor;
                        phasor += (p2 * p2 - phasor) * (pw - 0.5).abs() * 2.0;
                    }
                    *o = phasor + phase_modulation.next() * this_sample;
                } else {
                    *o = 2.0 * this_sample - 1.0;
                }
            }
        }
        self.master_phase = master_phase;
        self.slave_phase = slave_phase;
        self.previous_pw = previous_pw;
        self.high = high;
        self.next_sample = next_sample;
    }

    #[inline]
    pub fn set_master_phase(&mut self, master_phase: f32) {
        self.master_phase = master_phase;
    }
}
