// Rust port of the nimbus spectral mode: dsp/pvoc/stft.h + stft.cc,
// dsp/pvoc/phase_vocoder.h + phase_vocoder.cc and
// dsp/pvoc/frame_transformation.h + frame_transformation.cc of the MIT
// eurorack texture synthesizer. Original notices:
//
// Copyright 2014 Emilie Gillet. (dsp/pvoc/stft.h, dsp/pvoc/stft.cc,
//                                dsp/pvoc/phase_vocoder.h, dsp/pvoc/phase_vocoder.cc,
//                                dsp/pvoc/frame_transformation.h,
//                                dsp/pvoc/frame_transformation.cc)
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
// Differences from the original: buffers are owned (allocated once at their
// largest size; the number of magnitude textures follows the original's
// allocation arithmetic over the two memory blocks), the phase words the
// original keeps inside the last texture are their own arrays (zeroed instead
// of holding stale memory), and the u32 phases it stores in the float FFT
// buffer go through `to_bits` / `from_bits`.

use super::atan::fast_atan2r;
use super::fft::{ShyFft, FFT_SIZE};
use super::processor::Parameters;
use super::tables::{interp, tables};
use crate::kit::oscs::twist::stm::{clip16, crossfade, semitones_to_ratio, Random};

pub const MAX_NUM_TEXTURES: usize = 7;
pub const HIGH_FREQUENCY_TRUNCATION: usize = 16;
const TEXTURE_SIZE: usize = (FFT_SIZE >> 1) - HIGH_FREQUENCY_TRUNCATION;
const HOP_RATIO: usize = 4;
const HOP_SIZE: usize = FFT_SIZE / HOP_RATIO;
const STFT_BUFFER_SIZE: usize = FFT_SIZE + HOP_SIZE;

/// A float to u16 conversion of values beyond 65535: the reference's
/// compilers convert through a 32-bit integer and keep the low 16 bits.
const QUIRK_PHASE_ADVANCE_WRAPS_U16: bool = true;
const _: () = assert!(QUIRK_PHASE_ADVANCE_WRAPS_U16);

const WARP_POLYNOMIALS: [[f32; 4]; 6] = [
    [10.5882, -14.8824, 5.29412, 0.0],
    [-7.3333, 9.0, -1.79167, 0.125],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.5, 0.5, 0.0],
    [-7.3333, 9.5, -2.416667, 0.25],
    [-7.3333, 9.5, -2.416667, 0.25],
];

pub struct FrameTransformation {
    fft_size: usize,
    num_textures: usize,
    size: usize,
    textures: Vec<f32>,
    phases: Vec<u16>,
    phases_delta: Vec<u16>,
    glitch_algorithm: i32,
}

impl Default for FrameTransformation {
    fn default() -> Self {
        FrameTransformation {
            fft_size: FFT_SIZE,
            num_textures: 0,
            size: TEXTURE_SIZE,
            textures: vec![0.0; MAX_NUM_TEXTURES * TEXTURE_SIZE],
            phases: vec![0; TEXTURE_SIZE],
            phases_delta: vec![0; TEXTURE_SIZE],
            glitch_algorithm: 0,
        }
    }
}

impl FrameTransformation {
    pub fn init(&mut self, fft_size: usize, num_textures: usize) {
        self.fft_size = fft_size;
        self.size = (fft_size >> 1) - HIGH_FREQUENCY_TRUNCATION;
        // The last texture holds the phases.
        self.num_textures = num_textures - 1;
        self.phases.iter_mut().for_each(|x| *x = 0);
        self.phases_delta.iter_mut().for_each(|x| *x = 0);
        self.glitch_algorithm = 0;
        self.reset();
    }

    pub fn reset(&mut self) {
        let n = self.num_textures * self.size;
        self.textures[..n].iter_mut().for_each(|x| *x = 0.0);
    }

    #[inline]
    fn tex(&self, i: usize) -> usize {
        i * self.size
    }

    pub fn process(&mut self, parameters: &Parameters, fft_out: &mut [f32], ifft_in: &mut [f32], rng: &mut Random) {
        fft_out[0] = 0.0;
        fft_out[self.fft_size >> 1] = 0.0;
        let freeze = parameters.freeze;
        let glitch = parameters.gate;
        let pitch_ratio = semitones_to_ratio(parameters.pitch);
        if !freeze {
            self.rectangular_to_polar(fft_out);
            self.store_magnitudes(fft_out, parameters.position, parameters.spectral.refresh_rate, rng);
        }
        self.replay_magnitudes(ifft_in, parameters.position);
        self.warp_magnitudes(ifft_in, fft_out, parameters.spectral.warp);
        self.shift_magnitudes(fft_out, ifft_in, pitch_ratio);
        if glitch {
            self.add_glitch(ifft_in, rng);
        }
        self.quantize_magnitudes(ifft_in, parameters.spectral.quantization);
        self.set_phases(ifft_in, parameters.spectral.phase_randomization, pitch_ratio, rng);
        self.polar_to_rectangular(ifft_in);
        if !glitch {
            self.glitch_algorithm = (rng.get_sample() & 3) as i32;
        }
        ifft_in[0] = 0.0;
        ifft_in[self.fft_size >> 1] = 0.0;
    }

    fn rectangular_to_polar(&mut self, fft_data: &mut [f32]) {
        let h = self.fft_size >> 1;
        for i in 1..self.size {
            let mut m = 0.0;
            let angle = fast_atan2r(fft_data[h + i], fft_data[i], &mut m);
            fft_data[i] = m;
            self.phases_delta[i] = angle.wrapping_sub(self.phases[i]);
            self.phases[i] = angle;
        }
    }

    fn set_phases(&mut self, destination: &mut [f32], phase_randomization: f32, pitch_ratio: f32, rng: &mut Random) {
        let h = self.fft_size >> 1;
        for i in 0..self.size {
            destination[h + i] = f32::from_bits(self.phases[i] as u32);
            // QUIRK_PHASE_ADVANCE_WRAPS_U16
            let adv = ((self.phases_delta[i] as f32 * pitch_ratio) as i32) as u16;
            self.phases[i] = self.phases[i].wrapping_add(adv);
        }
        let mut r = phase_randomization;
        r = ((r - 0.05) * 1.06).clamp(0.0, 1.0);
        r *= r;
        let amount = (r * 32768.0) as i32;
        for i in 0..self.size {
            let p = destination[h + i].to_bits();
            let add = ((rng.get_sample() as i32).wrapping_mul(amount)) >> 14;
            destination[h + i] = f32::from_bits(p.wrapping_add(add as u32));
        }
    }

    fn polar_to_rectangular(&mut self, fft_data: &mut [f32]) {
        let h = self.fft_size >> 1;
        let sin = &tables().sin;
        for i in 1..self.size {
            let magnitude = fft_data[i];
            let angle = (fft_data[h + i].to_bits() as u16) >> 6;
            fft_data[i] = magnitude * sin[angle as usize + 256];
            fft_data[h + i] = magnitude * sin[angle as usize];
        }
        for i in self.size..h {
            fft_data[i] = 0.0;
            fft_data[h + i] = 0.0;
        }
    }

    fn add_glitch(&mut self, x: &mut [f32], rng: &mut Random) {
        let size = self.size;
        match self.glitch_algorithm {
            0 => {
                let mut held = 0.0f32;
                for v in x[..size].iter_mut() {
                    if (rng.get_sample() & 15) == 0 {
                        held = *v;
                    }
                    *v = held;
                    held *= 1.01;
                }
            }
            1 => {
                let factor = 1.0 + (rng.get_sample() & 7) as f32 / 4.0;
                let mut source = 0.0f32;
                for i in 0..size {
                    source += factor;
                    if source >= size as f32 {
                        source = 0.0;
                    }
                    x[i] = x[source as i32 as usize];
                }
            }
            2 => {
                let mut m = 0;
                for i in 1..size {
                    if x[i] > x[m] {
                        m = i;
                    }
                }
                x[m] = 0.0;
                let mut m = 0;
                for i in 1..size {
                    if x[i] > x[m] {
                        m = i;
                    }
                }
                x[m] *= 8.0;
            }
            3 => {
                for (i, v) in x[..size].iter_mut().enumerate() {
                    let random = (rng.get_sample() as u16 as u32) & 15;
                    if random == 0 {
                        *v *= i as f32 / 16.0;
                    }
                }
            }
            _ => {}
        }
    }

    fn quantize_magnitudes(&mut self, xf_polar: &mut [f32], mut amount: f32) {
        let size = self.size;
        if amount <= 0.48 {
            amount *= 2.0;
            let scale_down = 0.5 * semitones_to_ratio(-108.0 * (1.0 - amount * amount)) / self.fft_size as f32;
            let scale_up = 1.0 / scale_down;
            for v in xf_polar[..size].iter_mut() {
                *v = scale_up * ((scale_down * *v) as i32) as f32;
            }
        } else if amount >= 0.52 {
            amount = (amount - 0.52) * 2.0;
            let mut norm = xf_polar[0];
            for &v in xf_polar[1..size].iter() {
                if v > norm {
                    norm = v;
                }
            }
            let inv_norm = 1.0 / (norm + 0.0001);
            for v in xf_polar[1..size].iter_mut() {
                let x = *v * inv_norm;
                let warped = 4.0 * x * (1.0 - x) * (1.0 - x) * (1.0 - x);
                *v = (x + (warped - x) * amount) * norm;
            }
        }
    }

    fn warp_magnitudes(&mut self, source: &[f32], xf_polar: &mut [f32], amount: f32) {
        let size = self.size;
        let bin_width = 1.0 / size as f32;
        let mut f = 0.0f32;
        let amount = amount * 4.0;
        let ai = amount as i32 as usize;
        let af = amount - ai as f32;
        let mut co = [0.0f32; 4];
        for (i, c) in co.iter_mut().enumerate() {
            *c = crossfade(WARP_POLYNOMIALS[ai][i], WARP_POLYNOMIALS[(ai + 1).min(5)][i], af);
        }
        let [a, b, c, d] = co;
        for v in xf_polar[1..size].iter_mut() {
            f += bin_width;
            let wf = (d + f * (c + f * (b + a * f))) * size as f32;
            *v = interp(source, wf, 1.0);
        }
    }

    fn shift_magnitudes(&mut self, source: &[f32], xf_polar: &mut [f32], pitch_ratio: f32) {
        let size = self.size;
        // `temp` is xf_polar[size..2 size].
        if pitch_ratio == 1.0 {
            xf_polar[size..2 * size].copy_from_slice(&source[..size]);
        } else if pitch_ratio > 1.0 {
            let mut index = 1.0f32;
            let increment = 1.0 / pitch_ratio;
            for i in 1..size {
                xf_polar[size + i] = interp(source, index, 1.0);
                index += increment;
            }
        } else {
            xf_polar[size..2 * size].iter_mut().for_each(|x| *x = 0.0);
            let mut index = 1.0f32;
            let increment = pitch_ratio;
            for i in 1..size {
                let ii = index as i32 as usize;
                let fr = index - ii as f32;
                xf_polar[size + ii] += (1.0 - fr) * source[i];
                xf_polar[size + ii + 1] += fr * source[i];
                index += increment;
            }
        }
        xf_polar.copy_within(size..2 * size, 0);
    }

    fn store_magnitudes(&mut self, xf_polar: &[f32], position: f32, mut feedback: f32, rng: &mut Random) {
        let size = self.size;
        let index_float = position * (self.num_textures as f32 - 1.0);
        let index_int = index_float as i32 as usize;
        let index_fractional = index_float - index_int as f32;
        let mut gain_a = 1.0 - index_fractional;
        let mut gain_b = index_fractional;
        let a = self.tex(index_int);
        let b = self.tex(index_int + if position == 1.0 { 0 } else { 1 });
        let t = &mut self.textures;
        if feedback >= 0.5 {
            feedback = 2.0 * (feedback - 0.5);
            if feedback < 0.5 {
                gain_a *= 1.0 - feedback;
                gain_b *= 1.0 - feedback;
                for i in 0..size {
                    let x = xf_polar[i];
                    t[a + i] = crossfade(t[a + i], x, gain_a);
                    t[b + i] = crossfade(t[b + i], x, gain_b);
                }
            } else {
                let tt = (feedback - 0.5) * 0.7 + 0.5;
                let mut gain_new = tt - 0.5;
                gain_new = gain_new * gain_new * 2.0 + 0.5;
                let gain_new_a = gain_a * gain_new;
                let gain_new_b = gain_b * gain_new;
                let gain_old_a = 1.0 - gain_a * (1.0 - tt);
                let gain_old_b = 1.0 - gain_b * (1.0 - tt);
                for i in 0..size {
                    let x = xf_polar[i];
                    t[a + i] = t[a + i] * gain_old_a + x * gain_new_a;
                    t[b + i] = t[b + i] * gain_old_b + x * gain_new_b;
                }
            }
        } else {
            feedback *= 2.0;
            feedback *= feedback;
            let threshold = (feedback * 65535.0) as u16;
            for i in 0..size {
                let x = xf_polar[i];
                let gain = if (rng.get_sample() as u16) <= threshold { 1.0 } else { 0.0 };
                t[a + i] = crossfade(t[a + i], x, gain_a * gain);
                t[b + i] = crossfade(t[b + i], x, gain_b * gain);
            }
        }
    }

    fn replay_magnitudes(&mut self, xf_polar: &mut [f32], position: f32) {
        let size = self.size;
        let index_float = position * (self.num_textures as f32 - 1.0);
        let index_int = index_float as i32 as usize;
        let index_fractional = index_float - index_int as f32;
        let a = self.tex(index_int);
        let b = self.tex(index_int + if position == 1.0 { 0 } else { 1 });
        for i in 0..size {
            xf_polar[i] = crossfade(self.textures[a + i], self.textures[b + i], index_fractional);
        }
    }
}

pub struct Stft {
    analysis: Vec<i16>,
    synthesis: Vec<i16>,
    buffer_ptr: usize,
    process_ptr: usize,
    block_size: usize,
    ready: usize,
    done: usize,
    has_parameters: bool,
}

impl Default for Stft {
    fn default() -> Self {
        Stft {
            analysis: vec![0; STFT_BUFFER_SIZE],
            synthesis: vec![0; STFT_BUFFER_SIZE],
            buffer_ptr: 0,
            process_ptr: 0,
            block_size: 0,
            ready: 0,
            done: 0,
            has_parameters: false,
        }
    }
}

impl Stft {
    pub fn init(&mut self) {
        self.has_parameters = false;
        self.reset();
    }

    pub fn reset(&mut self) {
        self.buffer_ptr = 0;
        self.process_ptr = (2 * HOP_SIZE) % STFT_BUFFER_SIZE;
        self.block_size = 0;
        self.analysis.iter_mut().for_each(|x| *x = 0);
        self.synthesis.iter_mut().for_each(|x| *x = 0);
        self.ready = 0;
        self.done = 0;
    }

    /// One channel of interleaved stereo (`ch`) in and out.
    pub fn process(&mut self, input: &[f32], output: &mut [f32], ch: usize, mut size: usize) {
        self.has_parameters = true;
        let mut k = ch;
        while size > 0 {
            let processed = size.min(HOP_SIZE - self.block_size);
            for i in 0..processed {
                let sample = (input[k] * 32768.0) as i32;
                self.analysis[self.buffer_ptr + i] = clip16(sample) as i16;
                output[k] = self.synthesis[self.buffer_ptr + i] as f32 / 16384.0;
                k += 2;
            }
            self.block_size += processed;
            size -= processed;
            self.buffer_ptr += processed;
            if self.buffer_ptr >= STFT_BUFFER_SIZE {
                self.buffer_ptr -= STFT_BUFFER_SIZE;
            }
            if self.block_size >= HOP_SIZE {
                self.block_size -= HOP_SIZE;
                self.ready += 1;
            }
        }
    }

    pub fn buffer(
        &mut self,
        fft: &mut ShyFft,
        fft_buf: &mut [f32],
        ifft_buf: &mut [f32],
        modifier: &mut FrameTransformation,
        parameters: &Parameters,
        rng: &mut Random,
    ) {
        if self.ready == self.done {
            return;
        }
        let window = &tables().sine_window_4096;
        let window_stride = 4096 / FFT_SIZE;
        // fft_in = fft_buf, fft_out = ifft_buf, ifft_in = fft_buf, ifft_out = ifft_buf.
        let mut source_ptr = self.process_ptr;
        for i in 0..FFT_SIZE {
            fft_buf[i] = window[i * window_stride] * self.analysis[source_ptr] as f32;
            source_ptr += 1;
            if source_ptr >= STFT_BUFFER_SIZE {
                source_ptr -= STFT_BUFFER_SIZE;
            }
        }
        fft.direct(fft_buf, ifft_buf);
        if self.has_parameters {
            modifier.process(parameters, ifft_buf, fft_buf, rng);
        } else {
            fft_buf.copy_from_slice(ifft_buf);
        }
        fft.inverse(fft_buf, ifft_buf);

        let mut destination_ptr = self.process_ptr;
        let inverse_window_size = 1.0 / ((FFT_SIZE * FFT_SIZE / HOP_SIZE) >> 1) as f32;
        for i in 0..FFT_SIZE {
            let s = ifft_buf[i] * window[i * window_stride] * inverse_window_size;
            let mut x = s as i32;
            if i < FFT_SIZE - HOP_SIZE {
                x += self.synthesis[destination_ptr] as i32;
            }
            self.synthesis[destination_ptr] = clip16(x) as i16;
            destination_ptr += 1;
            if destination_ptr >= STFT_BUFFER_SIZE {
                destination_ptr -= STFT_BUFFER_SIZE;
            }
        }
        self.done += 1;
        self.process_ptr += HOP_SIZE;
        if self.process_ptr >= STFT_BUFFER_SIZE {
            self.process_ptr -= STFT_BUFFER_SIZE;
        }
    }
}

pub struct PhaseVocoder {
    fft: ShyFft,
    fft_buffer: Vec<f32>,
    ifft_buffer: Vec<f32>,
    stft: [Stft; 2],
    frame_transformation: [FrameTransformation; 2],
    num_channels: usize,
}

impl Default for PhaseVocoder {
    fn default() -> Self {
        PhaseVocoder {
            fft: ShyFft::default(),
            fft_buffer: vec![0.0; FFT_SIZE],
            ifft_buffer: vec![0.0; FFT_SIZE],
            stft: [Stft::default(), Stft::default()],
            frame_transformation: [FrameTransformation::default(), FrameTransformation::default()],
            num_channels: 2,
        }
    }
}

impl PhaseVocoder {
    /// `buffer_size` = the two memory blocks' byte sizes (the second is 0 in
    /// mono); the texture count follows the original's allocations in them.
    pub fn init(&mut self, buffer_size: [usize; 2], num_channels: usize) {
        self.num_channels = num_channels;
        let mut free = buffer_size;
        let f32s = std::mem::size_of::<f32>();
        // fft buffer from block 0, ifft buffer from block (channels - 1).
        free[0] = free[0].saturating_sub(FFT_SIZE * f32s);
        free[num_channels - 1] = free[num_channels - 1].saturating_sub(FFT_SIZE * f32s);
        let mut num_textures = MAX_NUM_TEXTURES;
        for i in 0..num_channels {
            free[i] = free[i].saturating_sub((FFT_SIZE + (FFT_SIZE >> 1)) * 2 * std::mem::size_of::<i16>());
            num_textures = num_textures.min(free[i] / (f32s * TEXTURE_SIZE));
            self.stft[i].init();
        }
        self.fft.init();
        for i in 0..num_channels {
            self.frame_transformation[i].init(FFT_SIZE, num_textures);
        }
    }

    pub fn process(&mut self, input: &[f32], output: &mut [f32], size: usize) {
        for i in 0..self.num_channels {
            self.stft[i].process(input, output, i, size);
        }
    }

    pub fn buffer(&mut self, parameters: &Parameters, rng: &mut Random) {
        for i in 0..self.num_channels {
            self.stft[i].buffer(
                &mut self.fft,
                &mut self.fft_buffer,
                &mut self.ifft_buffer,
                &mut self.frame_transformation[i],
                parameters,
                rng,
            );
        }
    }
}
