// Ported from Mojo (MojoProc.cpp, Mojo.cpp) of the MIT-licensed effects collection by Chris Johnson.
//
// MIT License
//
// Copyright (c) 2018 Chris Johnson
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! A gain stage into a soft wavefolding saturator.

use super::{denorm, dither32, pf, CRand, Sub};
use crate::kit::core::SlotDef;
use std::f64::consts::PI;

pub const PARAMS: &[SlotDef] = &[pf("Input", 0.5)];

pub struct Mojo {
    a: f32,
    fpd: [u32; 2],
}

impl Mojo {
    pub fn new(_sr: f64, rng: &mut CRand) -> Box<Mojo> {
        let l = rng.fpd();
        let r = rng.fpd();
        Box::new(Mojo { a: 0.5, fpd: [l, r] })
    }
}

impl Sub for Mojo {
    fn set_param(&mut self, i: usize, v: f32) {
        if i == 0 {
            self.a = v;
        }
    }

    fn process(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let gain = 10f64.powf(((self.a as f64 * 24.0) - 12.0) / 20.0);
        for (sl, sr) in out_l.iter_mut().zip(out_r.iter_mut()) {
            for (ch, s) in [sl, sr].into_iter().enumerate() {
                let mut x = denorm(*s as f64, self.fpd[ch]);
                if gain != 1.0 {
                    x *= gain;
                }
                let mojo = x.abs().powf(0.25);
                if mojo > 0.0 {
                    x = ((x * mojo * PI * 0.5).sin() / mojo) * 0.987654321;
                }
                *s = dither32(x, &mut self.fpd[ch]) as f32;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_curve() {
        // unity gain: y = sin(x |x|^0.25 pi/2) / |x|^0.25 * 0.987654321
        let mut m = Mojo::new(48000.0, &mut CRand::new(1));
        let mut l = [0.5f32, -0.25, 1.0, 0.0625];
        let mut r = l;
        let x0 = l;
        m.process(&mut l, &mut r);
        for i in 0..4 {
            let x = x0[i] as f64;
            let mj = x.abs().powf(0.25);
            let y = ((x * mj * PI * 0.5).sin() / mj) * 0.987654321;
            assert!((l[i] as f64 - y).abs() < 1e-6, "{} {}", l[i], y);
        }
        assert!((l[2] - 0.987654321).abs() < 1e-6);
    }
}
