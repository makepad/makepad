// Rust port of the nimbus pitch shifter / time stretcher: dsp/window.h,
// dsp/wsola_sample_player.h, dsp/correlator.h + correlator.cc of the MIT
// eurorack texture synthesizer. Original notices:
//
// Copyright 2014 Emilie Gillet. (dsp/window.h, dsp/wsola_sample_player.h,
//                                dsp/correlator.h, dsp/correlator.cc)
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
// `Window::start` also clears `done`, as the same file of the texture
// processor's MIT variant fork (same author) does; the copy we port from
// lacks that line, which leaves the stretch mode silent.

use super::buffer::SampleBuffer;
use super::processor::Parameters;
use crate::kit::oscs::twist::stm::semitones_to_ratio;

pub const MAX_WSOLA_SIZE: i32 = 4096;
/// Words of one correlator block (`kMaxWSOLASize / 32 + 2`).
pub const CORRELATOR_BLOCK_SIZE: usize = (MAX_WSOLA_SIZE as usize / 32) + 2;

/// The bit-sign correlation shifts a word right by `32 - offset_bits`, which
/// is 32 for a zero offset: undefined in C, and a shift by 0 on the desktop
/// targets the reference runs on (shift counts are taken mod 32), so the
/// whole next word is OR-ed in.
const QUIRK_CORRELATOR_SHIFT_MOD_32: bool = true;
const _: () = assert!(QUIRK_CORRELATOR_SHIFT_MOD_32);

pub struct Correlator {
    source: Vec<u32>,
    destination: Vec<u32>,
    offset: i32,
    increment: i32,
    size: i32,
    candidate: i32,
    best_score: u32,
    best_match: i32,
    done: bool,
}

impl Default for Correlator {
    fn default() -> Self {
        Correlator {
            source: vec![0; CORRELATOR_BLOCK_SIZE],
            destination: vec![0; CORRELATOR_BLOCK_SIZE * 2],
            offset: 0,
            increment: 0,
            size: 0,
            candidate: 0,
            best_score: 0,
            best_match: 0,
            done: true,
        }
    }
}

impl Correlator {
    pub fn init(&mut self) {
        self.offset = 0;
        self.best_match = 0;
        self.done = true;
    }

    pub fn start_search(&mut self, size: i32, offset: i32, increment: i32) {
        self.offset = offset;
        self.increment = increment;
        self.best_score = 0;
        self.best_match = 0;
        self.candidate = 0;
        self.size = size;
        self.done = false;
    }

    #[inline]
    pub fn best_match(&self) -> i32 {
        self.offset + ((self.best_match * (self.increment >> 4)) >> 12)
    }

    pub fn evaluate_some_candidates(&mut self) {
        let mut n = (self.size >> 2) + 16;
        while n > 0 {
            self.evaluate_next_candidate();
            n -= 1;
        }
    }

    pub fn evaluate_next_candidate(&mut self) {
        if self.done {
            return;
        }
        let num_words = (self.size >> 5) as usize;
        let offset_words = (self.candidate >> 5) as usize;
        let offset_bits = (self.candidate & 0x1f) as u32;
        let mut xcorr = 0u32;
        for i in 0..num_words {
            let source_bits = self.source[i];
            let d0 = self.destination.get(offset_words + i).copied().unwrap_or(0);
            let d1 = self.destination.get(offset_words + i + 1).copied().unwrap_or(0);
            let mut destination_bits = d0.wrapping_shl(offset_bits);
            // QUIRK_CORRELATOR_SHIFT_MOD_32: `wrapping_shr` takes the count mod 32.
            destination_bits |= d1.wrapping_shr(32 - offset_bits);
            xcorr += (!(source_bits ^ destination_bits)).count_ones();
        }
        if xcorr > self.best_score {
            self.best_match = self.candidate;
            self.best_score = xcorr;
        }
        self.candidate += 1;
        self.done = self.candidate >= self.size;
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Window {
    first_sample: i32,
    phase: i32,
    phase_increment: i32,
    envelope_phase_increment: f32,
    done: bool,
    half: bool,
    regenerated: bool,
}

impl Window {
    pub fn init(&mut self) {
        self.done = true;
        self.regenerated = false;
        self.half = false;
    }

    pub fn start(&mut self, buffer_size: i32, start: i32, width: i32, phase_increment: i32) {
        self.first_sample = start.wrapping_add(buffer_size).rem_euclid(buffer_size.max(1));
        self.phase_increment = phase_increment;
        self.phase = 0;
        self.done = false;
        self.regenerated = false;
        self.envelope_phase_increment = 2.0 / width as f32;
    }

    #[inline]
    pub fn overlap_add<B: SampleBuffer>(&mut self, buffer: &[B; 2], samples: &mut [f32], channels: i32) {
        if self.done {
            return;
        }
        let phase_integral = self.phase >> 16;
        let phase_fractional = (self.phase & 0xffff) as u16;
        let sample_index = self.first_sample + phase_integral;
        let envelope_phase = phase_integral as f32 * self.envelope_phase_increment;
        self.done = envelope_phase >= 2.0;
        self.half = envelope_phase >= 1.0;
        let gain = if envelope_phase >= 1.0 { 2.0 - envelope_phase } else { envelope_phase };
        let l = buffer[0].read_hermite(sample_index, phase_fractional) * gain;
        if channels == 1 {
            samples[0] += l;
            samples[1] += l;
        } else {
            let r = buffer[1].read_hermite(sample_index, phase_fractional) * gain;
            samples[0] += l;
            samples[1] += r;
        }
        self.phase = self.phase.wrapping_add(self.phase_increment);
    }

    #[inline]
    pub fn done(&self) -> bool {
        self.done
    }
    #[inline]
    pub fn needs_regeneration(&self) -> bool {
        self.half && !self.regenerated
    }
    #[inline]
    pub fn mark_as_regenerated(&mut self) {
        self.regenerated = true;
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WsolaSamplePlayer {
    windows: [Window; 2],
    window_size: i32,
    num_channels: i32,
    pitch: f32,
    smoothed_pitch: f32,
    position: f32,
    size_factor: f32,
    next_pitch_ratio: f32,
    correlator_loaded: bool,
    search_source: i32,
    search_target: i32,
    env_phase: f32,
    env_phase_increment: f32,
    elapsed: i32,
}

impl WsolaSamplePlayer {
    pub fn init(&mut self, num_channels: i32) {
        self.num_channels = num_channels;
        self.pitch = 0.0;
        self.position = 0.0;
        self.smoothed_pitch = 0.0;
        self.windows[0].init();
        self.windows[1].init();
        self.next_pitch_ratio = 1.0;
        self.correlator_loaded = true;
        self.search_source = 0;
        self.search_target = 0;
        self.window_size = MAX_WSOLA_SIZE / 2;
        self.env_phase = 0.0;
        self.env_phase_increment = 0.5;
        self.elapsed = 0;
    }

    pub fn play<B: SampleBuffer>(
        &mut self,
        buffer: &[B; 2],
        correlator: &mut Correlator,
        parameters: &Parameters,
        out: &mut [f32],
        size: usize,
    ) {
        self.elapsed += 1;
        if parameters.trigger {
            self.env_phase = 0.0;
            self.env_phase_increment = (1.0 / self.elapsed as f32).clamp(0.0001, 0.1);
            self.elapsed = 0;
        }
        self.env_phase += self.env_phase_increment;
        if self.env_phase >= 1.0 {
            self.env_phase = 1.0;
        }
        self.position = parameters.position;
        self.position += (1.0 - self.env_phase) * (1.0 - self.position);
        self.pitch = parameters.pitch;
        self.size_factor = parameters.size;

        if self.windows[0].done() && self.windows[1].done() {
            self.windows[1].mark_as_regenerated();
            self.schedule_aligned_window(buffer, correlator, 0);
        }

        for t in 0..size {
            let o = &mut out[2 * t..2 * t + 2];
            o[0] = 0.0;
            o[1] = 0.0;
            let nc = self.num_channels;
            for i in 0..2 {
                self.windows[i].overlap_add(buffer, o, nc);
            }
            for i in 0..2 {
                if self.windows[i].needs_regeneration() {
                    self.windows[i].mark_as_regenerated();
                    self.schedule_aligned_window(buffer, correlator, 1 - i);
                    self.windows[1 - i].overlap_add(buffer, o, nc);
                }
            }
        }
    }

    fn read_sign_bits<B: SampleBuffer>(
        buffer: &[B; 2],
        num_channels: i32,
        phase_increment: i32,
        mut source: i32,
        size: i32,
        destination: &mut [u32],
    ) -> i32 {
        let mut phase: i32 = 0;
        let mut bits: u32 = 0;
        let mut bit_counter: u32 = 0;
        let mut num_samples = 0;
        if source < 0 {
            source += buffer[0].size();
        }
        while (phase >> 16) < size {
            let integral = source + (phase >> 16);
            let fractional = (phase & 0xffff) as u16;
            let mut s = buffer[0].read_linear(integral, fractional);
            if num_channels == 2 {
                s += buffer[1].read_linear(integral, fractional);
            }
            bits |= if s > 0.0 { 1 } else { 0 };
            if (bit_counter & 0x1f) == 0x1f {
                if let Some(d) = destination.get_mut((bit_counter >> 5) as usize) {
                    *d = bits;
                }
                num_samples += 32;
            }
            bit_counter += 1;
            bits <<= 1;
            phase = phase.wrapping_add(phase_increment);
        }
        while bit_counter & 0x1f != 0 {
            if (bit_counter & 0x1f) == 0x1f {
                if let Some(d) = destination.get_mut((bit_counter >> 5) as usize) {
                    *d = bits;
                }
                num_samples += 32;
            }
            bit_counter += 1;
            bits <<= 1;
        }
        num_samples
    }

    pub fn load_correlator<B: SampleBuffer>(&mut self, buffer: &[B; 2], correlator: &mut Correlator) {
        if self.correlator_loaded {
            return;
        }
        let mut stride = (self.window_size as f32 / 2048.0).clamp(1.0, 2.0);
        stride *= 65536.0;
        let increment = (stride * if self.next_pitch_ratio < 1.25 { 1.25 } else { self.next_pitch_ratio }) as i32;
        let num_samples = Self::read_sign_bits(
            buffer,
            self.num_channels,
            increment,
            self.search_source,
            self.window_size,
            &mut correlator.source,
        );
        Self::read_sign_bits(
            buffer,
            self.num_channels,
            increment,
            self.search_target - self.window_size,
            self.window_size * 2,
            &mut correlator.destination,
        );
        correlator.start_search(num_samples, self.search_target - self.window_size + (self.window_size >> 1), increment);
        self.correlator_loaded = true;
    }

    fn schedule_aligned_window<B: SampleBuffer>(&mut self, buffer: &[B; 2], correlator: &Correlator, w: usize) {
        let next_window_position = correlator.best_match();
        self.correlator_loaded = false;
        self.windows[w].start(
            buffer[0].size(),
            next_window_position - (self.window_size >> 1),
            self.window_size,
            (self.next_pitch_ratio * 65536.0) as u32 as i32,
        );

        let mut pitch_error = self.pitch - self.smoothed_pitch;
        let pitch_error_sign = if pitch_error < 0.0 { -1.0 } else { 1.0 };
        pitch_error *= pitch_error_sign;
        if pitch_error >= 12.0 {
            pitch_error = 12.0;
        }
        self.smoothed_pitch += pitch_error * pitch_error_sign;
        let pitch_ratio = semitones_to_ratio(self.smoothed_pitch);
        let inv_pitch_ratio = semitones_to_ratio(-self.smoothed_pitch);
        self.next_pitch_ratio = pitch_ratio;

        let size_factor = semitones_to_ratio((self.size_factor - 1.0) * 60.0);
        let mut new_window_size = (size_factor * MAX_WSOLA_SIZE as f32) as i32;
        if (new_window_size - self.window_size).abs() > 64 {
            let error = (new_window_size - self.window_size) >> 5;
            new_window_size = self.window_size + error;
            self.window_size = new_window_size - (new_window_size % 4);
        }

        let mut limit = buffer[0].size();
        limit -= (2.0 * self.window_size as f32 * inv_pitch_ratio) as i32;
        limit -= 2 * self.window_size;
        if limit < 0 {
            limit = 0;
        }
        let position = self.position;
        let mut target_position = buffer[0].head();
        target_position -= (limit as f32 * position) as i32;
        target_position -= self.window_size;
        self.search_source = next_window_position;
        self.search_target = target_position;
    }
}
