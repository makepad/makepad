// Rust port of the twist engine's speech model (engine 7):
// dsp/engine/speech_engine.h + .cc, dsp/speech/lpc_speech_synth.h + .cc,
// dsp/speech/lpc_speech_synth_controller.h + .cc, dsp/speech/naive_speech_synth.h + .cc
// and dsp/speech/sam_speech_synth.h + .cc of the MIT eurorack macro-oscillator
// (the frame and word data are in speech_phonemes.rs / speech_words.rs).
// Original notices:
//
// Copyright 2016 Emilie Gillet. (dsp/engine/speech_engine.h, dsp/engine/speech_engine.cc,
//                                dsp/speech/lpc_speech_synth.h, dsp/speech/lpc_speech_synth.cc,
//                                dsp/speech/lpc_speech_synth_controller.h,
//                                dsp/speech/lpc_speech_synth_controller.cc,
//                                dsp/speech/naive_speech_synth.h, dsp/speech/naive_speech_synth.cc,
//                                dsp/speech/sam_speech_synth.h, dsp/speech/sam_speech_synth.cc)
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
use super::osc_base::{sine_raw, Oscillator, OSCILLATOR_SHAPE_IMPULSE_TRAIN};
use super::speech_phonemes::PHONEMES;
/// No recorded word banks ship (the upstream ones were extracted from
/// third-party speech ROMs, so they are left out): the word range of the
/// speech engine plays the phoneme frames instead.
const NUM_WORD_BANKS: usize = 0;
static WORD_BANKS: [&[u8]; NUM_WORD_BANKS] = [];
use super::stm::*;
use super::tables::tables;

/// Reading the frame after the last one (blend 0, so it only must not be
/// NaN) runs past the table in the original; we clamp the index instead.
#[allow(dead_code)]
const QUIRK_PLAY_FRAME_READS_NEXT_FRAME: bool = true;

// --------------------------------------------------- lpc_speech_synth.h/cc

const LPC_ORDER: usize = 10;
const LPC_SPEECH_SYNTH_DEFAULT_F0: f32 = 100.0;
const LPC_EXCITATION_PULSE_SIZE: i32 = 640;

/// One LPC-10 frame (energy, pitch period, ten reflection coefficients).
#[derive(Clone, Copy, Debug, Default)]
pub struct LpcFrame {
    pub energy: u8,
    pub period: u8,
    pub k0: i16,
    pub k1: i16,
    pub k2: i8,
    pub k3: i8,
    pub k4: i8,
    pub k5: i8,
    pub k6: i8,
    pub k7: i8,
    pub k8: i8,
    pub k9: i8,
}

#[inline]
fn blend_coefficient(a: f32, b: f32, scale: f32, blend: f32) -> f32 {
    let a_f = a / scale;
    let b_f = b / scale;
    a_f + (b_f - a_f) * blend
}

/// LPC-10 speech synth.
#[derive(Clone, Copy, Debug, Default)]
pub struct LpcSpeechSynth {
    phase: f32,
    frequency: f32,
    noise_energy: f32,
    pulse_energy: f32,
    next_sample: f32,
    excitation_pulse_sample_index: i32,
    k: [f32; LPC_ORDER],
    s: [f32; LPC_ORDER + 1],
}

impl LpcSpeechSynth {
    pub fn init(&mut self) {
        self.phase = 0.0;
        self.frequency = 0.0125;
        self.noise_energy = 0.0;
        self.pulse_energy = 0.0;
        self.next_sample = 0.0;
        self.excitation_pulse_sample_index = 0;
        self.k = [0.0; LPC_ORDER];
        self.s = [0.0; LPC_ORDER + 1];
    }

    pub fn render(
        &mut self,
        prosody_amount: f32,
        pitch_shift: f32,
        excitation: &mut [f32],
        output: &mut [f32],
        rng: &mut Random,
    ) {
        let pulse = &tables().lpc_excitation_pulse;
        let base_f0 = LPC_SPEECH_SYNTH_DEFAULT_F0 / 8000.0;
        let d = self.frequency - base_f0;
        let f = constrain((base_f0 + d * prosody_amount) * pitch_shift, 0.0, 0.5);
        let mut next_sample = self.next_sample;
        let k = self.k;
        for i in 0..excitation.len() {
            self.phase += f;
            let mut this_sample = next_sample;
            next_sample = 0.0;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
                let reset_time = self.phase / f;
                let reset_sample = (32.0 * reset_time) as i32;
                let mut discontinuity = 0.0;
                if self.excitation_pulse_sample_index < LPC_EXCITATION_PULSE_SIZE {
                    self.excitation_pulse_sample_index -= reset_sample;
                    let s = pulse[self.excitation_pulse_sample_index.max(0) as usize];
                    discontinuity = s as f32 / 128.0 * self.pulse_energy;
                }
                this_sample += -discontinuity * this_blep_sample(reset_time);
                next_sample += -discontinuity * next_blep_sample(reset_time);
                self.excitation_pulse_sample_index = reset_sample;
            }
            let mut e = [0.0f32; 11];
            e[10] = if rng.get_sample() > 0 { self.noise_energy } else { -self.noise_energy };
            if self.excitation_pulse_sample_index < LPC_EXCITATION_PULSE_SIZE {
                let s = pulse[self.excitation_pulse_sample_index as usize];
                next_sample += s as f32 / 128.0 * self.pulse_energy;
                self.excitation_pulse_sample_index += 32;
            }
            e[10] += this_sample;
            e[10] *= 1.5;
            for j in (0..10).rev() {
                e[j] = e[j + 1] - k[j] * self.s[j];
            }
            e[0] = constrain(e[0], -2.0, 2.0);
            for j in (1..10).rev() {
                self.s[j] = self.s[j - 1] + k[j - 1] * e[j - 1];
            }
            self.s[0] = e[0];
            excitation[i] = e[10];
            output[i] = e[0];
        }
        self.next_sample = next_sample;
    }

    /// `PlayFrame(frames, frame, interpolate)`.
    pub fn play_frame_at(&mut self, frames: &[LpcFrame], frame: f32, interpolate: bool) {
        let (fi, mut ff) = integral_fractional(frame);
        if !interpolate {
            ff = 0.0;
        }
        let i = fi as usize;
        // QUIRK_PLAY_FRAME_READS_NEXT_FRAME: index clamped (blend is 0 there).
        let j = (i + 1).min(frames.len() - 1);
        self.play_frame(frames[i], frames[j], ff);
    }

    fn play_frame(&mut self, f1: LpcFrame, f2: LpcFrame, blend: f32) {
        let frequency_1 = if f1.period == 0 { self.frequency } else { 1.0 / f1.period as f32 };
        let frequency_2 = if f2.period == 0 { self.frequency } else { 1.0 / f2.period as f32 };
        self.frequency = frequency_1 + (frequency_2 - frequency_1) * blend;

        let energy_1 = f1.energy as f32 / 256.0;
        let energy_2 = f2.energy as f32 / 256.0;
        let noise_energy_1 = if f1.period == 0 { energy_1 } else { 0.0 };
        let noise_energy_2 = if f2.period == 0 { energy_2 } else { 0.0 };
        self.noise_energy = noise_energy_1 + (noise_energy_2 - noise_energy_1) * blend;

        let pulse_energy_1 = if f1.period != 0 { energy_1 } else { 0.0 };
        let pulse_energy_2 = if f2.period != 0 { energy_2 } else { 0.0 };
        self.pulse_energy = pulse_energy_1 + (pulse_energy_2 - pulse_energy_1) * blend;

        self.k[0] = blend_coefficient(f1.k0 as f32, f2.k0 as f32, 32768.0, blend);
        self.k[1] = blend_coefficient(f1.k1 as f32, f2.k1 as f32, 32768.0, blend);
        self.k[2] = blend_coefficient(f1.k2 as f32, f2.k2 as f32, 128.0, blend);
        self.k[3] = blend_coefficient(f1.k3 as f32, f2.k3 as f32, 128.0, blend);
        self.k[4] = blend_coefficient(f1.k4 as f32, f2.k4 as f32, 128.0, blend);
        self.k[5] = blend_coefficient(f1.k5 as f32, f2.k5 as f32, 128.0, blend);
        self.k[6] = blend_coefficient(f1.k6 as f32, f2.k6 as f32, 128.0, blend);
        self.k[7] = blend_coefficient(f1.k7 as f32, f2.k7 as f32, 128.0, blend);
        self.k[8] = blend_coefficient(f1.k8 as f32, f2.k8 as f32, 128.0, blend);
        self.k[9] = blend_coefficient(f1.k9 as f32, f2.k9 as f32, 128.0, blend);
    }
}

// ------------------------------------- lpc_speech_synth_controller.h/cc

struct BitStream<'a> {
    data: &'a [u8],
    p: usize,
    available: i32,
    bits: u16,
}

impl<'a> BitStream<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitStream { data, p: 0, available: 0, bits: 0 }
    }
    fn flush(&mut self) {
        while self.available != 0 {
            self.get_bits(1);
        }
    }
    fn get_bits(&mut self, num_bits: i32) -> u8 {
        let mut shift = num_bits;
        if num_bits > self.available {
            self.bits <<= self.available;
            shift -= self.available;
            self.bits |= self.data[self.p].reverse_bits() as u16;
            self.p += 1;
            self.available += 8;
        }
        self.bits <<= shift;
        let result = (self.bits >> 8) as u8;
        self.bits &= 0xff;
        self.available -= num_bits;
        result
    }
}

const LPC_SPEECH_SYNTH_MAX_WORDS: usize = 32;
const LPC_SPEECH_SYNTH_MAX_FRAMES: usize = 1024;
const LPC_SPEECH_SYNTH_NUM_VOWELS: i32 = 5;
const LPC_SPEECH_SYNTH_NUM_CONSONANTS: i32 = 10;
const LPC_SPEECH_SYNTH_FPS: f32 = 40.0;

const ENERGY_LUT: [u8; 16] = [0x00, 0x02, 0x03, 0x04, 0x05, 0x07, 0x0a, 0x0f, 0x14, 0x20, 0x29, 0x39, 0x51, 0x72, 0xa1, 0xff];
const PERIOD_LUT: [u8; 64] = [
    0, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 45, 47,
    49, 51, 53, 54, 57, 59, 61, 63, 66, 69, 71, 73, 77, 79, 81, 85, 87, 92, 95, 99, 102, 106, 110, 115, 119, 123, 128, 133,
    138, 143, 149, 154, 160,
];
const K0_LUT: [i16; 32] = [
    -32064, -31872, -31808, -31680, -31552, -31424, -31232, -30848, -30592, -30336, -30016, -29696, -29376, -28928, -28480,
    -27968, -26368, -24256, -21632, -18368, -14528, -10048, -5184, 0, 5184, 10048, 14528, 18368, 21632, 24256, 26368, 27968,
];
const K1_LUT: [i16; 32] = [
    -20992, -19328, -17536, -15552, -13440, -11200, -8768, -6272, -3712, -1088, 1536, 4160, 6720, 9216, 11584, 13824, 15936,
    17856, 19648, 21248, 22656, 24000, 25152, 26176, 27072, 27840, 28544, 29120, 29632, 30080, 30464, 32384,
];
const K2_LUT: [i8; 16] = [-110, -97, -83, -70, -56, -43, -29, -16, -2, 11, 25, 38, 52, 65, 79, 92];
const K3_LUT: [i8; 16] = [-82, -68, -54, -40, -26, -12, 1, 15, 29, 43, 57, 71, 85, 99, 113, 126];
const K4_LUT: [i8; 16] = [-82, -70, -59, -47, -35, -24, -12, -1, 11, 23, 34, 46, 57, 69, 81, 92];
const K5_LUT: [i8; 16] = [-64, -53, -42, -31, -20, -9, 3, 14, 25, 36, 47, 58, 69, 80, 91, 102];
const K6_LUT: [i8; 16] = [-77, -65, -53, -41, -29, -17, -5, 7, 19, 31, 43, 55, 67, 79, 90, 102];
const K7_LUT: [i8; 8] = [-64, -40, -16, 7, 31, 55, 79, 102];
const K8_LUT: [i8; 8] = [-64, -44, -24, -4, 16, 37, 57, 77];
const K9_LUT: [i8; 8] = [-51, -33, -15, 4, 22, 32, 59, 77];

/// Decodes one word bank into frames and word boundaries.
pub struct LpcSpeechSynthWordBank {
    loaded_bank: i32,
    num_frames: usize,
    num_words: usize,
    /// One more entry than the original's buffer: the end boundary of a
    /// full 32-word bank lands there instead of past the buffer.
    word_boundaries: [i32; LPC_SPEECH_SYNTH_MAX_WORDS + 1],
    frames: Box<[LpcFrame]>,
}

impl LpcSpeechSynthWordBank {
    fn new() -> Self {
        let mut b = LpcSpeechSynthWordBank {
            loaded_bank: -1,
            num_frames: 0,
            num_words: 0,
            word_boundaries: [0; LPC_SPEECH_SYNTH_MAX_WORDS + 1],
            frames: vec![LpcFrame::default(); LPC_SPEECH_SYNTH_MAX_FRAMES].into_boxed_slice(),
        };
        b.reset();
        b
    }

    pub fn reset(&mut self) {
        self.loaded_bank = -1;
        self.num_frames = 0;
        self.num_words = 0;
        self.word_boundaries = [0; LPC_SPEECH_SYNTH_MAX_WORDS + 1];
    }

    fn load_next_word(&mut self, data: &[u8]) -> usize {
        let mut bs = BitStream::new(data);
        let mut frame = LpcFrame::default();
        loop {
            let energy = bs.get_bits(4) as usize;
            if energy == 0 {
                frame.energy = 0;
            } else if energy == 0xf {
                bs.flush();
                break;
            } else {
                frame.energy = ENERGY_LUT[energy];
                let repeat = bs.get_bits(1) != 0;
                frame.period = PERIOD_LUT[bs.get_bits(6) as usize];
                if !repeat {
                    frame.k0 = K0_LUT[bs.get_bits(5) as usize];
                    frame.k1 = K1_LUT[bs.get_bits(5) as usize];
                    frame.k2 = K2_LUT[bs.get_bits(4) as usize];
                    frame.k3 = K3_LUT[bs.get_bits(4) as usize];
                    if frame.period != 0 {
                        frame.k4 = K4_LUT[bs.get_bits(4) as usize];
                        frame.k5 = K5_LUT[bs.get_bits(4) as usize];
                        frame.k6 = K6_LUT[bs.get_bits(4) as usize];
                        frame.k7 = K7_LUT[bs.get_bits(3) as usize];
                        frame.k8 = K8_LUT[bs.get_bits(3) as usize];
                        frame.k9 = K9_LUT[bs.get_bits(3) as usize];
                    }
                }
            }
            if self.num_frames < LPC_SPEECH_SYNTH_MAX_FRAMES {
                self.frames[self.num_frames] = frame;
                self.num_frames += 1;
            }
        }
        bs.p
    }

    /// Loads bank `bank`; true when it changed.
    pub fn load(&mut self, bank: i32) -> bool {
        if bank == self.loaded_bank || bank >= NUM_WORD_BANKS as i32 {
            return false;
        }
        self.num_frames = 0;
        self.num_words = 0;
        let data = WORD_BANKS[bank as usize];
        let mut p = 0;
        while p < data.len() && self.num_words < LPC_SPEECH_SYNTH_MAX_WORDS {
            self.word_boundaries[self.num_words] = self.num_frames as i32;
            p += self.load_next_word(&data[p..]);
            self.num_words += 1;
        }
        self.word_boundaries[self.num_words] = self.num_frames as i32;
        self.loaded_bank = bank;
        true
    }

    pub fn num_frames(&self) -> usize {
        self.num_frames
    }

    pub fn frames(&self) -> &[LpcFrame] {
        &self.frames
    }

    /// (start, end) frames of the word at `address` in [0, 1].
    pub fn get_word_boundaries(&self, address: f32) -> (i32, i32) {
        if self.num_words == 0 {
            (-1, -1)
        } else {
            let mut word = (address * self.num_words as f32) as i32;
            if word >= self.num_words as i32 {
                word = self.num_words as i32 - 1;
            }
            let w = word.max(0) as usize;
            (self.word_boundaries[w], self.word_boundaries[w + 1] - 1)
        }
    }
}

/// Feeds frames (vowels, consonants or words) to the LPC-10 synth.
pub struct LpcSpeechSynthController {
    clock_phase: f32,
    sample: [f32; 2],
    next_sample: [f32; 2],
    gain: f32,
    synth: LpcSpeechSynth,
    playback_frame: i32,
    last_playback_frame: i32,
    remaining_frame_samples: usize,
}

impl LpcSpeechSynthController {
    fn new() -> Self {
        let mut c = LpcSpeechSynthController {
            clock_phase: 0.0,
            sample: [0.0; 2],
            next_sample: [0.0; 2],
            gain: 0.0,
            synth: LpcSpeechSynth::default(),
            playback_frame: -1,
            last_playback_frame: -1,
            remaining_frame_samples: 0,
        };
        c.synth.init();
        c
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        word_bank: &mut LpcSpeechSynthWordBank,
        free_running: bool,
        trigger: bool,
        bank: i32,
        frequency: f32,
        prosody_amount: f32,
        speed: f32,
        address: f32,
        formant_shift: f32,
        gain: f32,
        excitation: &mut [f32],
        output: &mut [f32],
        rng: &mut Random,
    ) {
        let size = excitation.len();
        let rate_ratio = semitones_to_ratio((formant_shift - 0.5) * 36.0);
        let rate = rate_ratio / 6.0;
        // All utterances are normalised for an average f0 of 100 Hz.
        let pitch_shift = frequency / (rate_ratio * LPC_SPEECH_SYNTH_DEFAULT_F0 / CORRECTED_SAMPLE_RATE);
        let time_stretch = semitones_to_ratio(
            -speed * 24.0
                + if formant_shift < 0.4 {
                    (formant_shift - 0.4) * -45.0
                } else if formant_shift > 0.6 {
                    (formant_shift - 0.6) * -45.0
                } else {
                    0.0
                },
        );
        if bank != -1 {
            let reset_everything = word_bank.load(bank);
            if reset_everything {
                self.playback_frame = -1;
                self.last_playback_frame = -1;
            }
        }
        let num_frames = if bank == -1 { LPC_SPEECH_SYNTH_NUM_VOWELS as usize } else { word_bank.num_frames() };
        let frames: &[LpcFrame] = if bank == -1 { &PHONEMES } else { word_bank.frames() };

        if trigger {
            if bank == -1 {
                // A pseudo-random consonant, played for one frame.
                let r = ((address + 3.0 * formant_shift + 7.0 * frequency) * 8.0) as i32;
                self.playback_frame = r % LPC_SPEECH_SYNTH_NUM_CONSONANTS;
                self.playback_frame += LPC_SPEECH_SYNTH_NUM_VOWELS;
                self.last_playback_frame = self.playback_frame + 1;
            } else {
                let (s, e) = word_bank.get_word_boundaries(address);
                self.playback_frame = s;
                self.last_playback_frame = e;
            }
            self.remaining_frame_samples = 0;
        }

        if self.playback_frame == -1 && self.remaining_frame_samples == 0 {
            self.synth.play_frame_at(frames, address * (num_frames as f32 - 1.0001), true);
        } else {
            if self.remaining_frame_samples == 0 {
                self.synth.play_frame_at(frames, self.playback_frame.max(0) as f32, false);
                self.remaining_frame_samples = (SAMPLE_RATE / LPC_SPEECH_SYNTH_FPS * time_stretch) as usize;
                self.playback_frame += 1;
                if self.playback_frame >= self.last_playback_frame {
                    let back_to_scan_mode = bank == -1 || free_running;
                    self.playback_frame = if back_to_scan_mode { -1 } else { self.last_playback_frame };
                }
            }
            self.remaining_frame_samples -= size.min(self.remaining_frame_samples);
        }

        let mut gm = ParameterInterpolator::new(&mut self.gain, gain, size);
        for i in 0..size {
            let mut this_sample = self.next_sample;
            self.next_sample = [0.0; 2];
            self.clock_phase += rate;
            if self.clock_phase >= 1.0 {
                self.clock_phase -= 1.0;
                let reset_time = self.clock_phase / rate;
                let mut ns = [0.0f32; 1];
                let mut no = [0.0f32; 1];
                self.synth.render(prosody_amount, pitch_shift, &mut ns, &mut no, rng);
                let new_sample = [ns[0], no[0]];
                for c in 0..2 {
                    let discontinuity = new_sample[c] - self.sample[c];
                    this_sample[c] += discontinuity * this_blep_sample(reset_time);
                    self.next_sample[c] += discontinuity * next_blep_sample(reset_time);
                }
                self.sample = new_sample;
            }
            self.next_sample[0] += self.sample[0];
            self.next_sample[1] += self.sample[1];
            let g = gm.next();
            excitation[i] = this_sample[0] * g;
            output[i] = this_sample[1] * g;
        }
    }
}

// ----------------------------------------------- naive_speech_synth.h/cc

const NAIVE_SPEECH_NUM_FORMANTS: usize = 5;
const NAIVE_SPEECH_NUM_PHONEMES: usize = 5;
const NAIVE_SPEECH_NUM_REGISTERS: usize = 5;

/// (frequency, amplitude) of the five formants, per phoneme and register.
const NAIVE_PHONEMES: [[[(u8, u8); NAIVE_SPEECH_NUM_FORMANTS]; NAIVE_SPEECH_NUM_REGISTERS]; NAIVE_SPEECH_NUM_PHONEMES] = [
    [
        [(74, 255), (83, 114), (97, 90), (98, 90), (100, 25)],
        [(75, 255), (84, 128), (100, 114), (101, 101), (103, 20)],
        [(76, 255), (85, 128), (100, 18), (102, 16), (104, 3)],
        [(79, 255), (85, 161), (101, 25), (104, 4), (110, 0)],
        [(79, 255), (85, 128), (101, 6), (106, 25), (110, 0)],
    ],
    [
        [(67, 255), (91, 64), (98, 90), (101, 64), (102, 32)],
        [(67, 255), (92, 51), (99, 64), (103, 51), (105, 25)],
        [(69, 255), (93, 51), (100, 32), (102, 25), (103, 25)],
        [(67, 255), (91, 16), (100, 8), (103, 4), (110, 0)],
        [(65, 255), (95, 25), (101, 45), (105, 2), (110, 0)],
    ],
    [
        [(59, 255), (92, 8), (99, 40), (102, 20), (104, 10)],
        [(61, 255), (94, 45), (101, 32), (103, 25), (105, 8)],
        [(60, 255), (93, 16), (101, 16), (104, 4), (105, 4)],
        [(65, 255), (92, 25), (100, 8), (105, 4), (110, 0)],
        [(60, 255), (96, 64), (101, 12), (106, 12), (110, 1)],
    ],
    [
        [(67, 255), (78, 72), (98, 22), (99, 25), (101, 2)],
        [(67, 255), (79, 80), (99, 64), (101, 64), (102, 12)],
        [(68, 255), (79, 80), (100, 12), (102, 20), (103, 5)],
        [(69, 255), (79, 90), (101, 40), (104, 10), (110, 0)],
        [(69, 255), (79, 72), (101, 20), (106, 20), (110, 0)],
    ],
    [
        [(65, 255), (74, 25), (98, 6), (100, 10), (101, 4)],
        [(65, 255), (74, 25), (100, 36), (101, 51), (103, 12)],
        [(66, 255), (75, 25), (100, 18), (102, 8), (104, 5)],
        [(63, 255), (77, 64), (99, 8), (104, 2), (110, 0)],
        [(63, 255), (77, 40), (100, 4), (106, 2), (110, 0)],
    ],
];

/// Speech from synthesizer building blocks (pulse oscillator + SVFs).
#[derive(Clone, Copy, Debug, Default)]
pub struct NaiveSpeechSynth {
    pulse: Oscillator,
    click_duration: usize,
    filter: [Svf; NAIVE_SPEECH_NUM_FORMANTS],
    pulse_coloration: Svf,
}

impl NaiveSpeechSynth {
    pub fn init(&mut self) {
        self.pulse.init();
        self.click_duration = 0;
        for f in self.filter.iter_mut() {
            f.init();
        }
        self.pulse_coloration.init();
        self.pulse_coloration.set_f_q::<FREQUENCY_DIRTY>(800.0 / SAMPLE_RATE, 0.5);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        click: bool,
        mut frequency: f32,
        phoneme: f32,
        vocal_register: f32,
        excitation: &mut [f32],
        output: &mut [f32],
    ) {
        let size = excitation.len();
        if click {
            self.click_duration = (SAMPLE_RATE * 0.05) as usize;
        }
        self.click_duration -= self.click_duration.min(size);
        if self.click_duration != 0 {
            frequency *= 0.5;
        }
        // Glottal pulse.
        self.pulse.render::<OSCILLATOR_SHAPE_IMPULSE_TRAIN>(frequency, 0.5, excitation);
        self.pulse_coloration.process_in_place::<FILTER_MODE_BAND_PASS>(excitation);
        for x in excitation.iter_mut() {
            *x *= 4.0;
        }

        let (pi, pf) = integral_fractional(phoneme * (NAIVE_SPEECH_NUM_PHONEMES as f32 - 1.001));
        let (ri, rf) = integral_fractional(vocal_register * (NAIVE_SPEECH_NUM_REGISTERS as f32 - 1.001));
        let (pi, ri) = (pi as usize, ri as usize);
        output.fill(0.0);
        for i in 0..NAIVE_SPEECH_NUM_FORMANTS {
            let p0r0 = NAIVE_PHONEMES[pi][ri][i];
            let p0r1 = NAIVE_PHONEMES[pi][ri + 1][i];
            let p1r0 = NAIVE_PHONEMES[pi + 1][ri][i];
            let p1r1 = NAIVE_PHONEMES[pi + 1][ri + 1][i];
            let p0r_f = p0r0.0 as f32 + (p0r1.0 as f32 - p0r0.0 as f32) * rf;
            let p1r_f = p1r0.0 as f32 + (p1r1.0 as f32 - p1r0.0 as f32) * rf;
            let mut f = p0r_f + (p1r_f - p0r_f) * pf;
            let p0r_a = p0r0.1 as f32 + (p0r1.1 as f32 - p0r0.1 as f32) * rf;
            let p1r_a = p1r0.1 as f32 + (p1r1.1 as f32 - p1r0.1 as f32) * rf;
            let a = (p0r_a + (p1r_a - p0r_a) * pf) / 256.0;
            if f >= 160.0 {
                f = 160.0;
            }
            f = A0 * semitones_to_ratio(f - 33.0);
            if self.click_duration != 0 && i == 0 {
                f *= 0.5;
            }
            self.filter[i].set_f_q::<FREQUENCY_DIRTY>(f, 20.0);
            self.filter[i].process_add::<FILTER_MODE_BAND_PASS>(excitation, output, a);
        }
    }
}

// ------------------------------------------------- sam_speech_synth.h/cc

const SAM_NUM_FORMANTS: usize = 3;
const SAM_NUM_VOWELS: usize = 9;
const SAM_NUM_CONSONANTS: i32 = 8;

/// (frequency, amplitude index) of three formants; the last row is a guard.
const SAM_PHONEMES: [[(u8, u8); SAM_NUM_FORMANTS]; 18] = [
    [(60, 15), (90, 13), (200, 1)],
    [(40, 13), (114, 12), (139, 6)],
    [(33, 14), (155, 12), (209, 7)],
    [(22, 13), (189, 10), (247, 8)],
    [(51, 15), (99, 12), (195, 1)],
    [(29, 13), (65, 8), (180, 0)],
    [(13, 12), (103, 3), (182, 0)],
    [(20, 15), (114, 3), (213, 0)],
    [(13, 7), (164, 3), (222, 14)],
    [(13, 9), (121, 9), (254, 0)],
    [(40, 12), (112, 10), (114, 5)],
    [(24, 13), (54, 8), (157, 0)],
    [(33, 14), (155, 12), (166, 7)],
    [(36, 14), (83, 8), (249, 1)],
    [(40, 14), (114, 12), (139, 6)],
    [(13, 5), (58, 5), (182, 5)],
    [(13, 7), (164, 10), (222, 14)],
    [(13, 7), (164, 10), (222, 14)],
];

const SAM_FORMANT_AMPLITUDE_LUT: [f32; 16] = [
    0.03125000, 0.03756299, 0.04515131, 0.05427259, 0.06523652, 0.07841532, 0.09425646, 0.11329776, 0.13618570,
    0.16369736, 0.19676682, 0.23651683, 0.28429697, 0.34172946, 0.41076422, 0.49374509,
];

/// Formant speech in the style of the classic 8-bit speech synth.
#[derive(Clone, Copy, Debug, Default)]
pub struct SamSpeechSynth {
    phase: f32,
    frequency: f32,
    pulse_next_sample: f32,
    pulse_lp: f32,
    formant_phase: [u32; 3],
    consonant_samples: usize,
    consonant_index: f32,
}

impl SamSpeechSynth {
    pub fn init(&mut self) {
        *self = SamSpeechSynth::default();
    }

    fn interpolate_phoneme_data(phoneme: f32, formant_shift: f32) -> ([u32; 3], [f32; 3]) {
        let (pi, pf) = integral_fractional(phoneme);
        let p_1 = SAM_PHONEMES[pi as usize];
        let p_2 = SAM_PHONEMES[pi as usize + 1];
        let formant_shift = 1.0 + formant_shift * 2.5;
        let mut freq = [0u32; 3];
        let mut amp = [0.0f32; 3];
        for i in 0..SAM_NUM_FORMANTS {
            let f_1 = p_1[i].0 as f32;
            let f_2 = p_2[i].0 as f32;
            let mut f = f_1 + (f_2 - f_1) * pf;
            f *= 8.0 * formant_shift * 4294967296.0 / SAMPLE_RATE;
            freq[i] = f as u32;
            let a_1 = SAM_FORMANT_AMPLITUDE_LUT[p_1[i].1 as usize];
            let a_2 = SAM_FORMANT_AMPLITUDE_LUT[p_2[i].1 as usize];
            amp[i] = a_1 + (a_2 - a_1) * pf;
        }
        (freq, amp)
    }

    pub fn render(
        &mut self,
        consonant: bool,
        mut frequency: f32,
        vowel: f32,
        formant_shift: f32,
        excitation: &mut [f32],
        output: &mut [f32],
    ) {
        let size = excitation.len();
        if frequency >= 0.0625 {
            frequency = 0.0625;
        }
        if consonant {
            self.consonant_samples = (SAMPLE_RATE * 0.05) as usize;
            let r = ((vowel + 3.0 * frequency + 7.0 * formant_shift) * 8.0) as i32;
            self.consonant_index = (r % SAM_NUM_CONSONANTS) as f32;
        }
        self.consonant_samples -= self.consonant_samples.min(size);
        let phoneme = if self.consonant_samples != 0 {
            self.consonant_index + SAM_NUM_VOWELS as f32
        } else {
            vowel * (SAM_NUM_VOWELS as f32 - 1.0001)
        };
        let (ff, fa) = Self::interpolate_phoneme_data(phoneme, formant_shift);

        let mut phase = self.phase;
        let mut fp = self.formant_phase;
        let mut pulse_lp = self.pulse_lp;
        let mut pulse_next_sample = self.pulse_next_sample;
        {
            let mut fm = ParameterInterpolator::new(&mut self.frequency, frequency, size);
            for i in 0..size {
                let mut pulse_this_sample = pulse_next_sample;
                pulse_next_sample = 0.0;
                let frequency = fm.next();
                phase += frequency;
                if phase >= 1.0 {
                    phase -= 1.0;
                    let t = phase / frequency;
                    for j in 0..3 {
                        fp[j] = (t * ff[j] as f32) as u32;
                    }
                    pulse_this_sample -= this_blep_sample(t);
                    pulse_next_sample -= next_blep_sample(t);
                } else {
                    for j in 0..3 {
                        fp[j] = fp[j].wrapping_add(ff[j]);
                    }
                }
                pulse_next_sample += phase;
                let d = pulse_this_sample - 0.5 - pulse_lp;
                pulse_lp += (16.0 * frequency).min(1.0) * d;
                excitation[i] = d;
                let mut s = 0.0;
                s += sine_raw(fp[0]) * fa[0];
                s += sine_raw(fp[1]) * fa[1];
                s += sine_raw(fp[2]) * fa[2];
                s *= 1.0 - phase;
                output[i] = s;
            }
        }
        self.phase = phase;
        self.formant_phase = fp;
        self.pulse_lp = pulse_lp;
        self.pulse_next_sample = pulse_next_sample;
    }
}

// ----------------------------------------------------------- speech_engine

/// Various flavours of speech synthesis: naive formants, 8-bit formants,
/// LPC-10 vowels, LPC-10 word banks.
pub struct SpeechEngine {
    word_bank_quantizer: HysteresisQuantizer2,
    naive_speech_synth: NaiveSpeechSynth,
    sam_speech_synth: SamSpeechSynth,
    lpc_speech_synth_controller: LpcSpeechSynthController,
    lpc_speech_synth_word_bank: LpcSpeechSynthWordBank,
    prosody_amount: f32,
    speed: f32,
}

impl SpeechEngine {
    pub fn new() -> Self {
        let mut e = SpeechEngine {
            word_bank_quantizer: HysteresisQuantizer2::default(),
            naive_speech_synth: NaiveSpeechSynth::default(),
            sam_speech_synth: SamSpeechSynth::default(),
            lpc_speech_synth_controller: LpcSpeechSynthController::new(),
            lpc_speech_synth_word_bank: LpcSpeechSynthWordBank::new(),
            prosody_amount: 0.0,
            speed: 0.0,
        };
        e.sam_speech_synth.init();
        e.naive_speech_synth.init();
        e.word_bank_quantizer.init(NUM_WORD_BANKS as i32 + 1, 0.1, false);
        e
    }

    pub fn set_prosody_amount(&mut self, v: f32) {
        self.prosody_amount = v;
    }

    pub fn set_speed(&mut self, v: f32) {
        self.speed = v;
    }
}

impl Engine for SpeechEngine {
    fn reset(&mut self) {
        self.lpc_speech_synth_word_bank.reset();
    }

    fn render(
        &mut self,
        p: &EngineParameters,
        out: &mut [f32],
        aux: &mut [f32],
        already_enveloped: &mut bool,
        rng: &mut Random,
    ) {
        let size = out.len();
        let f0 = note_to_frequency(p.note);
        let group = p.harmonics * 6.0;
        let free_running = p.trigger & TRIGGER_UNPATCHED != 0;
        let rising = p.trigger & TRIGGER_RISING_EDGE != 0;

        // Interpolates between the three models: naive, 8-bit formant, LPC.
        if group <= 2.0 {
            *already_enveloped = false;
            let mut blend = group;
            if group <= 1.0 {
                self.naive_speech_synth.render(p.trigger == TRIGGER_RISING_EDGE, f0, p.morph, p.timbre, aux, out);
            } else {
                self.lpc_speech_synth_controller.render(
                    &mut self.lpc_speech_synth_word_bank,
                    free_running,
                    rising,
                    -1,
                    f0,
                    0.0,
                    0.0,
                    p.morph,
                    p.timbre,
                    1.0,
                    aux,
                    out,
                    rng,
                );
                blend = 2.0 - blend;
            }
            let mut t0 = [0.0f32; MAX_BLOCK_SIZE];
            let mut t1 = [0.0f32; MAX_BLOCK_SIZE];
            self.sam_speech_synth.render(
                p.trigger == TRIGGER_RISING_EDGE,
                f0,
                p.morph,
                p.timbre,
                &mut t0[..size],
                &mut t1[..size],
            );
            blend *= blend * (3.0 - 2.0 * blend);
            blend *= blend * (3.0 - 2.0 * blend);
            for i in 0..size {
                aux[i] += (t0[i] - aux[i]) * blend;
                out[i] += (t1[i] - out[i]) * blend;
            }
        } else {
            // Words / phonemes for the LPC synth.
            let word_bank = self.word_bank_quantizer.process((group - 2.0) * 0.275) - 1;
            let replay_prosody = word_bank >= 0 && !free_running;
            *already_enveloped = replay_prosody;
            self.lpc_speech_synth_controller.render(
                &mut self.lpc_speech_synth_word_bank,
                free_running,
                rising,
                word_bank,
                f0,
                self.prosody_amount,
                self.speed,
                p.morph,
                p.timbre,
                if replay_prosody { p.accent } else { 1.0 },
                aux,
                out,
                rng,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_banks_decode_within_limits() {
        let mut b = LpcSpeechSynthWordBank::new();
        for bank in 0..NUM_WORD_BANKS as i32 {
            assert!(b.load(bank));
            assert!(b.num_words > 0 && b.num_words < LPC_SPEECH_SYNTH_MAX_WORDS, "bank {bank}: {} words", b.num_words);
            assert!(b.num_frames > 0 && b.num_frames < LPC_SPEECH_SYNTH_MAX_FRAMES, "bank {bank}: {} frames", b.num_frames);
        }
    }

    #[test]
    fn all_speech_modes_render_finite() {
        let mut e = SpeechEngine::new();
        let mut rng = Random::new(1);
        for (k, h) in [0.1f32, 0.25, 0.4, 0.6, 0.8, 1.0].iter().enumerate() {
            let mut peak = 0.0f32;
            for n in 0..2000 {
                let trig = if n % 400 == 0 { TRIGGER_RISING_EDGE | TRIGGER_HIGH } else { TRIGGER_HIGH };
                let p = EngineParameters { trigger: trig, note: 60.0, timbre: 0.5, morph: (n as f32 / 2000.0), harmonics: *h, accent: 0.8 };
                let mut out = [0.0f32; 12];
                let mut aux = [0.0f32; 12];
                let mut env = false;
                e.render(&p, &mut out, &mut aux, &mut env, &mut rng);
                for v in out.iter().chain(aux.iter()) {
                    assert!(v.is_finite(), "mode {k}");
                    peak = peak.max(v.abs());
                }
            }
            assert!(peak > 1e-3 && peak < 4.0, "mode {k}: peak {peak}");
        }
    }
}
