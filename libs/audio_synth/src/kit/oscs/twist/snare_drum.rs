// Rust port of the twist engine's snare model (engine 14):
// dsp/engine/snare_drum_engine.h + snare_drum_engine.cc, dsp/drums/analog_snare_drum.h
// and dsp/drums/synthetic_snare_drum.h of the MIT eurorack macro-oscillator.
// Original notices:
//
// Copyright 2016 Emilie Gillet. (dsp/engine/snare_drum_engine.h, dsp/engine/snare_drum_engine.cc,
//                                dsp/drums/analog_snare_drum.h, dsp/drums/synthetic_snare_drum.h)
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
use super::osc_base::SineOscillator;
use super::stm::*;

// -------------------------------------------- drums/analog_snare_drum.h

const NUM_MODES: usize = 5;
const MODE_FREQUENCIES: [f32; NUM_MODES] = [1.00, 2.00, 3.18, 4.16, 5.62];

/// Analog snare drum model (resonant shell modes + filtered noise).
#[derive(Clone, Copy, Debug, Default)]
pub struct AnalogSnareDrum {
    pulse_remaining_samples: i32,
    pulse: f32,
    pulse_height: f32,
    pulse_lp: f32,
    noise_envelope: f32,
    sustain_gain: f32,
    resonator: [Svf; NUM_MODES],
    noise_filter: Svf,
    // Replace the resonators in free-running (sustain) mode.
    oscillator: [SineOscillator; NUM_MODES],
}

impl AnalogSnareDrum {
    pub fn init(&mut self) {
        self.pulse_remaining_samples = 0;
        self.pulse = 0.0;
        self.pulse_height = 0.0;
        self.pulse_lp = 0.0;
        self.noise_envelope = 0.0;
        self.sustain_gain = 0.0;
        for i in 0..NUM_MODES {
            self.resonator[i].init();
            self.oscillator[i].init();
        }
        self.noise_filter.init();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        sustain: bool,
        trigger: bool,
        accent: f32,
        f0: f32,
        mut tone: f32,
        decay: f32,
        mut snappy: f32,
        out: &mut [f32],
        rng: &mut Random,
    ) {
        let decay_xt = decay * (1.0 + decay * (decay - 1.0));
        let trigger_pulse_duration = (1.0e-3 * SAMPLE_RATE) as i32;
        let pulse_decay_time = 0.1e-3 * SAMPLE_RATE;
        let q = 2000.0 * semitones_to_ratio(decay_xt * 84.0);
        let noise_envelope_decay = 1.0 - 0.0017 * semitones_to_ratio(-decay * (50.0 + snappy * 10.0));
        let exciter_leak = snappy * (2.0 - snappy) * 0.1;

        snappy = constrain(snappy * 1.1 - 0.05, 0.0, 1.0);

        if trigger {
            self.pulse_remaining_samples = trigger_pulse_duration;
            self.pulse_height = 3.0 + 7.0 * accent;
            self.noise_envelope = 2.0;
        }

        let mut f = [0.0f32; NUM_MODES];
        let mut gain = [0.0f32; NUM_MODES];
        for i in 0..NUM_MODES {
            f[i] = (f0 * MODE_FREQUENCIES[i]).min(0.499);
            self.resonator[i].set_f_q::<FREQUENCY_FAST>(f[i], 1.0 + f[i] * if i == 0 { q } else { q * 0.25 });
        }

        if tone < 0.666667 {
            // Two modes.
            tone *= 1.5;
            gain[0] = 1.5 + (1.0 - tone) * (1.0 - tone) * 4.5;
            gain[1] = 2.0 * tone + 0.15;
            for g in gain.iter_mut().skip(2) {
                *g = 0.0;
            }
        } else {
            // With extra modes.
            tone = (tone - 0.666667) * 3.0;
            gain[0] = 1.5 - tone * 0.5;
            gain[1] = 2.15 - tone * 0.7;
            for g in gain.iter_mut().skip(2) {
                *g = tone;
                tone *= tone;
            }
        }

        let f_noise = constrain(f0 * 16.0, 0.0, 0.499);
        self.noise_filter.set_f_q::<FREQUENCY_FAST>(f_noise, 1.0 + f_noise * 1.5);

        let size = out.len();
        let mut sustain_gain = ParameterInterpolator::new(&mut self.sustain_gain, accent * decay, size);
        for o in out.iter_mut() {
            // Q45 / Q46
            let pulse;
            if self.pulse_remaining_samples != 0 {
                self.pulse_remaining_samples -= 1;
                pulse = if self.pulse_remaining_samples != 0 { self.pulse_height } else { self.pulse_height - 1.0 };
                self.pulse = pulse;
            } else {
                self.pulse *= 1.0 - 1.0 / pulse_decay_time;
                pulse = self.pulse;
            }

            let sustain_gain_value = sustain_gain.next();

            // R189 / C57 / R190 + C58 / C59 / R197 / R196 / IC14
            one_pole(&mut self.pulse_lp, pulse, 0.75);

            let mut shell = 0.0;
            for i in 0..NUM_MODES {
                let excitation = if i == 0 { (pulse - self.pulse_lp) + 0.006 * pulse } else { 0.026 * pulse };
                shell += gain[i]
                    * if sustain {
                        self.oscillator[i].next(f[i]) * sustain_gain_value * 0.25
                    } else {
                        self.resonator[i].process::<FILTER_MODE_BAND_PASS>(excitation) + excitation * exciter_leak
                    };
            }
            shell = soft_clip(shell);

            // C56 / R194 / Q48 / C54 / R188 / D54
            let mut noise = 2.0 * rng.get_float() - 1.0;
            if noise < 0.0 {
                noise = 0.0;
            }
            self.noise_envelope *= noise_envelope_decay;
            noise *= (if sustain { sustain_gain_value } else { self.noise_envelope }) * snappy * 2.0;

            // C66 / R201 / C67 / R202 / R203 / Q49
            noise = self.noise_filter.process::<FILTER_MODE_BAND_PASS>(noise);

            // IC13
            *o = noise + shell * (1.0 - snappy);
        }
    }
}

// ----------------------------------------- drums/synthetic_snare_drum.h

/// Naive snare drum model (two coupled oscillators + filtered noise).
#[derive(Clone, Copy, Debug, Default)]
pub struct SyntheticSnareDrum {
    phase: [f32; 2],
    drum_amplitude: f32,
    snare_amplitude: f32,
    fm: f32,
    sustain_gain: f32,
    hold_counter: i32,
    drum_lp: OnePole,
    snare_hp: OnePole,
    snare_lp: Svf,
}

#[inline]
fn distorted_sine(phase: f32) -> f32 {
    let triangle = (if phase < 0.5 { phase } else { 1.0 - phase }) * 4.0 - 1.3;
    2.0 * triangle / (1.0 + triangle.abs())
}

impl SyntheticSnareDrum {
    pub fn init(&mut self) {
        self.phase = [0.0; 2];
        self.drum_amplitude = 0.0;
        self.snare_amplitude = 0.0;
        self.fm = 0.0;
        self.hold_counter = 0;
        self.sustain_gain = 0.0;
        self.drum_lp.init();
        self.snare_hp.init();
        self.snare_lp.init();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        sustain: bool,
        trigger: bool,
        accent: f32,
        f0: f32,
        mut fm_amount: f32,
        decay: f32,
        mut snappy: f32,
        out: &mut [f32],
        rng: &mut Random,
    ) {
        let decay_xt = decay * (1.0 + decay * (decay - 1.0));
        fm_amount *= fm_amount;
        let drum_decay = 1.0
            - 1.0 / (0.015 * SAMPLE_RATE) * semitones_to_ratio(-decay_xt * 72.0 - fm_amount * 12.0 + snappy * 7.0);
        let snare_decay = 1.0 - 1.0 / (0.01 * SAMPLE_RATE) * semitones_to_ratio(-decay * 60.0 - snappy * 7.0);
        let fm_decay = 1.0 - 1.0 / (0.007 * SAMPLE_RATE);

        snappy = constrain(snappy * 1.1 - 0.05, 0.0, 1.0);

        let drum_level = (1.0 - snappy).sqrt();
        let snare_level = snappy.sqrt();

        let snare_f_min = (10.0 * f0).min(0.5);
        let snare_f_max = (35.0 * f0).min(0.5);

        self.snare_hp.set_f::<FREQUENCY_FAST>(snare_f_min);
        self.snare_lp.set_f_q::<FREQUENCY_FAST>(snare_f_max, 0.5 + 2.0 * snappy);
        self.drum_lp.set_f::<FREQUENCY_FAST>(3.0 * f0);

        if trigger {
            self.drum_amplitude = 0.3 + 0.7 * accent;
            self.snare_amplitude = self.drum_amplitude;
            self.fm = 1.0;
            self.phase = [0.0; 2];
            self.hold_counter = ((0.04 + decay * 0.03) * SAMPLE_RATE) as i32;
        }

        let size = out.len();
        let mut sustain_gain = ParameterInterpolator::new(&mut self.sustain_gain, accent * decay, size);
        for n in 0..size {
            // The original counts `size` down to 0 inside the loop; this is
            // the remaining count after its decrement.
            let remaining = size - 1 - n;
            if sustain {
                self.snare_amplitude = sustain_gain.next();
                self.drum_amplitude = self.snare_amplitude;
                self.fm = 0.0;
            } else {
                // Long-tailed drum envelope; the snare envelope holds for
                // 40 to 70 ms first.
                self.drum_amplitude *=
                    if self.drum_amplitude > 0.03 || (remaining & 1) == 0 { drum_decay } else { 1.0 };
                if self.hold_counter != 0 {
                    self.hold_counter -= 1;
                } else {
                    self.snare_amplitude *= snare_decay;
                }
                self.fm *= fm_decay;
            }

            // Oscillator coupling through the shared reset line.
            let mut reset_noise = 0.0;
            let mut reset_noise_amount = constrain((0.125 - f0) * 8.0, 0.0, 1.0);
            reset_noise_amount *= reset_noise_amount;
            reset_noise_amount *= fm_amount;
            reset_noise += if self.phase[0] > 0.5 { -1.0 } else { 1.0 };
            reset_noise += if self.phase[1] > 0.5 { -1.0 } else { 1.0 };
            reset_noise *= reset_noise_amount * 0.025;

            let f = f0 * (1.0 + fm_amount * (4.0 * self.fm));
            self.phase[0] += f;
            self.phase[1] += f * 1.47;
            if reset_noise_amount > 0.1 {
                if self.phase[0] >= 1.0 + reset_noise {
                    self.phase[0] = 1.0 - self.phase[0];
                }
                if self.phase[1] >= 1.0 + reset_noise {
                    self.phase[1] = 1.0 - self.phase[1];
                }
            } else {
                if self.phase[0] >= 1.0 {
                    self.phase[0] -= 1.0;
                }
                if self.phase[1] >= 1.0 {
                    self.phase[1] -= 1.0;
                }
            }

            let mut drum = -0.1;
            drum += distorted_sine(self.phase[0]) * 0.60;
            drum += distorted_sine(self.phase[1]) * 0.25;
            drum *= self.drum_amplitude * drum_level;
            drum = self.drum_lp.process::<FILTER_MODE_LOW_PASS>(drum);

            let noise = rng.get_float();
            let mut snare = self.snare_lp.process::<FILTER_MODE_LOW_PASS>(noise);
            snare = self.snare_hp.process::<FILTER_MODE_HIGH_PASS>(snare);
            snare = (snare + 0.1) * (self.snare_amplitude + self.fm) * snare_level;

            out[n] = snare + drum;
        }
    }
}

// -------------------------------------------- engine/snare_drum_engine.*

/// Analog and synthetic snare drum generators (main / aux).
pub struct SnareDrumEngine {
    analog_snare_drum: AnalogSnareDrum,
    synthetic_snare_drum: SyntheticSnareDrum,
}

impl SnareDrumEngine {
    pub fn new() -> Self {
        let mut e =
            SnareDrumEngine { analog_snare_drum: AnalogSnareDrum::default(), synthetic_snare_drum: SyntheticSnareDrum::default() };
        e.analog_snare_drum.init();
        e.synthetic_snare_drum.init();
        e
    }
}

impl Engine for SnareDrumEngine {
    fn reset(&mut self) {}

    fn render(
        &mut self,
        p: &EngineParameters,
        out: &mut [f32],
        aux: &mut [f32],
        _already_enveloped: &mut bool,
        rng: &mut Random,
    ) {
        let f0 = note_to_frequency(p.note);
        let sustain = p.trigger & TRIGGER_UNPATCHED != 0;
        let rising = p.trigger & TRIGGER_RISING_EDGE != 0;
        self.analog_snare_drum.render(sustain, rising, p.accent, f0, p.timbre, p.morph, p.harmonics, out, rng);
        self.synthetic_snare_drum.render(sustain, rising, p.accent, f0, p.timbre, p.morph, p.harmonics, aux, rng);
    }
}
