// Ported from TapeDust (TapeDustProc.cpp, TapeDust.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Tape Dust (sub-effect id 67): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Dust", 0.0),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct TapeDust {
    p: [f32; 2],
    sr: f32,
    rand: CRand,
    fpd_l: u32,
    fpd_r: u32,
    fp_flip: bool,
    b_l: [f64; 11],
    f_l: [f64; 11],
    b_r: [f64; 11],
    f_r: [f64; 11],
}

impl TapeDust {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<TapeDust> {
        let mut s = Box::new(TapeDust {
            p: [0.0, 1.0],
            sr: sr as f32,
            rand: rng.fork(),
            fpd_l: 0,
            fpd_r: 0,
            fp_flip: false,
            b_l: [0.0; 11],
            f_l: [0.0; 11],
            b_r: [0.0; 11],
            f_r: [0.0; 11],
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s.fp_flip = true;
        s
    }
}

impl Sub for TapeDust {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut input_sample_l: f64 = 0.0;
        let mut input_sample_r: f64 = 0.0;
        let mut dry_sample_l: f64 = 0.0;
        let mut dry_sample_r: f64 = 0.0;
        let mut r_range: f64 = (s.p[0] as f64).powf(2.0) * 5.0;
        let mut xfuzz: f64 = r_range * 0.002;
        let mut r_offset: f64 = r_range * 0.4 + 1.0;
        let mut r_depth_l: f64 = 0.0;
        let mut r_depth_r: f64 = 0.0;
        let mut gain_l: f64 = 0.0;
        let mut gain_r: f64 = 0.0;
        let mut wet: f64 = s.p[1] as f64;
        for smp in 0..l.len() {
            input_sample_l = l[smp] as f64;
            input_sample_r = r[smp] as f64;
            if input_sample_l.abs() < 1.18e-23 {
                input_sample_l = s.fpd_l as f64 * 1.18e-17;
            }
            if input_sample_r.abs() < 1.18e-23 {
                input_sample_r = s.fpd_r as f64 * 1.18e-17;
            }
            dry_sample_l = input_sample_l;
            dry_sample_r = input_sample_r;
            {
                let mut count: i32 = 9;
                while count < 0 {
                    s.b_l[(count + 1) as usize] = s.b_l[count as usize];
                    s.b_r[(count + 1) as usize] = s.b_r[count as usize];
                    count -= 1;
                }
            }
            s.b_l[0] = input_sample_l;
            s.b_r[0] = input_sample_r;
            input_sample_l = s.rand.next() as i32 as f64 / 2147483647.0;
            input_sample_r = s.rand.next() as i32 as f64 / 2147483647.0;
            r_depth_l = input_sample_l * r_range + r_offset;
            gain_l = r_depth_l;
            r_depth_r = input_sample_r * r_range + r_offset;
            gain_r = r_depth_r;
            input_sample_l *= (1.0 - (s.b_l[0] - s.b_l[1]).abs()) * xfuzz;
            input_sample_r *= (1.0 - (s.b_r[0] - s.b_r[1]).abs()) * xfuzz;
            if s.fp_flip {
                input_sample_l = -input_sample_l;
                input_sample_r = -input_sample_r;
            }
            s.fp_flip = !s.fp_flip;
            {
                let mut count: i32 = 0;
                while count < 9 {
                    if gain_l > 1.0 {
                        s.f_l[count as usize] = 1.0;
                        gain_l -= 1.0;
                    } else {
                        s.f_l[count as usize] = gain_l;
                        gain_l = 0.0;
                    }
                    if gain_r > 1.0 {
                        s.f_r[count as usize] = 1.0;
                        gain_r -= 1.0;
                    } else {
                        s.f_r[count as usize] = gain_r;
                        gain_r = 0.0;
                    }
                    s.f_l[count as usize] /= r_depth_l;
                    s.f_r[count as usize] /= r_depth_r;
                    input_sample_l += s.b_l[count as usize] * s.f_l[count as usize];
                    input_sample_r += s.b_r[count as usize] * s.f_r[count as usize];
                    count += 1;
                }
            }
            if wet < 1.0 {
                input_sample_l = input_sample_l * wet + dry_sample_l * (1.0 - wet);
                input_sample_r = input_sample_r * wet + dry_sample_r * (1.0 - wet);
            }
            let mut expon: i32 = 0;
            expon = frexpf_exp(input_sample_l as f32);
            s.fpd_l ^= s.fpd_l << 13;
            s.fpd_l ^= s.fpd_l >> 17;
            s.fpd_l ^= s.fpd_l << 5;
            input_sample_l += (s.fpd_l as f64 - 2147483647.0) * 5.5e-36 * 2.0_f64.powf((expon + 62) as f64);
            expon = frexpf_exp(input_sample_r as f32);
            s.fpd_r ^= s.fpd_r << 13;
            s.fpd_r ^= s.fpd_r >> 17;
            s.fpd_r ^= s.fpd_r << 5;
            input_sample_r += (s.fpd_r as f64 - 2147483647.0) * 5.5e-36 * 2.0_f64.powf((expon + 62) as f64);
            l[smp] = input_sample_l as f32;
            r[smp] = input_sample_r as f32;
        }
    }
}
