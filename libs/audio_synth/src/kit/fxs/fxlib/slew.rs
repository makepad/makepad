// Ported from Slew (SlewProc.cpp, Slew.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Slew 1 (sub-effect id 47): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Clamping", 0.0),
];

#[allow(dead_code)]
pub struct Slew {
    p: [f32; 1],
    sr: f32,
    last_sample_l: f64,
    last_sample_r: f64,
    fpd_l: u32,
    fpd_r: u32,
}

impl Slew {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Slew> {
        let mut s = Box::new(Slew {
            p: [0.0],
            sr: sr as f32,
            last_sample_l: 0.0,
            last_sample_r: 0.0,
            fpd_l: 0,
            fpd_r: 0,
        });
        s
    }
}

impl Sub for Slew {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut overallscale: f64 = 1.0;
        overallscale /= 44100.0;
        overallscale *= s.sr as f64;
        let mut input_sample_l: f64 = 0.0;
        let mut input_sample_r: f64 = 0.0;
        let mut output_sample_l: f64 = 0.0;
        let mut output_sample_r: f64 = 0.0;
        let mut clamp: f64 = 0.0;
        let mut threshold: f64 = ((1.0 - s.p[0]) as f64).powf(4.0) / overallscale;
        for smp in 0..l.len() {
            input_sample_l = l[smp] as f64;
            input_sample_r = r[smp] as f64;
            if input_sample_l.abs() < 1.18e-23 {
                input_sample_l = s.fpd_l as f64 * 1.18e-17;
            }
            if input_sample_r.abs() < 1.18e-23 {
                input_sample_r = s.fpd_r as f64 * 1.18e-17;
            }
            clamp = input_sample_l - s.last_sample_l;
            output_sample_l = input_sample_l;
            if clamp > threshold {
                output_sample_l = s.last_sample_l + threshold;
            }
            if -clamp > threshold {
                output_sample_l = s.last_sample_l - threshold;
            }
            s.last_sample_l = output_sample_l;
            clamp = input_sample_r - s.last_sample_r;
            output_sample_r = input_sample_r;
            if clamp > threshold {
                output_sample_r = s.last_sample_r + threshold;
            }
            if -clamp > threshold {
                output_sample_r = s.last_sample_r - threshold;
            }
            s.last_sample_r = output_sample_r;
            l[smp] = output_sample_l as f32;
            r[smp] = output_sample_r as f32;
        }
    }
}
