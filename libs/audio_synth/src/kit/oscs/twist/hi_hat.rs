// Rust port of the twist engine's hi-hat model (engine 15):
// dsp/engine/hi_hat_engine.h + hi_hat_engine.cc and dsp/drums/hi_hat.h of the MIT
// eurorack macro-oscillator. Original notices:
//
// Copyright 2016 Emilie Gillet. (dsp/engine/hi_hat_engine.h, dsp/engine/hi_hat_engine.cc,
//                                dsp/drums/hi_hat.h)
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
use super::osc_base::{Oscillator, OSCILLATOR_SHAPE_SAW, OSCILLATOR_SHAPE_SQUARE};
use super::stm::*;

// ------------------------------------------------------- drums/hi_hat.h

/// A "metallic noise" source for the hi-hat.
pub trait MetallicNoise: Default {
    fn init(&mut self);
    fn render(&mut self, f0: f32, temp_1: &mut [f32], temp_2: &mut [f32], out: &mut [f32]);
}

/// Six square oscillators (the classic hi-hat's metallic noise).
#[derive(Clone, Copy, Debug, Default)]
pub struct SquareNoise {
    phase: [u32; 6],
}

impl MetallicNoise for SquareNoise {
    fn init(&mut self) {
        self.phase = [0; 6];
    }

    fn render(&mut self, f0: f32, _temp_1: &mut [f32], _temp_2: &mut [f32], out: &mut [f32]) {
        // Nominal f0: 414 Hz.
        const RATIOS: [f32; 6] = [1.0, 1.304, 1.466, 1.787, 1.932, 2.536];
        let mut increment = [0u32; 6];
        let mut phase = self.phase;
        for i in 0..6 {
            let mut f = f0 * RATIOS[i];
            if f >= 0.499 {
                f = 0.499;
            }
            increment[i] = (f * 4294967296.0) as u32;
        }
        for o in out.iter_mut() {
            let mut noise = 0u32;
            for i in 0..6 {
                phase[i] = phase[i].wrapping_add(increment[i]);
                noise += phase[i] >> 31;
            }
            *o = 0.33 * noise as f32 - 1.0;
        }
        self.phase = phase;
    }
}

/// Three ring-modulated square/saw pairs (a more metallic, FM-like noise).
#[derive(Clone, Copy, Debug, Default)]
pub struct RingModNoise {
    oscillator: [Oscillator; 6],
}

impl RingModNoise {
    fn render_pair(osc: &mut [Oscillator], f: [f32; 2], temp_1: &mut [f32], temp_2: &mut [f32], out: &mut [f32]) {
        osc[0].render::<OSCILLATOR_SHAPE_SQUARE>(f[0], 0.5, temp_1);
        osc[1].render::<OSCILLATOR_SHAPE_SAW>(f[1], 0.5, temp_2);
        for i in 0..out.len() {
            out[i] += temp_1[i] * temp_2[i];
        }
    }
}

impl MetallicNoise for RingModNoise {
    fn init(&mut self) {
        for o in self.oscillator.iter_mut() {
            o.init();
        }
    }

    fn render(&mut self, f0: f32, temp_1: &mut [f32], temp_2: &mut [f32], out: &mut [f32]) {
        let ratio = f0 / (0.01 + f0);
        let f1a = 200.0 / SAMPLE_RATE * ratio;
        let f1b = 7530.0 / SAMPLE_RATE * ratio;
        let f2a = 510.0 / SAMPLE_RATE * ratio;
        let f2b = 8075.0 / SAMPLE_RATE * ratio;
        let f3a = 730.0 / SAMPLE_RATE * ratio;
        let f3b = 10500.0 / SAMPLE_RATE * ratio;
        let f = [[f1a, f1b], [f2a, f2b], [f3a, f3b]];
        out.fill(0.0);
        for i in 0..3 {
            Self::render_pair(&mut self.oscillator[2 * i..2 * i + 2], f[i], temp_1, temp_2, out);
        }
    }
}

#[inline]
fn swing_vca(s: f32, gain: f32) -> f32 {
    let s = s * if s > 0.0 { 4.0 } else { 0.1 };
    let s = s / (1.0 + s.abs());
    (s + 0.1) * gain
}

#[inline]
fn linear_vca(s: f32, gain: f32) -> f32 {
    s * gain
}

/// Analog-style hi-hat; `SWING_VCA` picks the swing VCA over the linear
/// one, `RESONANCE` the resonant colouration filter, `TWO_STAGE` the
/// two-stage envelope.
#[derive(Clone, Copy, Debug, Default)]
pub struct HiHat<N: MetallicNoise, const SWING_VCA: bool, const RESONANCE: bool, const TWO_STAGE: bool> {
    envelope: f32,
    noise_clock: f32,
    noise_sample: f32,
    sustain_gain: f32,
    metallic_noise: N,
    noise_coloration_svf: Svf,
    hpf: Svf,
}

impl<N: MetallicNoise, const SWING_VCA: bool, const RESONANCE: bool, const TWO_STAGE: bool>
    HiHat<N, SWING_VCA, RESONANCE, TWO_STAGE>
{
    pub fn init(&mut self) {
        self.envelope = 0.0;
        self.noise_clock = 0.0;
        self.noise_sample = 0.0;
        self.sustain_gain = 0.0;
        self.metallic_noise.init();
        self.noise_coloration_svf.init();
        self.hpf.init();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        sustain: bool,
        trigger: bool,
        accent: f32,
        f0: f32,
        tone: f32,
        decay: f32,
        mut noisiness: f32,
        temp_1: &mut [f32],
        temp_2: &mut [f32],
        out: &mut [f32],
        rng: &mut Random,
    ) {
        let size = out.len();
        let envelope_decay = 1.0 - 0.003 * semitones_to_ratio(-decay * 84.0);
        let cut_decay = 1.0 - 0.0025 * semitones_to_ratio(-decay * 36.0);

        if trigger {
            self.envelope = (1.5 + 0.5 * (1.0 - decay)) * (0.3 + 0.7 * accent);
        }

        // Render the metallic noise.
        self.metallic_noise.render(2.0 * f0, temp_1, temp_2, out);

        // Apply BPF on the metallic noise.
        let cutoff = constrain(150.0 / SAMPLE_RATE * semitones_to_ratio(tone * 72.0), 0.0, 16000.0 / SAMPLE_RATE);
        self.noise_coloration_svf
            .set_f_q::<FREQUENCY_ACCURATE>(cutoff, if RESONANCE { 3.0 + 3.0 * tone } else { 1.0 });
        self.noise_coloration_svf.process_in_place::<FILTER_MODE_BAND_PASS>(out);

        // Not part of the original circuit: a variable amount of clocked
        // noise added to the oscillators' output.
        noisiness *= noisiness;
        let noise_f = constrain(f0 * (16.0 + 16.0 * (1.0 - noisiness)), 0.0, 0.5);
        for o in out.iter_mut() {
            self.noise_clock += noise_f;
            if self.noise_clock >= 1.0 {
                self.noise_clock -= 1.0;
                self.noise_sample = rng.get_float() - 0.5;
            }
            *o += noisiness * (self.noise_sample - *o);
        }

        // Apply VCA.
        {
            let mut sustain_gain = ParameterInterpolator::new(&mut self.sustain_gain, accent * decay, size);
            for o in out.iter_mut() {
                self.envelope *= if self.envelope > 0.5 || !TWO_STAGE { envelope_decay } else { cut_decay };
                let g = if sustain { sustain_gain.next() } else { self.envelope };
                *o = if SWING_VCA { swing_vca(*o, g) } else { linear_vca(*o, g) };
            }
        }

        self.hpf.set_f_q::<FREQUENCY_ACCURATE>(cutoff, 0.5);
        self.hpf.process_in_place::<FILTER_MODE_HIGH_PASS>(out);
    }
}

// ------------------------------------------------ engine/hi_hat_engine.*

/// Hi-hat with two noise sources: one faithful to the circuit, the other
/// more metallic (main / aux).
pub struct HiHatEngine {
    hi_hat_1: HiHat<SquareNoise, true, true, false>,
    hi_hat_2: HiHat<RingModNoise, false, false, true>,
    temp_buffer: [f32; MAX_BLOCK_SIZE * 2],
}

impl HiHatEngine {
    pub fn new() -> Self {
        let mut e = HiHatEngine { hi_hat_1: HiHat::default(), hi_hat_2: HiHat::default(), temp_buffer: [0.0; MAX_BLOCK_SIZE * 2] };
        e.hi_hat_1.init();
        e.hi_hat_2.init();
        e
    }
}

impl Engine for HiHatEngine {
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
        let sustain = p.trigger & TRIGGER_UNPATCHED != 0;
        let rising = p.trigger & TRIGGER_RISING_EDGE != 0;
        let (t1, rest) = self.temp_buffer.split_at_mut(size);
        let t2 = &mut rest[..size];
        self.hi_hat_1.render(sustain, rising, p.accent, f0, p.timbre, p.morph, p.harmonics, t1, t2, out, rng);
        self.hi_hat_2.render(sustain, rising, p.accent, f0, p.timbre, p.morph, p.harmonics, t1, t2, aux, rng);
    }
}
