// Rust port of the nimbus recording buffer: dsp/audio_buffer.h (circular
// buffer, 16-bit or mu-law 8-bit) and dsp/mu_law.h of the MIT eurorack
// texture synthesizer. Original notices:
//
// Copyright 2014 Emilie Gillet. (dsp/audio_buffer.h, dsp/mu_law.h)
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
// Differences from the original: the buffer owns its storage (allocated once
// at its largest size; `init` picks the used length), and reads that would
// fall outside the storage (out-of-bounds in the original) wrap instead.

use super::grain::{GRAIN_QUALITY_LOW, GRAIN_QUALITY_MEDIUM};
use super::tables::tables;
use crate::kit::oscs::twist::stm::clip16;

pub const CROSS_FADE_SIZE: i32 = 256;
pub const INTERPOLATION_TAIL: i32 = 8;

/// 16-bit linear to 8-bit mu-law.
#[inline]
pub fn lin2mulaw(pcm_val: i16) -> u8 {
    let mut pcm = (pcm_val >> 2) as i32;
    let mask: i32;
    if pcm < 0 {
        pcm = -pcm;
        mask = 0x7f;
    } else {
        mask = 0xff;
    }
    if pcm > 8159 {
        pcm = 8159;
    }
    pcm += 0x84 >> 2;
    let seg = if pcm <= 0x3f {
        0
    } else if pcm <= 0x7f {
        1
    } else if pcm <= 0xff {
        2
    } else if pcm <= 0x1ff {
        3
    } else if pcm <= 0x3ff {
        4
    } else if pcm <= 0x7ff {
        5
    } else if pcm <= 0xfff {
        6
    } else if pcm <= 0x1fff {
        7
    } else {
        8
    };
    if seg >= 8 {
        (0x7f ^ mask) as u8
    } else {
        let uval = ((seg << 4) | ((pcm >> (seg + 1)) & 0x0f)) as u8;
        uval ^ mask as u8
    }
}

/// What the players read from: a recording buffer of either resolution.
pub trait SampleBuffer {
    fn size(&self) -> i32;
    fn head(&self) -> i32;
    /// Raw sample at an index already inside the storage, scaled to +-1.
    fn at(&self, i: usize) -> f32;
    fn capacity(&self) -> usize;

    #[inline]
    fn base(&self, mut integral: i32, taps: usize) -> usize {
        if integral >= self.size() {
            integral -= self.size();
        }
        if integral < 0 || integral as usize + taps > self.capacity() {
            integral = integral.rem_euclid(self.size().max(1));
        }
        integral as usize
    }
    #[inline]
    fn read_zoh(&self, integral: i32, _fractional: u16) -> f32 {
        let i = self.base(integral, 1);
        self.at(i)
    }
    #[inline]
    fn read_linear(&self, integral: i32, fractional: u16) -> f32 {
        let i = self.base(integral, 2);
        let t = fractional as f32 / 65536.0;
        let x0 = self.at(i);
        let x1 = self.at(i + 1);
        x0 + (x1 - x0) * t
    }
    #[inline]
    fn read_hermite(&self, integral: i32, fractional: u16) -> f32 {
        let i = self.base(integral, 4);
        let t = fractional as f32 / 65536.0;
        let xm1 = self.at(i);
        let x0 = self.at(i + 1);
        let x1 = self.at(i + 2);
        let x2 = self.at(i + 3);
        let c = (x1 - xm1) * 0.5;
        let v = x0 - x1;
        let w = c + v;
        let a = w + v + (x2 - x0) * 0.5;
        let b_neg = w + a;
        (((a * t) - b_neg) * t + c) * t + x0
    }
    /// `Read<method>` with method 0 ZOH, 1 linear, 2 hermite.
    #[inline]
    fn read_q(&self, quality: u8, integral: i32, fractional: u16) -> f32 {
        match quality {
            GRAIN_QUALITY_LOW => self.read_zoh(integral, fractional),
            GRAIN_QUALITY_MEDIUM => self.read_linear(integral, fractional),
            _ => self.read_hermite(integral, fractional),
        }
    }
}

/// `AudioBuffer<RESOLUTION_16_BIT>` (`MU = false`) or
/// `AudioBuffer<RESOLUTION_8_BIT_MU_LAW>` (`MU = true`).
pub struct AudioBuffer<const MU: bool> {
    s16: Vec<i16>,
    s8: Vec<u8>,
    size: i32,
    write_head: i32,
    crossfade_counter: i32,
    // One extra slot: the original reads one past its 256-entry tail when
    // the crossfade ends (with a zero gain).
    tail: [i16; 257],
}

impl<const MU: bool> AudioBuffer<MU> {
    /// Storage for at most `max_samples` samples.
    pub fn new(max_samples: usize) -> Self {
        AudioBuffer {
            s16: if MU { Vec::new() } else { vec![0; max_samples] },
            s8: if MU { vec![127; max_samples] } else { Vec::new() },
            size: 0,
            write_head: 0,
            crossfade_counter: 0,
            tail: [0; 257],
        }
    }

    /// Use the first `size` samples of the storage.
    pub fn init(&mut self, size: i32) {
        let n = (size.max(0) as usize).min(if MU { self.s8.len() } else { self.s16.len() });
        self.size = n as i32 - INTERPOLATION_TAIL;
        self.write_head = 0;
        self.crossfade_counter = 0;
        if MU {
            self.s8[..n].iter_mut().for_each(|s| *s = 127);
        } else {
            self.s16[..n].iter_mut().for_each(|s| *s = 0);
        }
    }

    #[inline]
    pub fn write(&mut self, input: f32) {
        let h = self.write_head as usize;
        let sample = clip16((input * 32768.0) as i32);
        if MU {
            self.s8[h] = lin2mulaw(sample as i16);
            if self.write_head < INTERPOLATION_TAIL {
                self.s8[h + self.size as usize] = self.s8[h];
            }
        } else {
            self.s16[h] = sample as i16;
            if self.write_head < INTERPOLATION_TAIL {
                self.s16[h + self.size as usize] = self.s16[h];
            }
        }
        self.write_head += 1;
        if self.write_head >= self.size {
            self.write_head = 0;
        }
    }

    /// Records `size` samples of channel `ch` from interleaved stereo
    /// `input`; while `write` is off only the crossfade tail is recorded.
    pub fn write_fade(&mut self, input: &[f32], ch: usize, size: i32, write: bool) {
        let mut k = ch;
        if !write {
            if self.crossfade_counter < CROSS_FADE_SIZE {
                for _ in 0..size {
                    if self.crossfade_counter < CROSS_FADE_SIZE {
                        self.tail[self.crossfade_counter as usize] = clip16((input[k] * 32767.0) as i32) as i16;
                        self.crossfade_counter += 1;
                        k += 2;
                    }
                }
            }
        } else if self.crossfade_counter == 0
            && !MU
            && self.write_head >= INTERPOLATION_TAIL
            && self.write_head < self.size - size
        {
            // Fast path (note: 32767 here, 32768 in `write`).
            for _ in 0..size {
                self.s16[self.write_head as usize] = clip16((input[k] * 32767.0) as i32) as i16;
                self.write_head += 1;
                k += 2;
            }
        } else {
            for _ in 0..size {
                let mut sample = input[k];
                if self.crossfade_counter != 0 {
                    self.crossfade_counter -= 1;
                    let tail_sample = self.tail[(CROSS_FADE_SIZE - self.crossfade_counter) as usize] as f32;
                    let gain = self.crossfade_counter as f32 * (1.0 / CROSS_FADE_SIZE as f32);
                    sample += (tail_sample / 32768.0 - sample) * gain;
                }
                self.write(sample);
                k += 2;
            }
        }
    }
}

impl<const MU: bool> SampleBuffer for AudioBuffer<MU> {
    #[inline]
    fn size(&self) -> i32 {
        self.size
    }
    #[inline]
    fn head(&self) -> i32 {
        self.write_head
    }
    #[inline]
    fn capacity(&self) -> usize {
        (self.size + INTERPOLATION_TAIL).max(0) as usize
    }
    #[inline]
    fn at(&self, i: usize) -> f32 {
        if MU {
            tables().ulaw[self.s8[i] as usize] as f32 * (1.0 / 32768.0)
        } else {
            self.s16[i] as f32 * (1.0 / 32768.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mulaw_round_trip_is_close() {
        for x in [-30000i16, -1000, -10, 0, 10, 1000, 30000] {
            let y = tables().ulaw[lin2mulaw(x) as usize];
            assert!(((y as i32) - (x as i32)).abs() <= (x as i32).abs() / 16 + 8, "{x} -> {y}");
        }
    }

    #[test]
    fn buffer_records_and_reads_back() {
        let mut b = AudioBuffer::<false>::new(1024);
        b.init(1024);
        let input: Vec<f32> = (0..64).flat_map(|i| [i as f32 / 128.0, 0.0]).collect();
        b.write_fade(&input, 0, 64, true);
        assert_eq!(b.head(), 64);
        assert!((b.read_linear(10, 32768) - 10.5 / 128.0).abs() < 1e-3);
    }
}
