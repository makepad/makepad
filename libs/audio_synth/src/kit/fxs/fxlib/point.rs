// Ported from Point (PointProc.cpp, Point.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Point (sub-effect id 7): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Input Trim", 0.5),
    pf("Point", 0.5),
    pf("Reaction Speed", 0.5),
];

#[allow(dead_code)]
pub struct Point {
    p: [f32; 3],
    sr: f32,
    fpd_l: u32,
    fpd_r: u32,
    fp_flip: bool,
    nib_al: f64,
    nob_al: f64,
    nib_bl: f64,
    nob_bl: f64,
    nib_ar: f64,
    nob_ar: f64,
    nib_br: f64,
    nob_br: f64,
}

impl Point {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Point> {
        let mut s = Box::new(Point {
            p: [0.5, 0.5, 0.5],
            sr: sr as f32,
            fpd_l: 0,
            fpd_r: 0,
            fp_flip: false,
            nib_al: 0.0,
            nob_al: 0.0,
            nib_bl: 0.0,
            nob_bl: 0.0,
            nib_ar: 0.0,
            nob_ar: 0.0,
            nib_br: 0.0,
            nob_br: 0.0,
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s.fp_flip = true;
        s
    }
}

impl Sub for Point {
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
        let mut gaintrim: f64 = 10.0_f64.powf((s.p[0] as f64 * 24.0 - 12.0) / 20.0);
        let mut nib_div: f64 = 1.0 / (s.p[2] as f64 + 0.2).powf(7.0);
        nib_div /= overallscale;
        let mut nob_div: f64 = 0.0;
        if s.p[1] as f64 * 2.0 - 1.0 > 0.0 {
            nob_div = nib_div / (1.001 - (s.p[1] as f64 * 2.0 - 1.0));
        } else {
            nob_div = nib_div * (1.001 - ((s.p[1] as f64 * 2.0 - 1.0) * 0.75).powf(2.0));
        }
        let mut nibnob_factor: f64 = 0.0;
        let mut absolute: f64 = 0.0;
        let mut input_sample_l: f64 = 0.0;
        let mut input_sample_r: f64 = 0.0;
        for smp in 0..l.len() {
            input_sample_l = l[smp] as f64;
            input_sample_r = r[smp] as f64;
            if input_sample_l.abs() < 1.18e-23 {
                input_sample_l = s.fpd_l as f64 * 1.18e-17;
            }
            if input_sample_r.abs() < 1.18e-23 {
                input_sample_r = s.fpd_r as f64 * 1.18e-17;
            }
            input_sample_l *= gaintrim;
            absolute = input_sample_l.abs();
            if s.fp_flip {
                s.nib_al = s.nib_al + absolute / nib_div;
                s.nib_al = s.nib_al / (1.0 + 1.0 / nib_div);
                s.nob_al = s.nob_al + absolute / nob_div;
                s.nob_al = s.nob_al / (1.0 + 1.0 / nob_div);
                if s.nob_al > 0.0 {
                    nibnob_factor = s.nib_al / s.nob_al;
                }
            } else {
                s.nib_bl = s.nib_bl + absolute / nib_div;
                s.nib_bl = s.nib_bl / (1.0 + 1.0 / nib_div);
                s.nob_bl = s.nob_bl + absolute / nob_div;
                s.nob_bl = s.nob_bl / (1.0 + 1.0 / nob_div);
                if s.nob_bl > 0.0 {
                    nibnob_factor = s.nib_bl / s.nob_bl;
                }
            }
            input_sample_l *= nibnob_factor;
            input_sample_r *= gaintrim;
            absolute = input_sample_r.abs();
            if s.fp_flip {
                s.nib_ar = s.nib_ar + absolute / nib_div;
                s.nib_ar = s.nib_ar / (1.0 + 1.0 / nib_div);
                s.nob_ar = s.nob_ar + absolute / nob_div;
                s.nob_ar = s.nob_ar / (1.0 + 1.0 / nob_div);
                if s.nob_ar > 0.0 {
                    nibnob_factor = s.nib_ar / s.nob_ar;
                }
            } else {
                s.nib_br = s.nib_br + absolute / nib_div;
                s.nib_br = s.nib_br / (1.0 + 1.0 / nib_div);
                s.nob_br = s.nob_br + absolute / nob_div;
                s.nob_br = s.nob_br / (1.0 + 1.0 / nob_div);
                if s.nob_br > 0.0 {
                    nibnob_factor = s.nib_br / s.nob_br;
                }
            }
            input_sample_r *= nibnob_factor;
            s.fp_flip = !s.fp_flip;
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
