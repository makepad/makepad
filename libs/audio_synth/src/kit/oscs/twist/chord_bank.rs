// Rust port of the twist engine's chord bank: dsp/chords/chord_bank.h + .cc of
// the MIT eurorack macro-oscillator (the default chord table; the alternative
// table behind a compile switch is not ported). Original notices:
//
// Copyright 2016 Emilie Gillet. (dsp/chords/chord_bank.h)
// Copyright 2021 Emilie Gillet. (dsp/chords/chord_bank.cc)
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

pub const CHORD_NUM_NOTES: usize = 4;
pub const CHORD_NUM_VOICES: usize = CHORD_NUM_NOTES + 1;
pub const CHORD_NUM_CHORDS: usize = 11;

const CHORDS: [[f32; CHORD_NUM_NOTES]; CHORD_NUM_CHORDS] = [
    [0.00, 0.01, 11.99, 12.00], // OCT
    [0.00, 7.00, 7.01, 12.00],  // 5
    [0.00, 5.00, 7.00, 12.00],  // sus4
    [0.00, 3.00, 7.00, 12.00],  // m
    [0.00, 3.00, 7.00, 10.00],  // m7
    [0.00, 3.00, 10.00, 14.00], // m9
    [0.00, 3.00, 10.00, 17.00], // m11
    [0.00, 2.00, 9.00, 16.00],  // 69
    [0.00, 4.00, 11.00, 14.00], // M9
    [0.00, 4.00, 7.00, 11.00],  // M7
    [0.00, 4.00, 7.00, 12.00],  // M
];

/// Chord bank shared by the chord models.
#[derive(Clone, Copy, Debug)]
pub struct ChordBank {
    chord_index_quantizer: HysteresisQuantizer2,
    ratios: [f32; CHORD_NUM_CHORDS * CHORD_NUM_NOTES],
    sorted_ratios: [f32; CHORD_NUM_NOTES],
    note_count: [i32; CHORD_NUM_CHORDS],
}

impl Default for ChordBank {
    fn default() -> Self {
        ChordBank {
            chord_index_quantizer: HysteresisQuantizer2::default(),
            ratios: [0.0; CHORD_NUM_CHORDS * CHORD_NUM_NOTES],
            sorted_ratios: [0.0; CHORD_NUM_NOTES],
            note_count: [0; CHORD_NUM_CHORDS],
        }
    }
}

impl ChordBank {
    pub fn init(&mut self) {
        self.chord_index_quantizer.init(CHORD_NUM_CHORDS as i32, 0.075, false);
    }

    pub fn reset(&mut self) {
        for i in 0..CHORD_NUM_CHORDS {
            let mut count = 0;
            for j in 0..CHORD_NUM_NOTES {
                let c = CHORDS[i][j];
                self.ratios[i * CHORD_NUM_NOTES + j] = semitones_to_ratio(c);
                if c != 0.01 && c != 7.01 && c != 11.99 && c != 12.00 {
                    count += 1;
                }
            }
            self.note_count[i] = count;
        }
        self.sort();
    }

    pub fn sort(&mut self) {
        for i in 0..CHORD_NUM_NOTES {
            let mut r = self.ratio(i);
            while r > 2.0 {
                r *= 0.5;
            }
            self.sorted_ratios[i] = r;
        }
        self.sorted_ratios.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    }

    #[inline]
    pub fn set_chord(&mut self, parameter: f32) {
        self.chord_index_quantizer.process(parameter * 1.02);
    }

    #[inline]
    pub fn chord_index(&self) -> usize {
        self.chord_index_quantizer.quantized_value() as usize
    }

    #[inline]
    pub fn ratios(&self) -> &[f32] {
        let i = self.chord_index() * CHORD_NUM_NOTES;
        &self.ratios[i..i + CHORD_NUM_NOTES]
    }

    #[inline]
    pub fn ratio(&self, note: usize) -> f32 {
        self.ratios[self.chord_index() * CHORD_NUM_NOTES + note]
    }

    #[allow(dead_code)]
    #[inline]
    pub fn sorted_ratio(&self, note: usize) -> f32 {
        self.sorted_ratios[note]
    }

    #[allow(dead_code)]
    #[inline]
    pub fn num_notes(&self) -> i32 {
        self.note_count[self.chord_index()]
    }

    /// Fills the 5 voices' ratios and amplitudes for an inversion; returns
    /// the mask of voices that carry the root (they go to AUX).
    pub fn compute_chord_inversion(&self, inversion: f32, ratios: &mut [f32], amplitudes: &mut [f32]) -> i32 {
        let base_ratio = self.ratios();
        let inversion = inversion * (CHORD_NUM_NOTES * CHORD_NUM_VOICES) as f32;
        let (inversion_integral, inversion_fractional) = integral_fractional(inversion);

        let n = CHORD_NUM_NOTES as i32;
        let v = CHORD_NUM_VOICES as i32;
        let num_rotations = inversion_integral / n;
        let rotated_note = inversion_integral % n;

        const BASE_GAIN: f32 = 0.25;
        let mut mask = 0;

        for i in 0..n {
            let transposition = 0.25 * (1i32 << ((n - 1 + inversion_integral - i) / n)) as f32;
            let target_voice = ((i - num_rotations + v) % v) as usize;
            let previous_voice = ((target_voice as i32 - 1 + v) % v) as usize;

            if i == rotated_note {
                ratios[target_voice] = base_ratio[i as usize] * transposition;
                ratios[previous_voice] = ratios[target_voice] * 2.0;
                amplitudes[previous_voice] = BASE_GAIN * inversion_fractional;
                amplitudes[target_voice] = BASE_GAIN * (1.0 - inversion_fractional);
            } else if i < rotated_note {
                ratios[previous_voice] = base_ratio[i as usize] * transposition;
                amplitudes[previous_voice] = BASE_GAIN;
            } else {
                ratios[target_voice] = base_ratio[i as usize] * transposition;
                amplitudes[target_voice] = BASE_GAIN;
            }

            if i == 0 {
                if i >= rotated_note {
                    mask |= 1 << target_voice;
                }
                if i <= rotated_note {
                    mask |= 1 << previous_voice;
                }
            }
        }
        mask
    }
}
