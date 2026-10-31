// Rust port of the twist engine's delay-network helpers: dsp/fx/fx_engine.h
// (12-bit delay memory engine; the 12-bit format is the only one used) and
// dsp/fx/diffuser.h of the MIT eurorack macro-oscillator. Original notices:
//
// Copyright 2014 Emilie Gillet. (dsp/fx/fx_engine.h)
// Copyright 2016 Emilie Gillet. (dsp/fx/diffuser.h)
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

// ------------------------------------------------------- fx/fx_engine.h

/// A delay line inside the engine's shared memory: `base` and `length` as
/// the original's `Reserve<>` chain lays them out (each line takes
/// `length + 1` slots).
#[derive(Clone, Copy, Debug)]
pub struct Line {
    pub base: i32,
    pub length: i32,
}

/// Lays out lines of the given lengths like the `Reserve<>` chain.
pub const fn layout<const N: usize>(lengths: [i32; N]) -> [Line; N] {
    let mut out = [Line { base: 0, length: 0 }; N];
    let mut i = 0;
    let mut base = 0;
    while i < N {
        out[i] = Line { base, length: lengths[i] };
        base += lengths[i] + 1;
        i += 1;
    }
    out
}

/// The `TAIL` offset: read/write at the line's last slot.
pub const TAIL: i32 = -1;

pub const LFO_1: usize = 0;

/// 12-bit fixed point in a 16-bit word (`FORMAT_12_BIT`).
#[inline]
fn decompress(v: u16) -> f32 {
    v as i16 as f32 / 4096.0
}

#[inline]
fn compress(v: f32) -> u16 {
    clip16((v * 4096.0) as i32) as i16 as u16
}

/// Delay-network engine over a power-of-two ring of 12-bit samples.
pub struct FxEngine {
    buffer: Box<[u16]>,
    mask: i32,
    write_ptr: i32,
    lfo: [CosineOscillator; 2],
}

/// The per-sample accumulator of the original's `Context`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Context {
    accumulator: f32,
    previous_read: f32,
    lfo_value: [f32; 2],
    write_ptr: i32,
}

impl FxEngine {
    pub fn new(size: usize) -> FxEngine {
        debug_assert!(size.is_power_of_two());
        FxEngine {
            buffer: vec![0u16; size].into_boxed_slice(),
            mask: size as i32 - 1,
            write_ptr: 0,
            lfo: [CosineOscillator::default(); 2],
        }
    }

    pub fn clear(&mut self) {
        self.buffer.iter_mut().for_each(|x| *x = 0);
        self.write_ptr = 0;
    }

    pub fn set_lfo_frequency(&mut self, index: usize, frequency: f32) {
        self.lfo[index].init::<COSINE_OSCILLATOR_APPROXIMATE>(frequency * 32.0);
    }

    #[inline]
    pub fn start(&mut self, c: &mut Context) {
        self.write_ptr -= 1;
        if self.write_ptr < 0 {
            self.write_ptr += self.mask + 1;
        }
        c.accumulator = 0.0;
        c.previous_read = 0.0;
        c.write_ptr = self.write_ptr;
        if self.write_ptr & 31 == 0 {
            c.lfo_value[0] = self.lfo[0].next();
            c.lfo_value[1] = self.lfo[1].next();
        } else {
            c.lfo_value[0] = self.lfo[0].value();
            c.lfo_value[1] = self.lfo[1].value();
        }
    }

    #[inline]
    fn at(&self, i: i32) -> usize {
        (i & self.mask) as usize
    }

    // Context operations (the buffer lives in the engine here; only the
    // operations the diffuser uses are ported).

    #[inline]
    pub fn read_input(c: &mut Context, value: f32) {
        c.accumulator += value;
    }
    #[inline]
    pub fn write_value(c: &mut Context, value: &mut f32, scale: f32) {
        *value = c.accumulator;
        c.accumulator *= scale;
    }
    #[inline]
    pub fn write(&mut self, c: &mut Context, d: Line, offset: i32, scale: f32) {
        let w = compress(c.accumulator);
        let i = if offset == -1 {
            self.at(c.write_ptr + d.base + d.length - 1)
        } else {
            self.at(c.write_ptr + d.base + offset)
        };
        self.buffer[i] = w;
        c.accumulator *= scale;
    }
    #[inline]
    pub fn write_all_pass(&mut self, c: &mut Context, d: Line, offset: i32, scale: f32) {
        self.write(c, d, offset, scale);
        c.accumulator += c.previous_read;
    }
    #[inline]
    pub fn read(&self, c: &mut Context, d: Line, offset: i32, scale: f32) {
        let i = if offset == -1 {
            self.at(c.write_ptr + d.base + d.length - 1)
        } else {
            self.at(c.write_ptr + d.base + offset)
        };
        let r = decompress(self.buffer[i]);
        c.previous_read = r;
        c.accumulator += r * scale;
    }
    #[inline]
    pub fn lp(c: &mut Context, state: &mut f32, coefficient: f32) {
        *state += coefficient * (c.accumulator - *state);
        c.accumulator = *state;
    }
    #[inline]
    pub fn interpolate(&self, c: &mut Context, d: Line, offset: f32, scale: f32) {
        let (oi, of) = integral_fractional(offset);
        let a = decompress(self.buffer[self.at(c.write_ptr + oi + d.base)]);
        let b = decompress(self.buffer[self.at(c.write_ptr + oi + d.base + 1)]);
        let x = a + (b - a) * of;
        c.previous_read = x;
        c.accumulator += x * scale;
    }
    #[inline]
    pub fn interpolate_lfo(&self, c: &mut Context, d: Line, offset: f32, lfo: usize, amplitude: f32, scale: f32) {
        let offset = offset + amplitude * c.lfo_value[lfo];
        self.interpolate(c, d, offset, scale);
    }
}

// --------------------------------------------------------- fx/diffuser.h

const DIFFUSER_LINES: [Line; 7] = layout([126, 180, 269, 444, 1653, 2010, 3411]);

/// Granular diffuser (allpass chain + modulated delay loop).
pub struct Diffuser {
    engine: FxEngine,
    lp_decay: f32,
}

impl Diffuser {
    pub fn new() -> Diffuser {
        let mut engine = FxEngine::new(8192);
        engine.set_lfo_frequency(LFO_1, 0.3 / 48000.0);
        Diffuser { engine, lp_decay: 0.0 }
    }

    pub fn reset(&mut self) {
        self.engine.clear();
    }

    pub fn process(&mut self, amount: f32, rt: f32, in_out: &mut [f32]) {
        let [ap1, ap2, ap3, ap4, dapa, dapb, del] = DIFFUSER_LINES;
        let kap = 0.625f32;
        let klp = 0.75f32;
        let mut lp = self.lp_decay;
        let e = &mut self.engine;
        let mut c = Context::default();
        for x in in_out.iter_mut() {
            let mut wet = 0.0f32;
            e.start(&mut c);
            FxEngine::read_input(&mut c, *x);
            e.read(&mut c, ap1, TAIL, kap);
            e.write_all_pass(&mut c, ap1, 0, -kap);
            e.read(&mut c, ap2, TAIL, kap);
            e.write_all_pass(&mut c, ap2, 0, -kap);
            e.read(&mut c, ap3, TAIL, kap);
            e.write_all_pass(&mut c, ap3, 0, -kap);
            e.interpolate_lfo(&mut c, ap4, 400.0, LFO_1, 43.0, kap);
            e.write_all_pass(&mut c, ap4, 0, -kap);
            e.interpolate_lfo(&mut c, del, 3070.0, LFO_1, 340.0, rt);
            FxEngine::lp(&mut c, &mut lp, klp);
            e.read(&mut c, dapa, TAIL, -kap);
            e.write_all_pass(&mut c, dapa, 0, kap);
            e.read(&mut c, dapb, TAIL, kap);
            e.write_all_pass(&mut c, dapb, 0, -kap);
            e.write(&mut c, del, 0, 2.0);
            FxEngine::write_value(&mut c, &mut wet, 0.0);
            *x += amount * (wet - *x);
        }
        self.lp_decay = lp;
    }
}
