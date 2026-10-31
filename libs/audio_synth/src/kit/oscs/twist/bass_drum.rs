// Rust port of the twist engine's kick model (engine 13):
// dsp/engine/bass_drum_engine.h + bass_drum_engine.cc, dsp/drums/analog_bass_drum.h,
// dsp/drums/synthetic_bass_drum.h and dsp/fx/overdrive.h of the MIT eurorack
// macro-oscillator. Original notices:
//
// Copyright 2016 Emilie Gillet. (dsp/engine/bass_drum_engine.h, dsp/engine/bass_drum_engine.cc,
//                                dsp/drums/analog_bass_drum.h, dsp/drums/synthetic_bass_drum.h)
// Copyright 2014 Emilie Gillet. (dsp/fx/overdrive.h)
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
use super::osc_base::{sine, SineOscillator};
use super::stm::*;

// ---------------------------------------------------- fx/overdrive.h

/// Distortion / overdrive.
#[derive(Clone, Copy, Debug, Default)]
pub struct Overdrive {
    pre_gain: f32,
    post_gain: f32,
}

impl Overdrive {
    pub fn init(&mut self) {
        self.pre_gain = 0.0;
        self.post_gain = 0.0;
    }

    pub fn process(&mut self, drive: f32, in_out: &mut [f32]) {
        let size = in_out.len();
        let drive_2 = drive * drive;
        let pre_gain_a = drive * 0.5;
        let pre_gain_b = drive_2 * drive_2 * drive * 24.0;
        let pre_gain = pre_gain_a + (pre_gain_b - pre_gain_a) * drive_2;
        let drive_squashed = drive * (2.0 - drive);
        let post_gain = 1.0 / soft_clip(0.33 + drive_squashed * (pre_gain - 0.33));
        let mut pre_gain_modulation = ParameterInterpolator::new(&mut self.pre_gain, pre_gain, size);
        let mut post_gain_modulation = ParameterInterpolator::new(&mut self.post_gain, post_gain, size);
        for x in in_out.iter_mut() {
            let pre = pre_gain_modulation.next() * *x;
            *x = soft_clip(pre) * post_gain_modulation.next();
        }
    }
}

// --------------------------------------------- drums/analog_bass_drum.h

/// Analog bass drum model (bridged-T resonator with pulse exciter).
#[derive(Clone, Copy, Debug, Default)]
pub struct AnalogBassDrum {
    pulse_remaining_samples: i32,
    fm_pulse_remaining_samples: i32,
    pulse: f32,
    pulse_height: f32,
    pulse_lp: f32,
    fm_pulse_lp: f32,
    retrig_pulse: f32,
    lp_out: f32,
    tone_lp: f32,
    sustain_gain: f32,
    resonator: Svf,
    // Replaces the resonator in free-running (sustain) mode.
    oscillator: SineOscillator,
}

#[inline]
fn diode(x: f32) -> f32 {
    if x >= 0.0 {
        x
    } else {
        let x = x * 2.0;
        0.7 * x / (1.0 + x.abs())
    }
}

impl AnalogBassDrum {
    pub fn init(&mut self) {
        self.pulse_remaining_samples = 0;
        self.fm_pulse_remaining_samples = 0;
        self.pulse = 0.0;
        self.pulse_height = 0.0;
        self.pulse_lp = 0.0;
        self.fm_pulse_lp = 0.0;
        self.retrig_pulse = 0.0;
        self.lp_out = 0.0;
        self.tone_lp = 0.0;
        self.sustain_gain = 0.0;
        self.resonator.init();
        self.oscillator.init();
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
        attack_fm_amount: f32,
        self_fm_amount: f32,
        out: &mut [f32],
    ) {
        let trigger_pulse_duration = (1.0e-3 * SAMPLE_RATE) as i32;
        let fm_pulse_duration = (6.0e-3 * SAMPLE_RATE) as i32;
        let pulse_decay_time = 0.2e-3 * SAMPLE_RATE;
        let pulse_filter_time = 0.1e-3 * SAMPLE_RATE;
        let retrig_pulse_duration = 0.05 * SAMPLE_RATE;

        let scale = 0.001 / f0;
        let q = 1500.0 * semitones_to_ratio(decay * 80.0);
        let tone_f = (4.0 * f0 * semitones_to_ratio(tone * 108.0)).min(1.0);
        let exciter_leak = 0.08 * (tone + 0.25);

        if trigger {
            self.pulse_remaining_samples = trigger_pulse_duration;
            self.fm_pulse_remaining_samples = fm_pulse_duration;
            self.pulse_height = 3.0 + 7.0 * accent;
            self.lp_out = 0.0;
        }

        let size = out.len();
        let mut sustain_gain = ParameterInterpolator::new(&mut self.sustain_gain, accent * decay, size);
        for o in out.iter_mut() {
            // Q39 / Q40
            let mut pulse;
            if self.pulse_remaining_samples != 0 {
                self.pulse_remaining_samples -= 1;
                pulse = if self.pulse_remaining_samples != 0 { self.pulse_height } else { self.pulse_height - 1.0 };
                self.pulse = pulse;
            } else {
                self.pulse *= 1.0 - 1.0 / pulse_decay_time;
                pulse = self.pulse;
            }
            if sustain {
                pulse = 0.0;
            }

            // C40 / R163 / R162 / D83
            one_pole(&mut self.pulse_lp, pulse, 1.0 / pulse_filter_time);
            pulse = diode((pulse - self.pulse_lp) + pulse * 0.044);

            // Q41 / Q42
            let mut fm_pulse = 0.0;
            if self.fm_pulse_remaining_samples != 0 {
                self.fm_pulse_remaining_samples -= 1;
                fm_pulse = 1.0;
                // C39 / C52
                self.retrig_pulse = if self.fm_pulse_remaining_samples != 0 { 0.0 } else { -0.8 };
            } else {
                // C39 / R161
                self.retrig_pulse *= 1.0 - 1.0 / retrig_pulse_duration;
            }
            if sustain {
                fm_pulse = 0.0;
            }
            one_pole(&mut self.fm_pulse_lp, fm_pulse, 1.0 / pulse_filter_time);

            // Q43 and R170 leakage
            let punch = 0.7 + diode(10.0 * self.lp_out - 1.0);

            // Q43 / R165
            let attack_fm = self.fm_pulse_lp * 1.7 * attack_fm_amount;
            let self_fm = punch * 0.08 * self_fm_amount;
            let f = constrain(f0 * (1.0 + attack_fm + self_fm), 0.0, 0.4);

            let resonator_out;
            if sustain {
                let (s, c) = self.oscillator.next_quadrature(f, sustain_gain.next());
                resonator_out = s;
                self.lp_out = c;
            } else {
                self.resonator.set_f_q::<FREQUENCY_DIRTY>(f, 1.0 + q * f);
                let (bp, lp) = self
                    .resonator
                    .process2::<FILTER_MODE_BAND_PASS, FILTER_MODE_LOW_PASS>((pulse - self.retrig_pulse * 0.2) * scale);
                resonator_out = bp;
                self.lp_out = lp;
            }

            one_pole(&mut self.tone_lp, pulse * exciter_leak + resonator_out, tone_f);
            *o = self.tone_lp;
        }
    }
}

// ------------------------------------------ drums/synthetic_bass_drum.h

#[derive(Clone, Copy, Debug, Default)]
struct SyntheticBassDrumClick {
    lp: f32,
    hp: f32,
    filter: Svf,
}

impl SyntheticBassDrumClick {
    fn init(&mut self) {
        self.lp = 0.0;
        self.hp = 0.0;
        self.filter.init();
        self.filter.set_f_q::<FREQUENCY_FAST>(5000.0 / SAMPLE_RATE, 2.0);
    }

    fn process(&mut self, input: f32) -> f32 {
        slope(&mut self.lp, input, 0.5, 0.1);
        one_pole(&mut self.hp, self.lp, 0.04);
        self.filter.process::<FILTER_MODE_LOW_PASS>(self.lp - self.hp)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct SyntheticBassDrumAttackNoise {
    lp: f32,
    hp: f32,
}

impl SyntheticBassDrumAttackNoise {
    fn init(&mut self) {
        self.lp = 0.0;
        self.hp = 0.0;
    }

    fn render(&mut self, rng: &mut Random) -> f32 {
        let sample = rng.get_float();
        one_pole(&mut self.lp, sample, 0.05);
        one_pole(&mut self.hp, self.lp, 0.005);
        self.lp - self.hp
    }
}

/// `init()` does not clear the transient envelope's smoother.
#[allow(dead_code)]
const QUIRK_SYNTH_KICK_TRANSIENT_LP_NOT_RESET: bool = true;

/// Naive bass drum model (modulated oscillator with FM + envelope).
#[derive(Clone, Copy, Debug, Default)]
pub struct SyntheticBassDrum {
    f0: f32,
    phase: f32,
    phase_noise: f32,
    fm: f32,
    fm_lp: f32,
    body_env: f32,
    body_env_lp: f32,
    transient_env: f32,
    transient_env_lp: f32,
    sustain_gain: f32,
    tone_lp: f32,
    click: SyntheticBassDrumClick,
    noise: SyntheticBassDrumAttackNoise,
    body_env_pulse_width: i32,
    fm_pulse_width: i32,
}

#[inline]
fn distorted_sine(phase: f32, phase_noise: f32, dirtiness: f32) -> f32 {
    let phase = phase + phase_noise * dirtiness;
    let (_, phase) = integral_fractional(phase);
    let triangle = (if phase < 0.5 { phase } else { 1.0 - phase }) * 4.0 - 1.0;
    let sine_ = 2.0 * triangle / (1.0 + triangle.abs());
    let clean_sine = sine(phase + 0.75);
    sine_ + (1.0 - dirtiness) * (clean_sine - sine_)
}

#[inline]
fn transistor_vca(s: f32, gain: f32) -> f32 {
    let s = (s - 0.6) * gain;
    3.0 * s / (2.0 + s.abs()) + gain * 0.3
}

impl SyntheticBassDrum {
    pub fn init(&mut self) {
        // QUIRK_SYNTH_KICK_TRANSIENT_LP_NOT_RESET: transient_env_lp keeps its value.
        self.phase = 0.0;
        self.phase_noise = 0.0;
        self.f0 = 0.0;
        self.fm = 0.0;
        self.fm_lp = 0.0;
        self.body_env_lp = 0.0;
        self.body_env = 0.0;
        self.body_env_pulse_width = 0;
        self.fm_pulse_width = 0;
        self.tone_lp = 0.0;
        self.sustain_gain = 0.0;
        self.click.init();
        self.noise.init();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        sustain: bool,
        trigger: bool,
        accent: f32,
        f0: f32,
        tone: f32,
        mut decay: f32,
        mut dirtiness: f32,
        fm_envelope_amount: f32,
        mut fm_envelope_decay: f32,
        out: &mut [f32],
        rng: &mut Random,
    ) {
        let size = out.len();
        decay *= decay;
        fm_envelope_decay *= fm_envelope_decay;

        dirtiness *= (1.0 - 8.0 * f0).max(0.0);

        let fm_decay = 1.0 - 1.0 / (0.008 * (1.0 + fm_envelope_decay * 4.0) * SAMPLE_RATE);
        let body_env_decay = 1.0 - 1.0 / (0.02 * SAMPLE_RATE) * semitones_to_ratio(-decay * 60.0);
        let transient_env_decay = 1.0 - 1.0 / (0.005 * SAMPLE_RATE);
        let tone_f = (4.0 * f0 * semitones_to_ratio(tone * 108.0)).min(1.0);
        let transient_level = tone;

        if trigger {
            self.fm = 1.0;
            self.body_env = 0.3 + 0.7 * accent;
            self.transient_env = self.body_env;
            self.body_env_pulse_width = (SAMPLE_RATE * 0.001) as i32;
            self.fm_pulse_width = (SAMPLE_RATE * 0.0013) as i32;
        }

        let mut f0_mod = ParameterInterpolator::new(&mut self.f0, f0, size);
        let mut sustain_gain = ParameterInterpolator::new(&mut self.sustain_gain, accent * decay, size);
        for o in out.iter_mut() {
            one_pole(&mut self.phase_noise, rng.get_float() - 0.5, 0.002);

            let mut mix = 0.0;
            if sustain {
                self.phase += f0_mod.next();
                if self.phase >= 1.0 {
                    self.phase -= 1.0;
                }
                let body = distorted_sine(self.phase, self.phase_noise, dirtiness);
                mix -= transistor_vca(body, sustain_gain.next());
            } else {
                if self.fm_pulse_width != 0 {
                    self.fm_pulse_width -= 1;
                    self.phase = 0.25;
                } else {
                    self.fm *= fm_decay;
                    let fm = 1.0 + fm_envelope_amount * 3.5 * self.fm_lp;
                    self.phase += (f0_mod.next() * fm).min(0.5);
                    if self.phase >= 1.0 {
                        self.phase -= 1.0;
                    }
                }

                if self.body_env_pulse_width != 0 {
                    self.body_env_pulse_width -= 1;
                } else {
                    self.body_env *= body_env_decay;
                    self.transient_env *= transient_env_decay;
                }

                let envelope_lp_f = 0.1;
                one_pole(&mut self.body_env_lp, self.body_env, envelope_lp_f);
                one_pole(&mut self.transient_env_lp, self.transient_env, envelope_lp_f);
                one_pole(&mut self.fm_lp, self.fm, envelope_lp_f);

                let body = distorted_sine(self.phase, self.phase_noise, dirtiness);
                let transient = self.click.process(if self.body_env_pulse_width != 0 { 0.0 } else { 1.0 })
                    + self.noise.render(rng);

                mix -= transistor_vca(body, self.body_env_lp);
                mix -= transient * self.transient_env_lp * transient_level;
            }

            one_pole(&mut self.tone_lp, mix, tone_f);
            *o = self.tone_lp;
        }
    }
}

// --------------------------------------------- engine/bass_drum_engine.*

/// Analog and synthetic bass drum generators (main / aux).
pub struct BassDrumEngine {
    analog_bass_drum: AnalogBassDrum,
    synthetic_bass_drum: SyntheticBassDrum,
    overdrive: Overdrive,
}

impl BassDrumEngine {
    pub fn new() -> Self {
        let mut e = BassDrumEngine {
            analog_bass_drum: AnalogBassDrum::default(),
            synthetic_bass_drum: SyntheticBassDrum::default(),
            overdrive: Overdrive::default(),
        };
        e.analog_bass_drum.init();
        e.synthetic_bass_drum.init();
        e.overdrive.init();
        e
    }
}

impl Engine for BassDrumEngine {
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

        let attack_fm_amount = (p.harmonics * 4.0).min(1.0);
        let self_fm_amount = (p.harmonics * 4.0 - 1.0).min(1.0).max(0.0);
        let drive = (p.harmonics * 2.0 - 1.0).max(0.0) * (1.0 - 16.0 * f0).max(0.0);
        let sustain = p.trigger & TRIGGER_UNPATCHED != 0;
        let rising = p.trigger & TRIGGER_RISING_EDGE != 0;

        self.analog_bass_drum.render(sustain, rising, p.accent, f0, p.timbre, p.morph, attack_fm_amount, self_fm_amount, out);
        self.overdrive.process(0.5 + 0.5 * drive, out);
        self.synthetic_bass_drum.render(
            sustain,
            rising,
            p.accent,
            f0,
            p.timbre,
            p.morph,
            if sustain { p.harmonics } else { 0.4 - 0.25 * p.morph * p.morph },
            (p.harmonics * 2.0).min(1.0),
            (p.harmonics * 2.0 - 1.0).max(0.0),
            aux,
            rng,
        );
    }
}
