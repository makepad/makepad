// Ported from Drive (DriveProc.cpp, Drive.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Highpassed, cascaded cubic soft-clip stages (more drive = more stages).

use super::{denorm, dither32, pf, CRand, Sub};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[pf("Drive", 0.0), pf("Highpass", 0.0), pf("Out Level", 1.0), pf("Dry/Wet", 1.0)];

pub struct Drive {
    p: [f32; 4],
    sr: f64,
    iir_a: [f64; 2],
    iir_b: [f64; 2],
    fp_flip: bool,
    fpd: [u32; 2],
}

impl Drive {
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Drive> {
        let l = rng.fpd();
        let r = rng.fpd();
        Box::new(Drive { p: [0.0, 0.0, 1.0, 1.0], sr, iir_a: [0.0; 2], iir_b: [0.0; 2], fp_flip: true, fpd: [l, r] })
    }
}

impl Sub for Drive {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    fn process(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let [a, b, c, d] = self.p.map(|v| v as f64);
        let mut overallscale = 1.0f64;
        overallscale /= 44100.0;
        overallscale *= self.sr;
        let driveone = (a * 2.0).powf(2.0);
        let iir_amount = b.powf(3.0) / overallscale;
        let output = c;
        let wet = d;
        let dry = 1.0 - wet;
        let glitch = 0.60;

        for (sl, sr) in out_l.iter_mut().zip(out_r.iter_mut()) {
            let mut x = [denorm(*sl as f64, self.fpd[0]), denorm(*sr as f64, self.fpd[1])];
            let drys = x;
            for ch in 0..2 {
                if self.fp_flip {
                    self.iir_a[ch] = (self.iir_a[ch] * (1.0 - iir_amount)) + (x[ch] * iir_amount);
                    x[ch] -= self.iir_a[ch];
                } else {
                    self.iir_b[ch] = (self.iir_b[ch] * (1.0 - iir_amount)) + (x[ch] * iir_amount);
                    x[ch] -= self.iir_b[ch];
                }
            }
            self.fp_flip = !self.fp_flip;
            for v in x.iter_mut() {
                if *v > 1.0 {
                    *v = 1.0;
                }
                if *v < -1.0 {
                    *v = -1.0;
                }
            }
            let mut out = driveone;
            while out > glitch {
                out -= glitch;
                for v in x.iter_mut() {
                    *v -= *v * (v.abs() * glitch) * (v.abs() * glitch);
                    *v *= 1.0 + glitch;
                }
            }
            for v in x.iter_mut() {
                *v -= *v * (v.abs() * out) * (v.abs() * out);
                *v *= 1.0 + out;
            }
            for ch in 0..2 {
                if output < 1.0 {
                    x[ch] *= output;
                }
                if wet < 1.0 {
                    x[ch] = (drys[ch] * dry) + (x[ch] * wet);
                }
                x[ch] = dither32(x[ch], &mut self.fpd[ch]);
            }
            *sl = x[0] as f32;
            *sr = x[1] as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_pass_signal_unchanged() {
        let mut d = Drive::new(48000.0, &mut CRand::new(1));
        let mut l = [0.5f32, -0.25, 0.125, 0.75];
        let mut r = [0.1f32, 0.2, -0.3, 0.4];
        let (l0, r0) = (l, r);
        d.process(&mut l, &mut r);
        for i in 0..4 {
            assert!((l[i] - l0[i]).abs() <= l0[i].abs() * 1e-6);
            assert!((r[i] - r0[i]).abs() <= r0[i].abs() * 1e-6);
        }
    }
}
