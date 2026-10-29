// Rust port of the twist engine's swarm model (engine 8):
// dsp/engine/swarm_engine.h + .cc of the MIT eurorack macro-oscillator.
// Original notices:
//
// Copyright 2016 Emilie Gillet. (dsp/engine/swarm_engine.h, .cc)
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
use super::osc_base::{sine, FastSineOscillator, MAX_FREQUENCY};
use super::stm::*;

const NUM_SWARM_VOICES: usize = 8;

/// The voice ranks are spread around an integer-divided centre,
/// `(8 - 1) / 2 = 3`, so they run -1..+1.33 instead of -1..+1.
#[allow(dead_code)]
const QUIRK_SWARM_RANK_INTEGER_CENTRE: bool = true;

#[derive(Clone, Copy, Debug, Default)]
struct GrainEnvelope {
    from: f32,
    interval: f32,
    phase: f32,
    fm: f32,
    amplitude: f32,
    previous_size_ratio: f32,
    filter_coefficient: f32,
}

impl GrainEnvelope {
    fn init(&mut self) {
        self.from = 0.0;
        self.interval = 1.0;
        self.phase = 1.0;
        self.fm = 0.0;
        self.amplitude = 0.5;
        self.previous_size_ratio = 0.0;
        self.filter_coefficient = 0.0;
    }

    #[inline]
    fn step(&mut self, rate: f32, burst_mode: bool, start_burst: bool, rng: &mut Random) {
        let mut randomize = false;
        if start_burst {
            self.phase = 0.5;
            self.fm = 16.0;
            randomize = true;
        } else {
            self.phase += rate * self.fm;
            if self.phase >= 1.0 {
                self.phase -= (self.phase as i32) as f32;
                randomize = true;
            }
        }
        if randomize {
            self.from += self.interval;
            self.interval = rng.get_float() - self.from;
            // Randomize the duration of the grain.
            if burst_mode {
                self.fm *= 0.8 + 0.2 * rng.get_float();
            } else {
                self.fm = 0.5 + 1.5 * rng.get_float();
            }
        }
    }

    /// Two overlapping grains f1, f2 approximated by a tone ramping f1 -> f2.
    #[inline]
    fn frequency(&self, size_ratio: f32) -> f32 {
        if size_ratio < 1.0 {
            2.0 * (self.from + self.interval * self.phase) - 1.0
        } else {
            self.from
        }
    }

    #[inline]
    fn amplitude(&mut self, size_ratio: f32) -> f32 {
        let mut target_amplitude = 1.0;
        if size_ratio >= 1.0 {
            let phase = constrain((self.phase - 0.5) * size_ratio, -1.0, 1.0);
            let e = sine(0.5 * phase + 1.25);
            target_amplitude = 0.5 * (e + 1.0);
        }
        if (size_ratio >= 1.0) ^ (self.previous_size_ratio >= 1.0) {
            self.filter_coefficient = 0.5;
        }
        self.filter_coefficient *= 0.95;
        self.previous_size_ratio = size_ratio;
        one_pole(&mut self.amplitude, target_amplitude, 0.5 - self.filter_coefficient);
        self.amplitude
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct AdditiveSawOscillator {
    phase: f32,
    next_sample: f32,
    frequency: f32,
    gain: f32,
}

impl AdditiveSawOscillator {
    fn init(&mut self) {
        self.phase = 0.0;
        self.next_sample = 0.0;
        self.frequency = 0.01;
        self.gain = 0.0;
    }

    /// Adds into `out`.
    fn render(&mut self, mut frequency: f32, level: f32, out: &mut [f32]) {
        let size = out.len();
        if frequency >= MAX_FREQUENCY {
            frequency = MAX_FREQUENCY;
        }
        let mut next_sample = self.next_sample;
        let mut phase = self.phase;
        {
            let mut fm = ParameterInterpolator::new(&mut self.frequency, frequency, size);
            let mut gain = ParameterInterpolator::new(&mut self.gain, level, size);
            for o in out.iter_mut() {
                let mut this_sample = next_sample;
                next_sample = 0.0;
                let frequency = fm.next();
                phase += frequency;
                if phase >= 1.0 {
                    phase -= 1.0;
                    let t = phase / frequency;
                    this_sample -= this_blep_sample(t);
                    next_sample -= next_blep_sample(t);
                }
                next_sample += phase;
                *o += (2.0 * this_sample - 1.0) * gain.next();
            }
        }
        self.phase = phase;
        self.next_sample = next_sample;
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct SwarmVoice {
    rank: f32,
    envelope: GrainEnvelope,
    saw: AdditiveSawOscillator,
    sine: FastSineOscillator,
}

impl SwarmVoice {
    fn init(&mut self, rank: f32) {
        self.rank = rank;
        self.envelope.init();
        self.saw.init();
        self.sine.init();
    }

    #[allow(clippy::too_many_arguments)]
    fn render(
        &mut self,
        mut f0: f32,
        density: f32,
        burst_mode: bool,
        start_burst: bool,
        spread: f32,
        size_ratio: f32,
        saw: &mut [f32],
        sine: &mut [f32],
        rng: &mut Random,
    ) {
        self.envelope.step(density, burst_mode, start_burst, rng);
        let scale = 1.0 / NUM_SWARM_VOICES as f32;
        let amplitude = self.envelope.amplitude(size_ratio) * scale;

        let expo_amount = self.envelope.frequency(size_ratio);
        f0 *= semitones_to_ratio(48.0 * expo_amount * spread * self.rank);

        let linear_amount = self.rank * (self.rank + 0.01) * spread * 0.25;
        f0 *= 1.0 + linear_amount;

        self.saw.render(f0, amplitude, saw);
        self.sine.render_add(f0, amplitude, sine);
    }
}

/// Swarm of 8 sawtooths (OUT) and sines (AUX) with grain envelopes.
pub struct SwarmEngine {
    swarm_voice: [SwarmVoice; NUM_SWARM_VOICES],
}

impl SwarmEngine {
    pub fn new() -> Self {
        let mut e = SwarmEngine { swarm_voice: [SwarmVoice::default(); NUM_SWARM_VOICES] };
        e.reset();
        e
    }
}

impl Engine for SwarmEngine {
    fn reset(&mut self) {
        // QUIRK_SWARM_RANK_INTEGER_CENTRE.
        let n = ((NUM_SWARM_VOICES - 1) / 2) as f32;
        for (i, v) in self.swarm_voice.iter_mut().enumerate() {
            let rank = (i as f32 - n) / n;
            v.init(rank);
        }
    }

    fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32], _already_enveloped: &mut bool, rng: &mut Random) {
        let size = out.len();
        let f0 = note_to_frequency(p.note);
        let control_rate = size as f32;
        let density = note_to_frequency(p.timbre * 120.0) * 0.025 * control_rate;
        let spread = p.harmonics * p.harmonics * p.harmonics;
        let mut size_ratio = 0.25 * semitones_to_ratio((1.0 - p.morph) * 84.0);

        let burst_mode = (p.trigger & TRIGGER_UNPATCHED) == 0;
        let start_burst = (p.trigger & TRIGGER_RISING_EDGE) != 0;

        out.fill(0.0);
        aux.fill(0.0);

        for v in self.swarm_voice.iter_mut() {
            v.render(f0, density, burst_mode, start_burst, spread, size_ratio, out, aux, rng);
            size_ratio *= 0.97;
        }
    }
}
