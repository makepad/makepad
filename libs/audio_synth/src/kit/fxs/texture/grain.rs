// Rust port of the nimbus granular player: dsp/grain.h and
// dsp/granular_sample_player.h of the MIT eurorack texture synthesizer.
// Original notices:
//
// Copyright 2014 Emilie Gillet. (dsp/grain.h, dsp/granular_sample_player.h)
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

use super::buffer::SampleBuffer;
use super::processor::{Parameters, MAX_BLOCK_SIZE};
use super::tables::{interp, tables};
use crate::kit::oscs::twist::stm::{crossfade, fast_rsqrt_carmack, one_pole, semitones_to_ratio, slope, Random};

pub const GRAIN_QUALITY_LOW: u8 = 0;
pub const GRAIN_QUALITY_MEDIUM: u8 = 1;
pub const GRAIN_QUALITY_HIGH: u8 = 2;

pub const MAX_NUM_GRAINS: usize = 64;

#[derive(Clone, Copy, Debug, Default)]
pub struct Grain {
    first_sample: i32,
    width: i32,
    phase: i32,
    phase_increment: i32,
    pre_delay: i32,
    envelope_smoothness: f32,
    envelope_slope: f32,
    envelope_phase: f32,
    envelope_phase_increment: f32,
    gain_l: f32,
    gain_r: f32,
    active: bool,
    recommended_quality: u8,
}

impl Grain {
    pub fn init(&mut self) {
        self.active = false;
        self.envelope_phase = 2.0;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &mut self,
        pre_delay: i32,
        buffer_size: i32,
        start: i32,
        width: i32,
        phase_increment: i32,
        window_shape: f32,
        gain_l: f32,
        gain_r: f32,
        recommended_quality: u8,
    ) {
        self.pre_delay = pre_delay;
        self.width = width;
        self.first_sample = (start.wrapping_add(buffer_size)).rem_euclid(buffer_size.max(1));
        self.phase_increment = phase_increment;
        self.phase = 0;
        self.envelope_phase = 0.0;
        self.envelope_phase_increment = 2.0 / width as f32;
        if window_shape >= 0.5 {
            self.envelope_smoothness = (window_shape - 0.5) * 2.0;
            self.envelope_slope = 0.0;
        } else {
            self.envelope_smoothness = 0.0;
            self.envelope_slope = 0.5 / (window_shape + 0.01);
        }
        self.active = true;
        self.gain_l = gain_l;
        self.gain_r = gain_r;
        self.recommended_quality = recommended_quality;
    }

    #[inline]
    fn render_envelope(&mut self, use_lut: bool, quality: u8, destination: &mut [f32]) {
        let increment = self.envelope_phase_increment;
        let smoothness = self.envelope_smoothness;
        let slope = self.envelope_slope;
        let window = &tables().window;
        let mut phase = self.envelope_phase;
        for d in destination.iter_mut() {
            let mut gain = phase;
            gain = if gain >= 1.0 { 2.0 - gain } else { gain };
            if use_lut {
                if quality == GRAIN_QUALITY_HIGH {
                    let w = interp(window, gain, 4096.0);
                    gain += smoothness * (w - gain);
                }
            } else if quality >= GRAIN_QUALITY_MEDIUM {
                gain *= slope;
                if gain >= 1.0 {
                    gain = 1.0;
                }
            }
            phase += increment;
            if phase >= 2.0 {
                *d = -1.0;
                break;
            }
            *d = gain;
        }
        self.envelope_phase = phase;
    }

    /// Adds the grain into interleaved stereo `destination` (`size` frames).
    pub fn overlap_add<B: SampleBuffer>(
        &mut self,
        num_channels: i32,
        quality: u8,
        buffer: &[B; 2],
        destination: &mut [f32],
        envelope: &mut [f32],
        mut size: usize,
    ) {
        if !self.active {
            return;
        }
        let mut o = 0;
        while self.pre_delay != 0 && size != 0 {
            o += 2;
            size -= 1;
            self.pre_delay -= 1;
        }
        let use_lut = self.envelope_smoothness != 0.0;
        self.render_envelope(use_lut, quality, &mut envelope[..size]);
        let phase_increment = self.phase_increment;
        let first_sample = self.first_sample;
        let gain_l = self.gain_l;
        let gain_r = self.gain_r;
        let mut phase = self.phase;
        for &gain in envelope[..size].iter() {
            let sample_index = first_sample + (phase >> 16);
            if gain == -1.0 {
                self.active = false;
                break;
            }
            let frac = (phase & 65535) as u16;
            let l = buffer[0].read_q(quality, sample_index, frac) * gain;
            if num_channels == 1 {
                destination[o] += l * gain_l;
                destination[o + 1] += l * gain_r;
            } else {
                let r = buffer[1].read_q(quality, sample_index, frac) * gain;
                destination[o] += l * gain_l + r * (1.0 - gain_r);
                destination[o + 1] += r * gain_r + l * (1.0 - gain_l);
            }
            o += 2;
            phase = phase.wrapping_add(phase_increment);
        }
        self.phase = phase;
    }

    #[inline]
    pub fn active(&self) -> bool {
        self.active
    }
    #[inline]
    pub fn recommended_quality(&self) -> u8 {
        self.recommended_quality
    }
}

pub struct GranularSamplePlayer {
    max_num_grains: i32,
    num_midfi_grains: i32,
    num_channels: i32,
    num_grains: f32,
    gain_normalization: f32,
    grain_size_hint: f32,
    grain_rate_phasor: f32,
    grains: [Grain; MAX_NUM_GRAINS],
    available_grains: [i32; MAX_NUM_GRAINS],
    envelope_buffer: [f32; MAX_BLOCK_SIZE],
}

impl Default for GranularSamplePlayer {
    fn default() -> Self {
        GranularSamplePlayer {
            max_num_grains: 0,
            num_midfi_grains: 0,
            num_channels: 2,
            num_grains: 0.0,
            gain_normalization: 1.0,
            grain_size_hint: 1024.0,
            grain_rate_phasor: 0.0,
            grains: [Grain::default(); MAX_NUM_GRAINS],
            available_grains: [0; MAX_NUM_GRAINS],
            envelope_buffer: [0.0; MAX_BLOCK_SIZE],
        }
    }
}

impl GranularSamplePlayer {
    pub fn init(&mut self, num_channels: i32, max_num_grains: i32) {
        self.max_num_grains = max_num_grains;
        self.num_midfi_grains = 3 * max_num_grains / 4;
        self.gain_normalization = 1.0;
        for g in self.grains.iter_mut() {
            g.init();
        }
        self.num_grains = 0.0;
        self.num_channels = num_channels;
        self.grain_size_hint = 1024.0;
    }

    /// Renders `size` interleaved stereo frames into `out`.
    pub fn play<B: SampleBuffer>(&mut self, buffer: &[B; 2], parameters: &Parameters, out: &mut [f32], size: usize, rng: &mut Random) {
        let mut overlap = parameters.granular.overlap;
        overlap = overlap * overlap * overlap;
        let target_num_grains = self.max_num_grains as f32 * overlap;
        let mut p = target_num_grains / self.grain_size_hint;
        let space_between_grains = self.grain_size_hint / target_num_grains;
        if parameters.granular.use_deterministic_seed {
            p = -1.0;
        } else {
            self.grain_rate_phasor = -1000.0;
        }

        let mut num_available_grains = self.fill_available_grains_list();

        let mut seed_trigger = parameters.trigger;
        for t in 0..size {
            self.grain_rate_phasor += 1.0;
            let seed_probabilistic = rng.get_float() < p && target_num_grains > self.num_grains;
            let seed_deterministic = self.grain_rate_phasor >= space_between_grains;
            let seed = seed_probabilistic || seed_deterministic || seed_trigger;
            if num_available_grains != 0 && seed {
                num_available_grains -= 1;
                let index = self.available_grains[num_available_grains as usize] as usize;
                let quality = if num_available_grains < self.num_midfi_grains { GRAIN_QUALITY_MEDIUM } else { GRAIN_QUALITY_HIGH };
                let head = buffer[0].head().wrapping_sub(size as i32).wrapping_add(t as i32);
                self.schedule_grain(index, parameters, t as i32, buffer[0].size(), head, quality, rng);
                self.grain_rate_phasor = 0.0;
                seed_trigger = false;
            }
        }

        out[..size * 2].iter_mut().for_each(|x| *x = 0.0);
        let nc = self.num_channels;
        for i in 0..self.max_num_grains as usize {
            let q = self.grains[i].recommended_quality();
            let (grains, env) = (&mut self.grains, &mut self.envelope_buffer);
            grains[i].overlap_add(nc, q, buffer, out, env, size);
        }

        let active_grains = self.max_num_grains - num_available_grains;
        slope(&mut self.num_grains, active_grains as f32, 0.9, 0.2);

        let mut gain_normalization = if self.num_grains > 2.0 { fast_rsqrt_carmack(self.num_grains - 1.0) } else { 1.0 };
        let window_gain = (1.0 + 2.0 * parameters.granular.window_shape).clamp(1.0, 2.0);
        gain_normalization *= crossfade(1.0, window_gain, parameters.granular.overlap);

        for t in 0..size {
            one_pole(&mut self.gain_normalization, gain_normalization, 0.01);
            out[2 * t] *= self.gain_normalization;
            out[2 * t + 1] *= self.gain_normalization;
        }
    }

    fn fill_available_grains_list(&mut self) -> i32 {
        let mut n = 0;
        for i in 0..self.max_num_grains as usize {
            if !self.grains[i].active() {
                self.available_grains[n] = i as i32;
                n += 1;
            }
        }
        n as i32
    }

    #[allow(clippy::too_many_arguments)]
    fn schedule_grain(
        &mut self,
        index: usize,
        parameters: &Parameters,
        pre_delay: i32,
        buffer_size: i32,
        buffer_head: i32,
        quality: u8,
        rng: &mut Random,
    ) {
        let t = tables();
        let position = parameters.position;
        let pitch = parameters.pitch;
        let window_shape = parameters.granular.window_shape;
        let mut grain_size = interp(&t.grain_size, parameters.size, 256.0);
        let pitch_ratio = semitones_to_ratio(pitch);
        let inv_pitch_ratio = semitones_to_ratio(-pitch);
        let pan = 0.5 + parameters.stereo_spread * (rng.get_float() - 0.5);
        let (gain_l, gain_r);
        if self.num_channels == 1 {
            gain_l = interp(&t.sin, pan, 256.0);
            gain_r = interp(&t.sin[256..], pan, 256.0);
        } else if pan < 0.5 {
            gain_l = 1.0;
            gain_r = 2.0 * pan;
        } else {
            gain_r = 1.0;
            gain_l = 2.0 * (1.0 - pan);
        }
        if pitch_ratio > 1.0 {
            grain_size = grain_size.min(buffer_size as f32 * 0.25 * inv_pitch_ratio);
        }
        let eaten_by_play_head = grain_size * pitch_ratio;
        let eaten_by_recording_head = grain_size;
        let mut available = 0.0f32;
        available += buffer_size as f32;
        available -= eaten_by_play_head;
        available -= eaten_by_recording_head;

        let size = (grain_size as i32) & !1;
        let start = buffer_head.wrapping_sub((position * available + eaten_by_play_head) as i32);
        self.grains[index].start(
            pre_delay,
            buffer_size,
            start,
            size,
            (pitch_ratio * 65536.0) as u32 as i32,
            window_shape,
            gain_l,
            gain_r,
            quality,
        );
        one_pole(&mut self.grain_size_hint, grain_size, 0.1);
    }
}
