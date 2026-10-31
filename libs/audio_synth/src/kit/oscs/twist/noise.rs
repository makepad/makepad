// Rust port of the twist engine's filtered noise model (engine 9):
// dsp/engine/noise_engine.h + noise_engine.cc and dsp/noise/clocked_noise.h of
// the MIT eurorack macro-oscillator. Original notices:
//
// Copyright 2016 Emilie Gillet. (dsp/engine/noise_engine.h, dsp/engine/noise_engine.cc,
//                                dsp/noise/clocked_noise.h)
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
use super::stm::*;

// ---------------------------------------------------- noise/clocked_noise.h

/// Noise through a sample and hold running at a target frequency.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClockedNoise {
    phase: f32,
    sample: f32,
    next_sample: f32,
    frequency: f32,
}

impl ClockedNoise {
    pub fn init(&mut self) {
        self.phase = 0.0;
        self.sample = 0.0;
        self.next_sample = 0.0;
        self.frequency = 0.001;
    }

    pub fn render(&mut self, sync: bool, frequency: f32, out: &mut [f32], rng: &mut Random) {
        let frequency = constrain(frequency, 0.0, 1.0);
        let size = out.len();
        let mut next_sample = self.next_sample;
        let mut sample = self.sample;
        if sync {
            self.phase = 1.0;
        }
        let mut phase = self.phase;
        {
            let mut fm = ParameterInterpolator::new(&mut self.frequency, frequency, size);
            for o in out.iter_mut() {
                let mut this_sample = next_sample;
                next_sample = 0.0;
                let frequency = fm.next();
                let raw_sample = rng.get_float() * 2.0 - 1.0;
                let raw_amount = constrain(4.0 * (frequency - 0.25), 0.0, 1.0);
                phase += frequency;
                if phase >= 1.0 {
                    phase -= 1.0;
                    let t = phase / frequency;
                    let new_sample = raw_sample;
                    let discontinuity = new_sample - sample;
                    this_sample += discontinuity * this_blep_sample(t);
                    next_sample += discontinuity * next_blep_sample(t);
                    sample = new_sample;
                }
                next_sample += sample;
                *o = this_sample + raw_amount * (raw_sample - this_sample);
            }
        }
        self.phase = phase;
        self.next_sample = next_sample;
        self.sample = sample;
    }
}

// ------------------------------------------------- engine/noise_engine.*

/// Clocked noise processed by a multimode filter.
pub struct NoiseEngine {
    clocked_noise: [ClockedNoise; 2],
    lp_hp_filter: Svf,
    bp_filter: [Svf; 2],
    previous_f0: f32,
    previous_f1: f32,
    previous_q: f32,
    previous_mode: f32,
    temp_buffer: [f32; MAX_BLOCK_SIZE],
}

impl NoiseEngine {
    pub fn new() -> Self {
        let mut e = NoiseEngine {
            clocked_noise: [ClockedNoise::default(); 2],
            lp_hp_filter: Svf::default(),
            bp_filter: [Svf::default(); 2],
            previous_f0: 0.0,
            previous_f1: 0.0,
            previous_q: 0.0,
            previous_mode: 0.0,
            temp_buffer: [0.0; MAX_BLOCK_SIZE],
        };
        e.clocked_noise[0].init();
        e.clocked_noise[1].init();
        e.lp_hp_filter.init();
        e.bp_filter[0].init();
        e.bp_filter[1].init();
        e
    }
}

impl Engine for NoiseEngine {
    fn reset(&mut self) {}

    fn render(
        &mut self,
        p: &EngineParameters,
        out: &mut [f32],
        aux: &mut [f32],
        _already_enveloped: &mut bool,
        rng: &mut Random,
    ) {
        let size = out.len();
        let f0 = note_to_frequency(p.note);
        let f1 = note_to_frequency(p.note + p.harmonics * 48.0 - 24.0);
        let clock_lowest_note = if p.trigger & TRIGGER_UNPATCHED != 0 { 0.0 } else { -24.0 };
        let clock_f = note_to_frequency(p.timbre * (128.0 - clock_lowest_note) + clock_lowest_note);
        let q = 0.5 * semitones_to_ratio(p.morph * 120.0);
        let sync = p.trigger & TRIGGER_RISING_EDGE != 0;
        let temp = &mut self.temp_buffer[..size];
        self.clocked_noise[0].render(sync, clock_f, aux, rng);
        self.clocked_noise[1].render(sync, clock_f * f1 / f0, temp, rng);

        let mut f0_modulation = ParameterInterpolator::new(&mut self.previous_f0, f0, size);
        let mut f1_modulation = ParameterInterpolator::new(&mut self.previous_f1, f1, size);
        let mut q_modulation = ParameterInterpolator::new(&mut self.previous_q, q, size);
        let mut mode_modulation = ParameterInterpolator::new(&mut self.previous_mode, p.harmonics, size);
        for i in 0..size {
            let f0 = f0_modulation.next();
            let f1 = f1_modulation.next();
            let q = q_modulation.next();
            let gain = 1.0 / ((0.5 + q) * 40.0 * f0).sqrt();
            self.lp_hp_filter.set_f_q::<FREQUENCY_ACCURATE>(f0, q);
            self.bp_filter[0].set_f_q::<FREQUENCY_ACCURATE>(f0, q);
            self.bp_filter[1].set_f_q::<FREQUENCY_ACCURATE>(f1, q);
            let input_1 = aux[i] * gain;
            let input_2 = temp[i] * gain;
            let mut o = [0.0f32];
            self.lp_hp_filter.process_multimode_lp_to_hp(&[input_1], &mut o, mode_modulation.next());
            out[i] = o[0];
            aux[i] = self.bp_filter[0].process::<FILTER_MODE_BAND_PASS>(input_1)
                + self.bp_filter[1].process::<FILTER_MODE_BAND_PASS>(input_2);
        }
    }
}
