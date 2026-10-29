// Rust port of the twist engine's modal resonator model (engine 12):
// dsp/engine/modal_engine.h + modal_engine.cc, dsp/physical_modelling/modal_voice.h
// + modal_voice.cc and dsp/physical_modelling/resonator.h + resonator.cc of the
// MIT eurorack macro-oscillator. Original notices:
//
// Copyright 2016 Emilie Gillet. (dsp/engine/modal_engine.h, dsp/engine/modal_engine.cc,
//                                dsp/physical_modelling/modal_voice.h, modal_voice.cc,
//                                dsp/physical_modelling/resonator.h, resonator.cc)
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
use super::noise_dust::dust;
use super::stm::*;
use super::tables::tables;

// ---------------------------------------- physical_modelling/resonator.*

const MAX_NUM_MODES: usize = 24;
const MODE_BATCH_SIZE: usize = 4;

/// `B` SVF band/low-pass filters run side by side and summed.
#[derive(Clone, Copy, Debug)]
pub struct ResonatorSvf<const B: usize> {
    state_1: [f32; B],
    state_2: [f32; B],
}

impl<const B: usize> Default for ResonatorSvf<B> {
    fn default() -> Self {
        ResonatorSvf { state_1: [0.0; B], state_2: [0.0; B] }
    }
}

impl<const B: usize> ResonatorSvf<B> {
    pub fn init(&mut self) {
        self.state_1 = [0.0; B];
        self.state_2 = [0.0; B];
    }

    /// `out` (+)= the sum of gain[i] * filter_i(in).
    pub fn process<const M: u8, const ADD: bool>(
        &mut self,
        f: &[f32; B],
        q: &[f32; B],
        gain: &[f32; B],
        input: &[f32],
        out: &mut [f32],
    ) {
        let mut g = [0.0f32; B];
        let mut r_plus_g = [0.0f32; B];
        let mut h = [0.0f32; B];
        let mut state_1 = self.state_1;
        let mut state_2 = self.state_2;
        for i in 0..B {
            g[i] = tan_approx::<FREQUENCY_FAST>(f[i]);
            let r = 1.0 / q[i];
            h[i] = 1.0 / (1.0 + r * g[i] + g[i] * g[i]);
            r_plus_g[i] = r + g[i];
        }
        for n in 0..out.len() {
            let s_in = input[n];
            let mut s_out = 0.0f32;
            for i in 0..B {
                let hp = (s_in - r_plus_g[i] * state_1[i] - state_2[i]) * h[i];
                let bp = g[i] * hp + state_1[i];
                state_1[i] = g[i] * hp + bp;
                let lp = g[i] * bp + state_2[i];
                state_2[i] = g[i] * bp + lp;
                s_out += gain[i] * if M == FILTER_MODE_LOW_PASS { lp } else { bp };
            }
            if ADD {
                out[n] += s_out;
            } else {
                out[n] = s_out;
            }
        }
        self.state_1 = state_1;
        self.state_2 = state_2;
    }

    /// In-place variant (the original passes the same buffer as in and out).
    pub fn process_in_place<const M: u8>(&mut self, f: &[f32; B], q: &[f32; B], gain: &[f32; B], in_out: &mut [f32]) {
        let mut tmp = [0.0f32; MAX_BLOCK_SIZE];
        let n = in_out.len();
        tmp[..n].copy_from_slice(in_out);
        self.process::<M, false>(f, q, gain, &tmp[..n], in_out);
    }
}

#[inline]
fn nth_harmonic_compensation(n: i32, mut stiffness: f32) -> f32 {
    let mut stretch_factor = 1.0;
    for _ in 0..n - 1 {
        stretch_factor += stiffness;
        if stiffness < 0.0 {
            stiffness *= 0.93;
        } else {
            stiffness *= 0.98;
        }
    }
    1.0 / stretch_factor
}

/// Modal resonator with a fixed excitation position.
pub struct Resonator {
    resolution: usize,
    mode_amplitude: [f32; MAX_NUM_MODES],
    mode_filters: [ResonatorSvf<MODE_BATCH_SIZE>; MAX_NUM_MODES / MODE_BATCH_SIZE],
}

impl Resonator {
    pub fn new() -> Resonator {
        Resonator {
            resolution: MAX_NUM_MODES,
            mode_amplitude: [0.0; MAX_NUM_MODES],
            mode_filters: [ResonatorSvf::default(); MAX_NUM_MODES / MODE_BATCH_SIZE],
        }
    }

    pub fn init(&mut self, position: f32, resolution: usize) {
        self.resolution = resolution.min(MAX_NUM_MODES);
        let mut amplitudes = CosineOscillator::default();
        amplitudes.init::<COSINE_OSCILLATOR_APPROXIMATE>(position);
        for i in 0..resolution {
            self.mode_amplitude[i] = amplitudes.next() * 0.25;
        }
        for f in self.mode_filters.iter_mut() {
            f.init();
        }
    }

    pub fn process(&mut self, mut f0: f32, structure: f32, mut brightness: f32, damping: f32, input: &[f32], out: &mut [f32]) {
        let mut stiffness = interpolate(&tables().stiffness, structure, 64.0);
        f0 *= nth_harmonic_compensation(3, stiffness);

        let mut harmonic = f0;
        let mut stretch_factor = 1.0;
        let q_sqrt = semitones_to_ratio(damping * 79.7);
        let mut q = 500.0 * q_sqrt * q_sqrt;
        brightness *= 1.0 - structure * 0.3;
        brightness *= 1.0 - damping * 0.3;
        let q_loss = brightness * (2.0 - brightness) * 0.85 + 0.15;

        let mut mode_q = [0.0f32; MODE_BATCH_SIZE];
        let mut mode_f = [0.0f32; MODE_BATCH_SIZE];
        let mut mode_a = [0.0f32; MODE_BATCH_SIZE];
        let mut batch_counter = 0;
        let mut batch = 0;
        for i in 0..self.resolution {
            let mut mode_frequency = harmonic * stretch_factor;
            if mode_frequency >= 0.499 {
                mode_frequency = 0.499;
            }
            let mode_attenuation = 1.0 - mode_frequency * 2.0;
            mode_f[batch_counter] = mode_frequency;
            mode_q[batch_counter] = 1.0 + mode_frequency * q;
            mode_a[batch_counter] = self.mode_amplitude[i] * mode_attenuation;
            batch_counter += 1;
            if batch_counter == MODE_BATCH_SIZE {
                batch_counter = 0;
                self.mode_filters[batch].process::<FILTER_MODE_BAND_PASS, true>(&mode_f, &mode_q, &mode_a, input, out);
                batch += 1;
            }
            stretch_factor += stiffness;
            if stiffness < 0.0 {
                // Keep the partials from folding back into negative frequencies.
                stiffness *= 0.93;
            } else {
                // A few extra partials in the highest frequencies.
                stiffness *= 0.98;
            }
            harmonic += f0;
            q *= q_loss;
        }
    }
}

// --------------------------------------- physical_modelling/modal_voice.*

/// Modal voice with a mallet exciter: click -> LPF -> resonator (dust when
/// free running).
pub struct ModalVoice {
    excitation_filter: ResonatorSvf<1>,
    resonator: Resonator,
}

impl ModalVoice {
    pub fn new() -> ModalVoice {
        let mut v = ModalVoice { excitation_filter: ResonatorSvf::default(), resonator: Resonator::new() };
        v.init();
        v
    }

    pub fn init(&mut self) {
        self.excitation_filter.init();
        self.resonator.init(0.015, MAX_NUM_MODES);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        sustain: bool,
        trigger: bool,
        accent: f32,
        f0: f32,
        structure: f32,
        mut brightness: f32,
        mut damping: f32,
        temp: &mut [f32],
        out: &mut [f32],
        aux: &mut [f32],
        rng: &mut Random,
    ) {
        let density = brightness * brightness;

        brightness += 0.25 * accent * (1.0 - brightness);
        damping += 0.25 * accent * (1.0 - damping);

        let range = if sustain { 36.0 } else { 60.0 };
        let f = if sustain { 4.0 * f0 } else { 2.0 * f0 };
        let cutoff = (f * semitones_to_ratio((brightness * (2.0 - brightness) - 0.5) * range)).min(0.499);
        let q = if sustain { 0.7 } else { 1.5 };

        // Synthesize excitation signal.
        if sustain {
            let dust_f = 0.00005 + 0.99995 * density * density;
            for t in temp.iter_mut() {
                *t = dust(dust_f, rng) * (4.0 - dust_f * 3.0) * accent;
            }
        } else {
            temp.fill(0.0);
            if trigger {
                let attenuation = 1.0 - damping * 0.5;
                let amplitude = (0.12 + 0.08 * accent) * attenuation;
                temp[0] = amplitude * semitones_to_ratio(cutoff * cutoff * 24.0) / cutoff;
            }
        }
        self.excitation_filter.process_in_place::<FILTER_MODE_LOW_PASS>(&[cutoff], &[q], &[1.0], temp);
        for i in 0..out.len() {
            aux[i] += temp[i];
        }

        self.resonator.process(f0, structure, brightness, damping, temp, out);
    }
}

// ------------------------------------------------- engine/modal_engine.*

/// One voice of modal synthesis.
pub struct ModalEngine {
    voice: ModalVoice,
    temp_buffer: [f32; MAX_BLOCK_SIZE],
    harmonics_lp: f32,
}

impl ModalEngine {
    pub fn new() -> Self {
        let mut e = ModalEngine { voice: ModalVoice::new(), temp_buffer: [0.0; MAX_BLOCK_SIZE], harmonics_lp: 0.0 };
        e.reset();
        e
    }
}

impl Engine for ModalEngine {
    fn reset(&mut self) {
        self.voice.init();
    }

    fn render(
        &mut self,
        p: &EngineParameters,
        out: &mut [f32],
        aux: &mut [f32],
        _already_enveloped: &mut bool,
        rng: &mut Random,
    ) {
        let size = out.len();
        out.fill(0.0);
        aux.fill(0.0);
        one_pole(&mut self.harmonics_lp, p.harmonics, 0.01);
        self.voice.render(
            p.trigger & TRIGGER_UNPATCHED != 0,
            p.trigger & TRIGGER_RISING_EDGE != 0,
            p.accent,
            note_to_frequency(p.note),
            self.harmonics_lp,
            p.timbre,
            p.morph,
            &mut self.temp_buffer[..size],
            out,
            aux,
            rng,
        );
    }
}
