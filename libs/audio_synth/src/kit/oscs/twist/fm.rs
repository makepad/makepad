// Rust port of the twist engine's 2-operator FM model (engine 2):
// dsp/engine/fm_engine.h + .cc and dsp/downsampler/4x_downsampler.h of the MIT
// eurorack macro-oscillator. Original notices:
//
// Copyright 2016 Emilie Gillet. (dsp/engine/fm_engine.h, .cc)
// Copyright 2021 Emilie Gillet. (dsp/downsampler/4x_downsampler.h)
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
use super::osc_base::sine_pm;
use super::stm::*;
use super::tables::tables;

// ------------------------------------------------- 4x_downsampler.h

pub const OVERSAMPLING: usize = 4;

/// 4x FIR decimator whose head lives in a caller-owned state between blocks.
pub struct Downsampler<'a> {
    head: f32,
    tail: f32,
    state: &'a mut f32,
    fir: &'a [f32; 4],
}

impl<'a> Downsampler<'a> {
    #[inline]
    pub fn new(state: &'a mut f32) -> Self {
        Downsampler { head: *state, tail: 0.0, state, fir: &tables().downsampler_fir }
    }
    #[inline]
    pub fn accumulate(&mut self, i: usize, sample: f32) {
        self.head += sample * self.fir[3 - (i & 3)];
        self.tail += sample * self.fir[i & 3];
    }
    #[inline]
    pub fn read(&mut self) -> f32 {
        let value = self.head;
        self.head = self.tail;
        self.tail = 0.0;
        value
    }
}

impl Drop for Downsampler<'_> {
    fn drop(&mut self) {
        *self.state = self.head;
    }
}

// ----------------------------------------------------------- fm_engine

/// Classic 2-operator FM (with feedback and a sub-octave), 4x oversampled.
pub struct FmEngine {
    carrier_phase: u32,
    modulator_phase: u32,
    sub_phase: u32,
    previous_carrier_frequency: f32,
    previous_modulator_frequency: f32,
    previous_amount: f32,
    previous_feedback: f32,
    previous_sample: f32,
    sub_fir: f32,
    carrier_fir: f32,
}

impl FmEngine {
    pub fn new() -> Self {
        // The original leaves the two FIR states uninitialised; zero here.
        FmEngine {
            carrier_phase: 0,
            modulator_phase: 0,
            sub_phase: 0,
            previous_carrier_frequency: A0,
            previous_modulator_frequency: A0,
            previous_amount: 0.0,
            previous_feedback: 0.0,
            previous_sample: 0.0,
            sub_fir: 0.0,
            carrier_fir: 0.0,
        }
    }
}

impl Engine for FmEngine {
    fn reset(&mut self) {}

    fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32], _already_enveloped: &mut bool, _rng: &mut Random) {
        let size = out.len();
        // 4x oversampling.
        let note = p.note - 24.0;
        let ratio = interpolate(&tables().fm_frequency_quantizer, p.harmonics, 128.0);

        let modulator_note = note + ratio;
        let target_modulator_frequency = constrain(note_to_frequency(modulator_note), 0.0, 0.5);

        // Reduce the maximum FM index for high notes, against aliasing.
        let mut hf_taming = constrain(1.0 - (modulator_note - 72.0) * 0.025, 0.0, 1.0);
        hf_taming *= hf_taming;

        let mut carrier_frequency =
            ParameterInterpolator::new(&mut self.previous_carrier_frequency, note_to_frequency(note), size);
        let mut modulator_frequency =
            ParameterInterpolator::new(&mut self.previous_modulator_frequency, target_modulator_frequency, size);
        let mut amount_modulation =
            ParameterInterpolator::new(&mut self.previous_amount, 2.0 * p.timbre * p.timbre * hf_taming, size);
        let mut feedback_modulation = ParameterInterpolator::new(&mut self.previous_feedback, 2.0 * p.morph - 1.0, size);

        let mut carrier_downsampler = Downsampler::new(&mut self.carrier_fir);
        let mut sub_downsampler = Downsampler::new(&mut self.sub_fir);

        let mut previous_sample = self.previous_sample;
        let mut modulator_phase = self.modulator_phase;
        let mut carrier_phase = self.carrier_phase;
        let mut sub_phase = self.sub_phase;
        for i in 0..size {
            let max_uint32 = 4294967296.0f32;
            let amount = amount_modulation.next();
            let feedback = feedback_modulation.next();
            let phase_feedback = if feedback < 0.0 { 0.5 * feedback * feedback } else { 0.0 };
            let carrier_increment = (max_uint32 * carrier_frequency.next()) as u32;
            let mf = modulator_frequency.next();

            for j in 0..OVERSAMPLING {
                modulator_phase =
                    modulator_phase.wrapping_add((max_uint32 * mf * (1.0 + previous_sample * phase_feedback)) as u32);
                carrier_phase = carrier_phase.wrapping_add(carrier_increment);
                sub_phase = sub_phase.wrapping_add(carrier_increment >> 1);
                let modulator_fb = if feedback > 0.0 { 0.25 * feedback * feedback } else { 0.0 };
                let modulator = sine_pm(modulator_phase, modulator_fb * previous_sample);
                let carrier = sine_pm(carrier_phase, amount * modulator);
                let sub = sine_pm(sub_phase, amount * carrier * 0.25);
                one_pole(&mut previous_sample, carrier, 0.05);
                carrier_downsampler.accumulate(j, carrier);
                sub_downsampler.accumulate(j, sub);
            }
            out[i] = carrier_downsampler.read();
            aux[i] = sub_downsampler.read();
        }
        self.previous_sample = previous_sample;
        self.modulator_phase = modulator_phase;
        self.carrier_phase = carrier_phase;
        self.sub_phase = sub_phase;
    }
}
