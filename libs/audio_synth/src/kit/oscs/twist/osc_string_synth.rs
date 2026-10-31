// Rust port of the twist engine's divide-down organ oscillator:
// dsp/oscillator/string_synth_oscillator.h of the MIT eurorack macro-oscillator.
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


use super::stm::*;

/// A mixture of 7 saw and square waveforms in the style of divide-down
/// organs: saw 8', square 8', saw 4', square 4', saw 2', square 2', saw 1'.
/// Renders 4 band-limited saws from one phase counter; squares come from
/// square 16' = 2 saw 16' - saw 8'.
#[derive(Clone, Copy, Debug, Default)]
pub struct StringSynthOscillator {
    phase: f32,
    next_sample: f32,
    segment: i32,
    frequency: f32,
    saw_8_gain: f32,
    saw_4_gain: f32,
    saw_2_gain: f32,
    saw_1_gain: f32,
}

impl StringSynthOscillator {
    pub fn init(&mut self) {
        self.phase = 0.0;
        self.next_sample = 0.0;
        self.segment = 0;
        self.frequency = 0.001;
        self.saw_8_gain = 0.0;
        self.saw_4_gain = 0.0;
        self.saw_2_gain = 0.0;
        self.saw_1_gain = 0.0;
    }

    /// Adds into `out`. `unshifted_registration` has (at least) 7 entries.
    pub fn render(&mut self, frequency: f32, unshifted_registration: &[f32], gain: f32, out: &mut [f32]) {
        let size = out.len();
        let mut frequency = frequency * 8.0;

        // Very high frequencies: shift everything 1 or 2 octaves down.
        let mut shift = 0usize;
        while frequency > 0.5 {
            shift += 2;
            frequency *= 0.5;
        }
        if shift >= 8 {
            return;
        }

        let mut registration = [0.0f32; 7];
        registration[shift..7].copy_from_slice(&unshifted_registration[..7 - shift]);

        let mut phase = self.phase;
        let mut next_sample = self.next_sample;
        let mut segment = self.segment;
        {
            let mut fm = ParameterInterpolator::new(&mut self.frequency, frequency, size);
            let mut saw_8_gain_modulation =
                ParameterInterpolator::new(&mut self.saw_8_gain, (registration[0] + 2.0 * registration[1]) * gain, size);
            let mut saw_4_gain_modulation = ParameterInterpolator::new(
                &mut self.saw_4_gain,
                (registration[2] - registration[1] + 2.0 * registration[3]) * gain,
                size,
            );
            let mut saw_2_gain_modulation = ParameterInterpolator::new(
                &mut self.saw_2_gain,
                (registration[4] - registration[3] + 2.0 * registration[5]) * gain,
                size,
            );
            let mut saw_1_gain_modulation =
                ParameterInterpolator::new(&mut self.saw_1_gain, (registration[6] - registration[5]) * gain, size);

            for o in out.iter_mut() {
                let mut this_sample = next_sample;
                next_sample = 0.0;

                let frequency = fm.next();
                let saw_8_gain = saw_8_gain_modulation.next();
                let saw_4_gain = saw_4_gain_modulation.next();
                let saw_2_gain = saw_2_gain_modulation.next();
                let saw_1_gain = saw_1_gain_modulation.next();

                phase += frequency;
                let mut next_segment = phase as i32;
                if next_segment != segment {
                    let mut discontinuity = 0.0;
                    if next_segment == 8 {
                        phase -= 8.0;
                        next_segment -= 8;
                        discontinuity -= saw_8_gain;
                    }
                    if (next_segment & 3) == 0 {
                        discontinuity -= saw_4_gain;
                    }
                    if (next_segment & 1) == 0 {
                        discontinuity -= saw_2_gain;
                    }
                    discontinuity -= saw_1_gain;
                    if discontinuity != 0.0 {
                        let fraction = phase - next_segment as f32;
                        let t = fraction / frequency;
                        this_sample += this_blep_sample(t) * discontinuity;
                        next_sample += next_blep_sample(t) * discontinuity;
                    }
                }
                segment = next_segment;

                next_sample += (phase - 4.0) * saw_8_gain * 0.125;
                next_sample += (phase - (segment & 4) as f32 - 2.0) * saw_4_gain * 0.25;
                next_sample += (phase - (segment & 6) as f32 - 1.0) * saw_2_gain * 0.5;
                next_sample += (phase - (segment & 7) as f32 - 0.5) * saw_1_gain;
                *o += 2.0 * this_sample;
            }
        }
        self.next_sample = next_sample;
        self.phase = phase;
        self.segment = segment;
    }
}
