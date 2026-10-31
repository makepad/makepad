// Rust port of the nimbus effect chain: dsp/fx/fx_engine.h (delay-memory
// "assembler" for reverbs), dsp/fx/diffuser.h, dsp/fx/pitch_shifter.h and
// dsp/fx/reverb.h of the MIT eurorack texture synthesizer. Original notices:
//
// Copyright 2014 Emilie Gillet. (dsp/fx/fx_engine.h, dsp/fx/diffuser.h,
//                                dsp/fx/pitch_shifter.h, dsp/fx/reverb.h)
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
// Differences from the original: the delay memory is kept as f32 holding the
// exact values the original's 12/16-bit words decode to (each write is
// quantised the same way), so reads are identical.

use crate::kit::oscs::twist::stm::{clip16, one_pole, CosineOscillator, COSINE_OSCILLATOR_APPROXIMATE};

pub const FORMAT_12_BIT: u8 = 0;
pub const FORMAT_16_BIT: u8 = 1;
pub const FORMAT_32_BIT: u8 = 2;

/// A delay line inside the engine memory.
#[derive(Clone, Copy, Debug)]
pub struct Dl {
    pub base: i32,
    pub length: i32,
}

/// Lines laid out back to back with one guard word between them.
pub const fn lines<const N: usize>(lengths: [i32; N]) -> [Dl; N] {
    let mut out = [Dl { base: 0, length: 0 }; N];
    let mut i = 0;
    let mut base = 0;
    while i < N {
        out[i] = Dl { base, length: lengths[i] };
        base += lengths[i] + 1;
        i += 1;
    }
    out
}

/// Offset meaning "the last sample of the line" (the original's TAIL).
pub const TAIL: i32 = -1;

pub struct FxEngine<const SIZE: usize, const FORMAT: u8> {
    buffer: Vec<f32>,
    write_ptr: i32,
    lfo: [CosineOscillator; 2],
}

impl<const SIZE: usize, const FORMAT: u8> Default for FxEngine<SIZE, FORMAT> {
    fn default() -> Self {
        FxEngine { buffer: vec![0.0; SIZE], write_ptr: 0, lfo: [CosineOscillator::default(); 2] }
    }
}

pub struct Context<'a, const FORMAT: u8> {
    buffer: &'a mut [f32],
    mask: i32,
    write_ptr: i32,
    accumulator: f32,
    previous_read: f32,
    lfo_value: [f32; 2],
}

impl<const SIZE: usize, const FORMAT: u8> FxEngine<SIZE, FORMAT> {
    pub fn init(&mut self) {
        self.clear();
    }
    pub fn clear(&mut self) {
        self.buffer.iter_mut().for_each(|x| *x = 0.0);
        self.write_ptr = 0;
    }
    pub fn set_lfo_frequency(&mut self, index: usize, frequency: f32) {
        self.lfo[index].init::<COSINE_OSCILLATOR_APPROXIMATE>(frequency * 32.0);
    }
    #[inline]
    pub fn start(&mut self) -> Context<'_, FORMAT> {
        self.write_ptr -= 1;
        if self.write_ptr < 0 {
            self.write_ptr += SIZE as i32;
        }
        let lfo_value = if (self.write_ptr & 31) == 0 {
            [self.lfo[0].next(), self.lfo[1].next()]
        } else {
            [self.lfo[0].value(), self.lfo[1].value()]
        };
        Context {
            buffer: &mut self.buffer,
            mask: SIZE as i32 - 1,
            write_ptr: self.write_ptr,
            accumulator: 0.0,
            previous_read: 0.0,
            lfo_value,
        }
    }
}

impl<const FORMAT: u8> Context<'_, FORMAT> {
    #[inline(always)]
    fn quantize(v: f32) -> f32 {
        match FORMAT {
            FORMAT_12_BIT => clip16((v * 4096.0) as i32) as i16 as f32 / 4096.0,
            FORMAT_16_BIT => clip16((v * 32768.0) as i32) as i16 as f32 / 32768.0,
            _ => v,
        }
    }
    #[inline(always)]
    fn at(&self, d: Dl, offset: i32) -> usize {
        let o = if offset == -1 { d.base + d.length - 1 } else { d.base + offset };
        ((self.write_ptr + o) & self.mask) as usize
    }
    #[inline]
    pub fn load(&mut self, value: f32) {
        self.accumulator = value;
    }
    #[inline]
    pub fn read_value(&mut self, value: f32, scale: f32) {
        self.accumulator += value * scale;
    }
    /// `Write(float& value)`.
    #[inline]
    pub fn write_out(&mut self, value: &mut f32) {
        *value = self.accumulator;
    }
    /// `Write(float& value, scale)`.
    #[inline]
    pub fn write_out_scale(&mut self, value: &mut f32, scale: f32) {
        *value = self.accumulator;
        self.accumulator *= scale;
    }
    #[inline]
    pub fn write(&mut self, d: Dl, offset: i32, scale: f32) {
        let i = self.at(d, offset);
        self.buffer[i] = Self::quantize(self.accumulator);
        self.accumulator *= scale;
    }
    #[inline]
    pub fn write_all_pass(&mut self, d: Dl, offset: i32, scale: f32) {
        self.write(d, offset, scale);
        self.accumulator += self.previous_read;
    }
    #[inline]
    pub fn read(&mut self, d: Dl, offset: i32, scale: f32) {
        let r = self.buffer[self.at(d, offset)];
        self.previous_read = r;
        self.accumulator += r * scale;
    }
    #[inline]
    pub fn lp(&mut self, state: &mut f32, coefficient: f32) {
        *state += coefficient * (self.accumulator - *state);
        self.accumulator = *state;
    }
    #[inline]
    pub fn interpolate(&mut self, d: Dl, offset: f32, scale: f32) {
        let oi = offset as i32;
        let of = offset - oi as f32;
        let a = self.buffer[((self.write_ptr + oi + d.base) & self.mask) as usize];
        let b = self.buffer[((self.write_ptr + oi + d.base + 1) & self.mask) as usize];
        let x = a + (b - a) * of;
        self.previous_read = x;
        self.accumulator += x * scale;
    }
    #[inline]
    pub fn interpolate_lfo(&mut self, d: Dl, offset: f32, lfo: usize, amplitude: f32, scale: f32) {
        let offset = offset + amplitude * self.lfo_value[lfo];
        self.interpolate(d, offset, scale);
    }
}

// ---------------------------------------------------------------- diffuser

pub struct Diffuser {
    engine: FxEngine<2048, FORMAT_32_BIT>,
    amount: f32,
}

impl Default for Diffuser {
    fn default() -> Self {
        Diffuser { engine: FxEngine::default(), amount: 0.0 }
    }
}

const DIFFUSER_LINES: [Dl; 8] = lines([126, 180, 269, 444, 151, 205, 245, 405]);

impl Diffuser {
    pub fn init(&mut self) {
        self.engine.init();
    }
    pub fn set_amount(&mut self, amount: f32) {
        self.amount = amount;
    }
    /// In place on interleaved stereo frames.
    pub fn process(&mut self, in_out: &mut [f32], size: usize) {
        let kap = 0.625;
        let amount = self.amount;
        let [apl1, apl2, apl3, apl4, apr1, apr2, apr3, apr4] = DIFFUSER_LINES;
        for t in 0..size {
            let mut c = self.engine.start();
            let mut wet = 0.0;
            c.read_value(in_out[2 * t], 1.0);
            for d in [apl1, apl2, apl3, apl4] {
                c.read(d, TAIL, kap);
                c.write_all_pass(d, 0, -kap);
            }
            c.write_out_scale(&mut wet, 0.0);
            in_out[2 * t] += amount * (wet - in_out[2 * t]);

            c.read_value(in_out[2 * t + 1], 1.0);
            for d in [apr1, apr2, apr3, apr4] {
                c.read(d, TAIL, kap);
                c.write_all_pass(d, 0, -kap);
            }
            c.write_out_scale(&mut wet, 0.0);
            in_out[2 * t + 1] += amount * (wet - in_out[2 * t + 1]);
        }
    }
}

// ----------------------------------------------------------- pitch shifter

pub struct PitchShifter {
    engine: FxEngine<4096, FORMAT_16_BIT>,
    phase: f32,
    ratio: f32,
    size: f32,
}

impl Default for PitchShifter {
    fn default() -> Self {
        PitchShifter { engine: FxEngine::default(), phase: 0.0, ratio: 1.0, size: 2047.0 }
    }
}

const PITCH_SHIFTER_LINES: [Dl; 2] = lines([2047, 2047]);

impl PitchShifter {
    pub fn init(&mut self) {
        self.engine.init();
        self.phase = 0.0;
        self.size = 2047.0;
    }
    pub fn clear(&mut self) {
        self.engine.clear();
    }
    pub fn set_ratio(&mut self, ratio: f32) {
        self.ratio = ratio;
    }
    pub fn set_size(&mut self, size: f32) {
        let target = 128.0 + (2047.0 - 128.0) * size * size * size;
        one_pole(&mut self.size, target, 0.05);
    }
    pub fn process(&mut self, in_out: &mut [f32], size: usize) {
        let [left, right] = PITCH_SHIFTER_LINES;
        for t in 0..size {
            self.phase += (1.0 - self.ratio) / self.size;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
            if self.phase <= 0.0 {
                self.phase += 1.0;
            }
            let tri = 2.0 * if self.phase >= 0.5 { 1.0 - self.phase } else { self.phase };
            let phase = self.phase * self.size;
            let mut half = phase + self.size * 0.5;
            if half >= self.size {
                half -= self.size;
            }
            let mut c = self.engine.start();
            let mut v = 0.0;
            c.read_value(in_out[2 * t], 1.0);
            c.write(left, 0, 0.0);
            c.interpolate(left, phase, tri);
            c.interpolate(left, half, 1.0 - tri);
            c.write_out_scale(&mut v, 0.0);
            in_out[2 * t] = v;

            c.read_value(in_out[2 * t + 1], 1.0);
            c.write(right, 0, 0.0);
            c.interpolate(right, phase, tri);
            c.interpolate(right, half, 1.0 - tri);
            c.write_out_scale(&mut v, 0.0);
            in_out[2 * t + 1] = v;
        }
    }
}

// ------------------------------------------------------------------ reverb

pub struct Reverb {
    engine: FxEngine<16384, FORMAT_12_BIT>,
    amount: f32,
    input_gain: f32,
    reverb_time: f32,
    diffusion: f32,
    lp: f32,
    lp_decay_1: f32,
    lp_decay_2: f32,
}

impl Default for Reverb {
    fn default() -> Self {
        Reverb {
            engine: FxEngine::default(),
            amount: 0.0,
            input_gain: 0.0,
            reverb_time: 0.0,
            diffusion: 0.625,
            lp: 0.7,
            lp_decay_1: 0.0,
            lp_decay_2: 0.0,
        }
    }
}

const REVERB_LINES: [Dl; 10] = lines([113, 162, 241, 399, 1653, 2038, 3411, 1913, 1663, 4782]);

impl Reverb {
    pub fn init(&mut self) {
        self.engine.init();
        self.engine.set_lfo_frequency(0, 0.5 / 32000.0);
        self.engine.set_lfo_frequency(1, 0.3 / 32000.0);
        self.lp = 0.7;
        self.diffusion = 0.625;
    }
    pub fn set_amount(&mut self, v: f32) {
        self.amount = v;
    }
    pub fn set_input_gain(&mut self, v: f32) {
        self.input_gain = v;
    }
    pub fn set_time(&mut self, v: f32) {
        self.reverb_time = v;
    }
    pub fn set_diffusion(&mut self, v: f32) {
        self.diffusion = v;
    }
    pub fn set_lp(&mut self, v: f32) {
        self.lp = v;
    }

    /// Griesinger topology (4 input allpasses, then a loop of 2x (2 AP + delay)).
    pub fn process(&mut self, in_out: &mut [f32], size: usize) {
        let [ap1, ap2, ap3, ap4, dap1a, dap1b, del1, dap2a, dap2b, del2] = REVERB_LINES;
        let kap = self.diffusion;
        let klp = self.lp;
        let krt = self.reverb_time;
        let amount = self.amount;
        let gain = self.input_gain;
        let mut lp_1 = self.lp_decay_1;
        let mut lp_2 = self.lp_decay_2;
        for t in 0..size {
            let mut wet = 0.0;
            let mut apout = 0.0;
            let mut c = self.engine.start();

            c.interpolate_lfo(ap1, 10.0, 0, 60.0, 1.0);
            c.write(ap1, 100, 0.0);

            c.read_value(in_out[2 * t] + in_out[2 * t + 1], gain);
            for d in [ap1, ap2, ap3, ap4] {
                c.read(d, TAIL, kap);
                c.write_all_pass(d, 0, -kap);
            }
            c.write_out(&mut apout);

            c.load(apout);
            c.interpolate_lfo(del2, 4680.0, 1, 100.0, krt);
            c.lp(&mut lp_1, klp);
            c.read(dap1a, TAIL, -kap);
            c.write_all_pass(dap1a, 0, kap);
            c.read(dap1b, TAIL, kap);
            c.write_all_pass(dap1b, 0, -kap);
            c.write(del1, 0, 2.0);
            c.write_out_scale(&mut wet, 0.0);
            in_out[2 * t] += (wet - in_out[2 * t]) * amount;

            c.load(apout);
            c.read(del1, TAIL, krt);
            c.lp(&mut lp_2, klp);
            c.read(dap2a, TAIL, kap);
            c.write_all_pass(dap2a, 0, -kap);
            c.read(dap2b, TAIL, -kap);
            c.write_all_pass(dap2b, 0, kap);
            c.write(del2, 0, 2.0);
            c.write_out_scale(&mut wet, 0.0);
            in_out[2 * t + 1] += (wet - in_out[2 * t + 1]) * amount;
        }
        self.lp_decay_1 = lp_1;
        self.lp_decay_2 = lp_2;
    }
}
