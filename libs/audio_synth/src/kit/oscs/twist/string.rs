// Rust port of the twist engine's inharmonic string model (engine 11):
// dsp/engine/string_engine.h + string_engine.cc, dsp/physical_modelling/string_voice.h
// + string_voice.cc and dsp/physical_modelling/string.h + string.cc of the MIT
// eurorack macro-oscillator. Original notices:
//
// Copyright 2016 Emilie Gillet. (dsp/engine/string_engine.h, dsp/engine/string_engine.cc,
//                                dsp/physical_modelling/string_voice.h, string_voice.cc,
//                                dsp/physical_modelling/string.h, string.cc)
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

// ------------------------------------------ physical_modelling/string.*

const DELAY_LINE_SIZE: usize = 1024;

const NON_LINEARITY_CURVED_BRIDGE: u8 = 0;
const NON_LINEARITY_DISPERSION: u8 = 1;

/// Comb filter / Karplus-Strong string ("lite" version).
pub struct KsString {
    string: DelayLine<DELAY_LINE_SIZE>,
    stretch: DelayLine<{ DELAY_LINE_SIZE / 4 }>,
    iir_damping_filter: Svf,
    dc_blocker: DcBlocker,
    delay: f32,
    dispersion_noise: f32,
    curved_bridge: f32,
    // Linear interpolation upsampler for pitches that do not fit the line.
    src_phase: f32,
    out_sample: [f32; 2],
}

impl KsString {
    pub fn new() -> KsString {
        let mut s = KsString {
            string: DelayLine::default(),
            stretch: DelayLine::default(),
            iir_damping_filter: Svf::default(),
            dc_blocker: DcBlocker::default(),
            delay: 100.0,
            dispersion_noise: 0.0,
            curved_bridge: 0.0,
            src_phase: 0.0,
            out_sample: [0.0; 2],
        };
        s.reset();
        s
    }

    pub fn reset(&mut self) {
        self.string.reset();
        self.stretch.reset();
        self.iir_damping_filter.init();
        self.dc_blocker.init(1.0 - 20.0 / SAMPLE_RATE);
        self.dispersion_noise = 0.0;
        self.curved_bridge = 0.0;
        self.out_sample = [0.0; 2];
        self.src_phase = 0.0;
    }

    pub fn process(
        &mut self,
        f0: f32,
        non_linearity_amount: f32,
        brightness: f32,
        damping: f32,
        input: &[f32],
        out: &mut [f32],
        rng: &mut Random,
    ) {
        if non_linearity_amount <= 0.0 {
            self.process_internal::<NON_LINEARITY_CURVED_BRIDGE>(
                f0, -non_linearity_amount, brightness, damping, input, out, rng,
            );
        } else {
            self.process_internal::<NON_LINEARITY_DISPERSION>(
                f0, non_linearity_amount, brightness, damping, input, out, rng,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn process_internal<const NL: u8>(
        &mut self,
        f0: f32,
        non_linearity_amount: f32,
        mut brightness: f32,
        damping: f32,
        input: &[f32],
        out: &mut [f32],
        rng: &mut Random,
    ) {
        let size = out.len();
        let delay = constrain(1.0 / f0, 4.0, DELAY_LINE_SIZE as f32 - 4.0);

        // Below ~11.7 Hz the line is too short: play the lowest note and
        // upsample with a linear interpolator.
        let mut src_ratio = delay * f0;
        if src_ratio >= 0.9999 {
            self.src_phase = 1.0;
            src_ratio = 1.0;
        }

        let mut damping_cutoff = (12.0 + damping * damping * 60.0 + brightness * 24.0).min(84.0);
        let mut damping_f = (f0 * semitones_to_ratio(damping_cutoff)).min(0.499);

        // Crossfade to infinite decay.
        if damping >= 0.95 {
            let to_infinite = 20.0 * (damping - 0.95);
            brightness += to_infinite * (1.0 - brightness);
            damping_f += to_infinite * (0.4999 - damping_f);
            damping_cutoff += to_infinite * (128.0 - damping_cutoff);
        }

        self.iir_damping_filter.set_f_q::<FREQUENCY_FAST>(damping_f, 0.5);

        let damping_compensation = interpolate(&tables().svf_shift, damping_cutoff, 1.0);

        let stretch_point = non_linearity_amount * (2.0 - non_linearity_amount) * 0.225;
        let stretch_correction = constrain((160.0 / SAMPLE_RATE) * delay, 1.0, 2.1);

        let noise_amount_sqrt =
            if non_linearity_amount > 0.75 { 4.0 * (non_linearity_amount - 0.75) } else { 0.0 };
        let noise_amount = noise_amount_sqrt * noise_amount_sqrt * 0.1;
        let noise_filter = 0.06 + 0.94 * brightness * brightness;

        let bridge_curving_sqrt = non_linearity_amount;
        let bridge_curving = bridge_curving_sqrt * bridge_curving_sqrt * 0.01;

        let ap_gain = -0.618 * non_linearity_amount / (0.15 + non_linearity_amount.abs());

        let mut delay_modulation = ParameterInterpolator::new(&mut self.delay, delay * damping_compensation, size);
        for i in 0..size {
            self.src_phase += src_ratio;
            if self.src_phase > 1.0 {
                self.src_phase -= 1.0;

                let mut delay = delay_modulation.next();
                let mut s;

                if NL == NON_LINEARITY_DISPERSION {
                    let noise = rng.get_float() - 0.5;
                    one_pole(&mut self.dispersion_noise, noise, noise_filter);
                    delay *= 1.0 + self.dispersion_noise * noise_amount;
                } else {
                    delay *= 1.0 - self.curved_bridge * bridge_curving;
                }

                if NL == NON_LINEARITY_DISPERSION {
                    let ap_delay = delay * stretch_point;
                    let main_delay = delay - ap_delay * (0.408 - stretch_point * 0.308) * stretch_correction;
                    if ap_delay >= 4.0 && main_delay >= 4.0 {
                        s = self.string.read_f(main_delay);
                        s = self.stretch.allpass(s, ap_delay as usize, ap_gain);
                    } else {
                        s = self.string.read_hermite(delay);
                    }
                } else {
                    s = self.string.read_hermite(delay);
                }

                if NL == NON_LINEARITY_CURVED_BRIDGE {
                    let value = s.abs() - 0.025;
                    let sign = if s > 0.0 { 1.0 } else { -1.5 };
                    self.curved_bridge = (value.abs() + value) * sign;
                }

                s += input[i];
                s = constrain(s, -20.0, 20.0);

                let mut one = [s];
                self.dc_blocker.process(&mut one);
                s = one[0];

                s = self.iir_damping_filter.process::<FILTER_MODE_LOW_PASS>(s);
                self.string.write(s);

                self.out_sample[1] = self.out_sample[0];
                self.out_sample[0] = s;
            }
            out[i] += crossfade(self.out_sample[1], self.out_sample[0], self.src_phase);
        }
    }
}

// ------------------------------------- physical_modelling/string_voice.*

/// Extended Karplus-Strong voice with a noise / dust exciter.
pub struct StringVoice {
    excitation_filter: Svf,
    string: KsString,
    remaining_noise_samples: usize,
}

impl StringVoice {
    pub fn new() -> StringVoice {
        let mut v = StringVoice { excitation_filter: Svf::default(), string: KsString::new(), remaining_noise_samples: 0 };
        v.excitation_filter.init();
        v
    }

    pub fn reset(&mut self) {
        self.string.reset();
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
        let size = out.len();
        let density = brightness * brightness;

        brightness += 0.25 * accent * (1.0 - brightness);
        damping += 0.25 * accent * (1.0 - damping);

        // Synthesize excitation signal.
        if trigger || sustain {
            let range = 72.0;
            let f = 4.0 * f0;
            let cutoff = (f * semitones_to_ratio((brightness * (2.0 - brightness) - 0.5) * range)).min(0.499);
            let q = if sustain { 1.0 } else { 0.5 };
            self.remaining_noise_samples = (1.0 / f0) as usize;
            self.excitation_filter.set_f_q::<FREQUENCY_DIRTY>(cutoff, q);
        }

        if sustain {
            let dust_f = 0.00005 + 0.99995 * density * density;
            for t in temp.iter_mut() {
                *t = dust(dust_f, rng) * (8.0 - dust_f * 6.0) * accent;
            }
        } else if self.remaining_noise_samples != 0 {
            let noise_samples = self.remaining_noise_samples.min(size);
            self.remaining_noise_samples -= noise_samples;
            for (i, t) in temp.iter_mut().enumerate() {
                *t = if i < noise_samples { 2.0 * rng.get_float() - 1.0 } else { 0.0 };
            }
        } else {
            temp.fill(0.0);
        }

        self.excitation_filter.process_in_place::<FILTER_MODE_LOW_PASS>(temp);
        for i in 0..size {
            aux[i] += temp[i];
        }

        let non_linearity = if structure < 0.24 {
            (structure - 0.24) * 4.166
        } else if structure > 0.26 {
            (structure - 0.26) * 1.35135
        } else {
            0.0
        };
        self.string.process(f0, non_linearity, brightness, damping, temp, out, rng);
    }
}

// ------------------------------------------------ engine/string_engine.*

const NUM_STRINGS: usize = 3;

/// Three voices of string synthesis.
pub struct StringEngine {
    voice: [StringVoice; NUM_STRINGS],
    f0: [f32; NUM_STRINGS],
    f0_delay: DelayLine<16>,
    active_string: usize,
    temp_buffer: [f32; MAX_BLOCK_SIZE],
}

impl StringEngine {
    pub fn new() -> Self {
        StringEngine {
            voice: [StringVoice::new(), StringVoice::new(), StringVoice::new()],
            f0: [0.01; NUM_STRINGS],
            f0_delay: DelayLine::default(),
            active_string: NUM_STRINGS - 1,
            temp_buffer: [0.0; MAX_BLOCK_SIZE],
        }
    }
}

impl Engine for StringEngine {
    fn reset(&mut self) {
        self.f0_delay.reset();
        for v in self.voice.iter_mut() {
            v.reset();
        }
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
        if p.trigger & TRIGGER_RISING_EDGE != 0 {
            // 8 in the original firmware version; 14 for slow MIDI interfaces.
            self.f0[self.active_string] = self.f0_delay.read_f(14.0);
            self.active_string = (self.active_string + 1) % NUM_STRINGS;
        }

        let f0 = note_to_frequency(p.note);
        self.f0[self.active_string] = f0;
        self.f0_delay.write(f0);

        out.fill(0.0);
        aux.fill(0.0);

        let temp = &mut self.temp_buffer[..size];
        for i in 0..NUM_STRINGS {
            self.voice[i].render(
                p.trigger & TRIGGER_UNPATCHED != 0 && i == self.active_string,
                p.trigger & TRIGGER_RISING_EDGE != 0 && i == self.active_string,
                p.accent,
                self.f0[i],
                p.harmonics,
                p.timbre * p.timbre,
                p.morph,
                temp,
                out,
                aux,
                rng,
            );
        }
    }
}
