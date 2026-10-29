// Rust port of the nimbus looping delay player: dsp/looping_sample_player.h
// of the MIT eurorack texture synthesizer. Original notice:
//
// Copyright 2014 Emilie Gillet. (dsp/looping_sample_player.h)
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
use super::processor::Parameters;
use crate::kit::oscs::twist::stm::semitones_to_ratio;

pub const CROSSFADE_DURATION: f32 = 64.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct LoopingSamplePlayer {
    phase: f32,
    current_delay: f32,
    loop_point: f32,
    loop_duration: f32,
    tail_start: f32,
    tail_duration: f32,
    loop_reset: f32,
    synchronized: bool,
    num_channels: i32,
    tap_delay: i32,
    tap_delay_counter: i32,
}

#[inline]
fn hermite_at<B: SampleBuffer>(b: &B, position: i32) -> f32 {
    b.read_hermite(position >> 12, (position << 4) as u16)
}

impl LoopingSamplePlayer {
    pub fn init(&mut self, num_channels: i32) {
        self.num_channels = num_channels;
        self.phase = 0.0;
        self.current_delay = 0.0;
        self.loop_point = 0.0;
        self.loop_duration = 0.0;
        self.tap_delay = 0;
        self.tap_delay_counter = 0;
        self.synchronized = false;
        self.tail_duration = 1.0;
    }

    #[inline]
    pub fn synchronized(&self) -> bool {
        self.synchronized
    }

    pub fn play<B: SampleBuffer>(&mut self, buffer: &[B; 2], parameters: &Parameters, out: &mut [f32], size: usize) {
        let max_delay = (buffer[0].size() as f32 - CROSSFADE_DURATION) as i32;
        self.tap_delay_counter += size as i32;
        if self.tap_delay_counter > max_delay {
            self.tap_delay = 0;
            self.tap_delay_counter = 0;
            self.synchronized = false;
        }
        if parameters.trigger {
            self.tap_delay = self.tap_delay_counter;
            self.tap_delay_counter = 0;
            self.synchronized = self.tap_delay > 128;
            self.loop_reset = self.phase;
            self.phase = 0.0;
        }

        let head = buffer[0].head();
        let bsize = buffer[0].size();
        if !parameters.freeze {
            let mut remaining = size as i32;
            let mut o = 0;
            while remaining > 0 {
                remaining -= 1;
                let mut target_delay = parameters.position * max_delay as f32;
                if self.synchronized {
                    target_delay = self.tap_delay as f32;
                }
                let error = target_delay - self.current_delay;
                let delay = self.current_delay + 0.00005 * error;
                self.current_delay = delay;
                let mut delay_int = (head.wrapping_sub(4).wrapping_sub(remaining).wrapping_add(bsize)).wrapping_shl(12);
                delay_int = delay_int.wrapping_sub((delay * 4096.0) as i32);
                let l = hermite_at(&buffer[0], delay_int);
                if self.num_channels == 1 {
                    out[o] = l;
                    out[o + 1] = l;
                } else {
                    out[o] = l;
                    out[o + 1] = hermite_at(&buffer[1], delay_int);
                }
                o += 2;
            }
            self.phase = 0.0;
        } else {
            let mut loop_point = parameters.position * max_delay as f32 * 15.0 / 16.0;
            loop_point += CROSSFADE_DURATION;
            let d = parameters.size;
            let mut loop_duration = (0.01 + 0.99 * d * d * d) * max_delay as f32;
            if self.synchronized {
                loop_duration = self.tap_delay as f32;
            }
            if loop_point + loop_duration >= max_delay as f32 {
                loop_point = max_delay as f32 - loop_duration;
            }
            let phase_increment = if self.synchronized { 1.0 } else { semitones_to_ratio(parameters.pitch) };

            let mut o = 0;
            for _ in 0..size {
                if self.phase >= self.loop_duration || self.phase == 0.0 {
                    if self.phase >= self.loop_duration {
                        self.loop_reset = self.loop_duration;
                    }
                    if self.loop_reset >= self.loop_duration {
                        self.loop_reset = self.loop_duration;
                    }
                    self.tail_start = self.loop_duration - self.loop_reset + self.loop_point;
                    self.phase = 0.0;
                    self.tail_duration = CROSSFADE_DURATION.min(CROSSFADE_DURATION * phase_increment);
                    self.loop_point = loop_point;
                    self.loop_duration = loop_duration;
                }
                self.phase += phase_increment;

                let mut gain = 1.0f32;
                if self.tail_duration != 0.0 {
                    gain = (self.phase / self.tail_duration).clamp(0.0, 1.0);
                }
                let delay_int = (head.wrapping_sub(4).wrapping_add(bsize)).wrapping_shl(12);
                let position =
                    delay_int.wrapping_sub(((self.loop_duration - self.phase + self.loop_point) * 4096.0) as i32);
                let l = hermite_at(&buffer[0], position);
                if self.num_channels == 1 {
                    out[o] = l * gain;
                    out[o + 1] = l * gain;
                } else {
                    let r = hermite_at(&buffer[1], position);
                    out[o] = l * gain;
                    out[o + 1] = r * gain;
                }
                if gain != 1.0 {
                    let g = 1.0 - gain;
                    let position = delay_int.wrapping_sub(((-self.phase + self.tail_start) * 4096.0) as i32);
                    let l = hermite_at(&buffer[0], position);
                    if self.num_channels == 1 {
                        out[o] += l * g;
                        out[o + 1] += l * g;
                    } else {
                        let r = hermite_at(&buffer[1], position);
                        out[o] += l * g;
                        out[o + 1] += r * g;
                    }
                }
                o += 2;
            }
        }
    }
}
