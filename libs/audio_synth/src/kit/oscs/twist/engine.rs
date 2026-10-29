// Rust port of the twist engine's engine interface and constants: dsp/dsp.h
// and dsp/engine/engine.h of the MIT eurorack macro-oscillator. Original
// notices:
//
// Copyright 2016 Emilie Gillet. (dsp/dsp.h, dsp/engine/engine.h)
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
//
// Differences from the original: engines allocate their own buffers in
// `new()` (no shared buffer allocator), user data loading is not ported (no
// user data exists in a patch), and the random generator is passed in.

use super::stm::{constrain, semitones_to_ratio, Random};

pub const SAMPLE_RATE: f32 = 48000.0;

/// The original corrects its frequency scale for the hardware's 47872.34 Hz
/// frame clock. The reference replaced it by the nominal rate.
const QUIRK_NOMINAL_RATE_FREQUENCY_SCALE: bool = true;
/// The hardware frame clock the original corrects for.
pub const CORRECTED_SAMPLE_RATE: f32 =
    if QUIRK_NOMINAL_RATE_FREQUENCY_SCALE { 48000.0 } else { 47872.34 };
/// Frequency of note 9 (A0 / 8), in cycles per sample.
pub const A0: f32 = (440.0 / 8.0) / CORRECTED_SAMPLE_RATE;

pub const MAX_BLOCK_SIZE: usize = 24;
pub const BLOCK_SIZE: usize = 12;

#[inline]
pub fn note_to_frequency(midi_note: f32) -> f32 {
    let n = constrain(midi_note - 9.0, -128.0, 127.0);
    A0 * 0.25 * semitones_to_ratio(n)
}

pub const TRIGGER_LOW: i32 = 0;
pub const TRIGGER_RISING_EDGE: i32 = 1;
pub const TRIGGER_UNPATCHED: i32 = 2;
pub const TRIGGER_HIGH: i32 = 4;

#[derive(Clone, Copy, Debug, Default)]
pub struct EngineParameters {
    pub trigger: i32,
    pub note: f32,
    pub timbre: f32,
    pub morph: f32,
    pub harmonics: f32,
    pub accent: f32,
}

/// One synthesis model. `new()` of each engine is the original's `Init`.
pub trait Engine: Send {
    fn reset(&mut self);
    /// Renders `out.len()` samples (<= MAX_BLOCK_SIZE) into `out` and `aux`.
    /// `already_enveloped` comes in as the engine's registered default; an
    /// engine may change it per call.
    fn render(
        &mut self,
        p: &EngineParameters,
        out: &mut [f32],
        aux: &mut [f32],
        already_enveloped: &mut bool,
        rng: &mut Random,
    );
}
