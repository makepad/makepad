// Rust port of the twist engine's chord model (engine 6):
// dsp/engine/chord_engine.h + .cc of the MIT eurorack macro-oscillator.
// Original notices:
//
// Copyright 2016 Emilie Gillet. (dsp/engine/chord_engine.h, .cc)
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


use super::chord_bank::{ChordBank, CHORD_NUM_VOICES};
use super::engine::*;
use super::osc_string_synth::StringSynthOscillator;
use super::osc_wavetable::WavetableOscillator;
use super::stm::*;
use super::tables::{tables, INTEGRATED_WAVE_SIZE};

const CHORD_NUM_HARMONICS: usize = 3;

const FADE_POINT: [f32; CHORD_NUM_VOICES] = [0.55, 0.47, 0.49, 0.51, 0.53];

const REGISTRATION_TABLE_SIZE: usize = 8;
const REGISTRATIONS: [[f32; CHORD_NUM_HARMONICS * 2]; REGISTRATION_TABLE_SIZE] = [
    [0.0, 1.0, 0.0, 0.0, 0.0, 0.0],    // Square
    [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],    // Saw
    [0.5, 0.0, 0.5, 0.0, 0.0, 0.0],    // Saw + saw
    [0.33, 0.0, 0.33, 0.0, 0.33, 0.0], // Full saw
    [0.33, 0.0, 0.0, 0.33, 0.0, 0.33], // Full saw + square hybrid
    [0.5, 0.0, 0.0, 0.0, 0.0, 0.5],    // Saw + high square harmo
    [0.0, 0.5, 0.0, 0.0, 0.0, 0.5],    // Square + high square harmo
    [0.0, 0.1, 0.1, 0.0, 0.2, 0.6],    // Saw + square + high harmo
];

const fn wave(bank: usize, row: usize, column: usize) -> usize {
    (bank * 64 + row * 8 + column) * INTEGRATED_WAVE_SIZE
}

const WAVETABLE: [usize; 15] = [
    wave(2, 6, 1),
    wave(2, 6, 6),
    wave(2, 6, 4),
    wave(0, 6, 0),
    wave(0, 6, 1),
    wave(0, 6, 2),
    wave(0, 6, 7),
    wave(2, 4, 7),
    wave(2, 4, 6),
    wave(2, 4, 5),
    wave(2, 4, 4),
    wave(2, 4, 3),
    wave(2, 4, 2),
    wave(2, 4, 1),
    wave(2, 4, 0),
];

/// Chords: wavetable and divide-down organ / string machine voices.
pub struct ChordEngine {
    divide_down_voice: [StringSynthOscillator; CHORD_NUM_VOICES],
    wavetable_voice: [WavetableOscillator<128, 15, true, true>; CHORD_NUM_VOICES],
    chords: ChordBank,
    morph_lp: f32,
    timbre_lp: f32,
}

impl ChordEngine {
    pub fn new() -> Self {
        let mut e = ChordEngine {
            divide_down_voice: [StringSynthOscillator::default(); CHORD_NUM_VOICES],
            wavetable_voice: [WavetableOscillator::default(); CHORD_NUM_VOICES],
            chords: ChordBank::default(),
            morph_lp: 0.0,
            timbre_lp: 0.0,
        };
        for i in 0..CHORD_NUM_VOICES {
            e.divide_down_voice[i].init();
            e.wavetable_voice[i].init();
        }
        e.chords.init();
        // The original computes the ratios in Reset (the voice always resets
        // before the first render); done here too so a fresh engine is valid.
        e.chords.reset();
        e
    }

    fn compute_registration(registration: f32, amplitudes: &mut [f32]) {
        let registration = registration * (REGISTRATION_TABLE_SIZE as f32 - 1.001);
        let (ri, rf) = integral_fractional(registration);
        let ri = ri as usize;
        for i in 0..CHORD_NUM_HARMONICS * 2 {
            let a = REGISTRATIONS[ri][i];
            let b = REGISTRATIONS[ri + 1][i];
            amplitudes[i] = a + (b - a) * rf;
        }
    }
}

impl Engine for ChordEngine {
    fn reset(&mut self) {
        self.chords.reset();
    }

    fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32], _already_enveloped: &mut bool, _rng: &mut Random) {
        let size = out.len();
        one_pole(&mut self.morph_lp, p.morph, 0.1);
        one_pole(&mut self.timbre_lp, p.timbre, 0.1);

        self.chords.set_chord(p.harmonics);

        let mut harmonics = [0.0f32; CHORD_NUM_HARMONICS * 2 + 2];
        let mut note_amplitudes = [0.0f32; CHORD_NUM_VOICES];
        let registration = (1.0 - self.morph_lp * 2.15).max(0.0);

        Self::compute_registration(registration, &mut harmonics);
        harmonics[CHORD_NUM_HARMONICS * 2] = 0.0;

        let mut ratios = [0.0f32; CHORD_NUM_VOICES];
        let aux_note_mask = self.chords.compute_chord_inversion(self.timbre_lp, &mut ratios, &mut note_amplitudes);

        out.fill(0.0);
        aux.fill(0.0);

        let f0 = note_to_frequency(p.note) * 0.998;
        let waveform = ((self.morph_lp - 0.535) * 2.15).max(0.0);
        let waves = &tables().integrated_waves[..];

        for note in 0..CHORD_NUM_VOICES {
            let wavetable_amount = constrain(50.0 * (self.morph_lp - FADE_POINT[note]), 0.0, 1.0);
            let mut divide_down_amount = 1.0 - wavetable_amount;
            let destination: &mut [f32] = if (1 << note) & aux_note_mask != 0 { &mut aux[..size] } else { &mut out[..size] };

            let note_f0 = f0 * ratios[note];
            let divide_down_gain = constrain(4.0 - note_f0 * 32.0, 0.0, 1.0);
            divide_down_amount *= divide_down_gain;

            if wavetable_amount != 0.0 {
                self.wavetable_voice[note].render(
                    note_f0 * 1.004,
                    note_amplitudes[note] * wavetable_amount,
                    waveform,
                    waves,
                    &WAVETABLE,
                    destination,
                );
            }
            if divide_down_amount != 0.0 {
                self.divide_down_voice[note].render(note_f0, &harmonics, note_amplitudes[note] * divide_down_amount, destination);
            }
        }

        for i in 0..size {
            out[i] += aux[i];
            aux[i] *= 3.0;
        }
    }
}
