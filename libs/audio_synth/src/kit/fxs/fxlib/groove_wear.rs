// Ported from GrooveWear (GrooveWearProc.cpp, GrooveWear.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Groove Wear (sub-effect id 18): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Wear", 0.064),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct GrooveWear {
    p: [f32; 2],
    sr: f32,
    fpd_l: u32,
    fpd_r: u32,
    a_mid_l: [f64; 21],
    a_mid_prev_l: f64,
    b_mid_l: [f64; 21],
    b_mid_prev_l: f64,
    c_mid_l: [f64; 21],
    c_mid_prev_l: f64,
    d_mid_l: [f64; 21],
    d_mid_prev_l: f64,
    a_mid_r: [f64; 21],
    a_mid_prev_r: f64,
    b_mid_r: [f64; 21],
    b_mid_prev_r: f64,
    c_mid_r: [f64; 21],
    c_mid_prev_r: f64,
    d_mid_r: [f64; 21],
    d_mid_prev_r: f64,
    f_mid: [f64; 21],
}

impl GrooveWear {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<GrooveWear> {
        let mut s = Box::new(GrooveWear {
            p: [0.064, 1.0],
            sr: sr as f32,
            fpd_l: 0,
            fpd_r: 0,
            a_mid_l: [0.0; 21],
            a_mid_prev_l: 0.0,
            b_mid_l: [0.0; 21],
            b_mid_prev_l: 0.0,
            c_mid_l: [0.0; 21],
            c_mid_prev_l: 0.0,
            d_mid_l: [0.0; 21],
            d_mid_prev_l: 0.0,
            a_mid_r: [0.0; 21],
            a_mid_prev_r: 0.0,
            b_mid_r: [0.0; 21],
            b_mid_prev_r: 0.0,
            c_mid_r: [0.0; 21],
            c_mid_prev_r: 0.0,
            d_mid_r: [0.0; 21],
            d_mid_prev_r: 0.0,
            f_mid: [0.0; 21],
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for GrooveWear {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut overallscale: f64 = (s.p[0] as f64).powf(2.0) * 19.0 + 1.0;
        let mut gain: f64 = overallscale;
        if gain > 1.0 {
            s.f_mid[0] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[0] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[1] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[1] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[2] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[2] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[3] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[3] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[4] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[4] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[5] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[5] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[6] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[6] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[7] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[7] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[8] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[8] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[9] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[9] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[10] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[10] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[11] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[11] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[12] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[12] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[13] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[13] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[14] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[14] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[15] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[15] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[16] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[16] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[17] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[17] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[18] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[18] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[19] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[19] = gain;
            gain = 0.0;
        }
        if overallscale < 1.0 {
            overallscale = 1.0;
        }
        s.f_mid[0] /= overallscale;
        s.f_mid[1] /= overallscale;
        s.f_mid[2] /= overallscale;
        s.f_mid[3] /= overallscale;
        s.f_mid[4] /= overallscale;
        s.f_mid[5] /= overallscale;
        s.f_mid[6] /= overallscale;
        s.f_mid[7] /= overallscale;
        s.f_mid[8] /= overallscale;
        s.f_mid[9] /= overallscale;
        s.f_mid[10] /= overallscale;
        s.f_mid[11] /= overallscale;
        s.f_mid[12] /= overallscale;
        s.f_mid[13] /= overallscale;
        s.f_mid[14] /= overallscale;
        s.f_mid[15] /= overallscale;
        s.f_mid[16] /= overallscale;
        s.f_mid[17] /= overallscale;
        s.f_mid[18] /= overallscale;
        s.f_mid[19] /= overallscale;
        let mut accumulator_sample_l: f64 = 0.0;
        let mut correction_l: f64 = 0.0;
        let mut accumulator_sample_r: f64 = 0.0;
        let mut correction_r: f64 = 0.0;
        let mut a_wet: f64 = 1.0;
        let mut b_wet: f64 = 1.0;
        let mut c_wet: f64 = 1.0;
        let mut d_wet: f64 = s.p[1] as f64 * 4.0;
        if d_wet < 1.0 {
            a_wet = d_wet;
            b_wet = 0.0;
            c_wet = 0.0;
            d_wet = 0.0;
        } else if d_wet < 2.0 {
            b_wet = d_wet - 1.0;
            c_wet = 0.0;
            d_wet = 0.0;
        } else if d_wet < 3.0 {
            c_wet = d_wet - 2.0;
            d_wet = 0.0;
        } else {
            d_wet -= 3.0;
        }
        a_wet *= 0.5;
        b_wet *= 0.5;
        c_wet *= 0.5;
        d_wet *= 0.5;
        let mut a_dry: f64 = 1.0 - a_wet;
        let mut b_dry: f64 = 1.0 - b_wet;
        let mut c_dry: f64 = 1.0 - c_wet;
        let mut d_dry: f64 = 1.0 - d_wet;
        let mut dry_sample_l: f64 = 0.0;
        let mut dry_sample_r: f64 = 0.0;
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
            dry_sample_l = input_sample_l;
            dry_sample_r = input_sample_r;
            if a_wet > 0.0 {
                s.a_mid_l[19] = s.a_mid_l[18];
                s.a_mid_l[18] = s.a_mid_l[17];
                s.a_mid_l[17] = s.a_mid_l[16];
                s.a_mid_l[16] = s.a_mid_l[15];
                s.a_mid_l[15] = s.a_mid_l[14];
                s.a_mid_l[14] = s.a_mid_l[13];
                s.a_mid_l[13] = s.a_mid_l[12];
                s.a_mid_l[12] = s.a_mid_l[11];
                s.a_mid_l[11] = s.a_mid_l[10];
                s.a_mid_l[10] = s.a_mid_l[9];
                s.a_mid_l[9] = s.a_mid_l[8];
                s.a_mid_l[8] = s.a_mid_l[7];
                s.a_mid_l[7] = s.a_mid_l[6];
                s.a_mid_l[6] = s.a_mid_l[5];
                s.a_mid_l[5] = s.a_mid_l[4];
                s.a_mid_l[4] = s.a_mid_l[3];
                s.a_mid_l[3] = s.a_mid_l[2];
                s.a_mid_l[2] = s.a_mid_l[1];
                s.a_mid_l[1] = s.a_mid_l[0];
                accumulator_sample_l = input_sample_l - s.a_mid_prev_l;
                s.a_mid_l[0] = accumulator_sample_l;
                accumulator_sample_l *= s.f_mid[0];
                accumulator_sample_l += s.a_mid_l[1] * s.f_mid[1];
                accumulator_sample_l += s.a_mid_l[2] * s.f_mid[2];
                accumulator_sample_l += s.a_mid_l[3] * s.f_mid[3];
                accumulator_sample_l += s.a_mid_l[4] * s.f_mid[4];
                accumulator_sample_l += s.a_mid_l[5] * s.f_mid[5];
                accumulator_sample_l += s.a_mid_l[6] * s.f_mid[6];
                accumulator_sample_l += s.a_mid_l[7] * s.f_mid[7];
                accumulator_sample_l += s.a_mid_l[8] * s.f_mid[8];
                accumulator_sample_l += s.a_mid_l[9] * s.f_mid[9];
                accumulator_sample_l += s.a_mid_l[10] * s.f_mid[10];
                accumulator_sample_l += s.a_mid_l[11] * s.f_mid[11];
                accumulator_sample_l += s.a_mid_l[12] * s.f_mid[12];
                accumulator_sample_l += s.a_mid_l[13] * s.f_mid[13];
                accumulator_sample_l += s.a_mid_l[14] * s.f_mid[14];
                accumulator_sample_l += s.a_mid_l[15] * s.f_mid[15];
                accumulator_sample_l += s.a_mid_l[16] * s.f_mid[16];
                accumulator_sample_l += s.a_mid_l[17] * s.f_mid[17];
                accumulator_sample_l += s.a_mid_l[18] * s.f_mid[18];
                accumulator_sample_l += s.a_mid_l[19] * s.f_mid[19];
                correction_l = input_sample_l - s.a_mid_prev_l - accumulator_sample_l;
                s.a_mid_prev_l = input_sample_l;
                input_sample_l -= correction_l;
                input_sample_l = input_sample_l * a_wet + dry_sample_l * a_dry;
                dry_sample_l = input_sample_l;
                s.a_mid_r[19] = s.a_mid_r[18];
                s.a_mid_r[18] = s.a_mid_r[17];
                s.a_mid_r[17] = s.a_mid_r[16];
                s.a_mid_r[16] = s.a_mid_r[15];
                s.a_mid_r[15] = s.a_mid_r[14];
                s.a_mid_r[14] = s.a_mid_r[13];
                s.a_mid_r[13] = s.a_mid_r[12];
                s.a_mid_r[12] = s.a_mid_r[11];
                s.a_mid_r[11] = s.a_mid_r[10];
                s.a_mid_r[10] = s.a_mid_r[9];
                s.a_mid_r[9] = s.a_mid_r[8];
                s.a_mid_r[8] = s.a_mid_r[7];
                s.a_mid_r[7] = s.a_mid_r[6];
                s.a_mid_r[6] = s.a_mid_r[5];
                s.a_mid_r[5] = s.a_mid_r[4];
                s.a_mid_r[4] = s.a_mid_r[3];
                s.a_mid_r[3] = s.a_mid_r[2];
                s.a_mid_r[2] = s.a_mid_r[1];
                s.a_mid_r[1] = s.a_mid_r[0];
                accumulator_sample_r = input_sample_r - s.a_mid_prev_r;
                s.a_mid_r[0] = accumulator_sample_r;
                accumulator_sample_r *= s.f_mid[0];
                accumulator_sample_r += s.a_mid_r[1] * s.f_mid[1];
                accumulator_sample_r += s.a_mid_r[2] * s.f_mid[2];
                accumulator_sample_r += s.a_mid_r[3] * s.f_mid[3];
                accumulator_sample_r += s.a_mid_r[4] * s.f_mid[4];
                accumulator_sample_r += s.a_mid_r[5] * s.f_mid[5];
                accumulator_sample_r += s.a_mid_r[6] * s.f_mid[6];
                accumulator_sample_r += s.a_mid_r[7] * s.f_mid[7];
                accumulator_sample_r += s.a_mid_r[8] * s.f_mid[8];
                accumulator_sample_r += s.a_mid_r[9] * s.f_mid[9];
                accumulator_sample_r += s.a_mid_r[10] * s.f_mid[10];
                accumulator_sample_r += s.a_mid_r[11] * s.f_mid[11];
                accumulator_sample_r += s.a_mid_r[12] * s.f_mid[12];
                accumulator_sample_r += s.a_mid_r[13] * s.f_mid[13];
                accumulator_sample_r += s.a_mid_r[14] * s.f_mid[14];
                accumulator_sample_r += s.a_mid_r[15] * s.f_mid[15];
                accumulator_sample_r += s.a_mid_r[16] * s.f_mid[16];
                accumulator_sample_r += s.a_mid_r[17] * s.f_mid[17];
                accumulator_sample_r += s.a_mid_r[18] * s.f_mid[18];
                accumulator_sample_r += s.a_mid_r[19] * s.f_mid[19];
                correction_r = input_sample_r - s.a_mid_prev_r - accumulator_sample_r;
                s.a_mid_prev_r = input_sample_r;
                input_sample_r -= correction_r;
                input_sample_r = input_sample_r * a_wet + dry_sample_r * a_dry;
                dry_sample_r = input_sample_r;
            }
            if b_wet > 0.0 {
                s.b_mid_l[19] = s.b_mid_l[18];
                s.b_mid_l[18] = s.b_mid_l[17];
                s.b_mid_l[17] = s.b_mid_l[16];
                s.b_mid_l[16] = s.b_mid_l[15];
                s.b_mid_l[15] = s.b_mid_l[14];
                s.b_mid_l[14] = s.b_mid_l[13];
                s.b_mid_l[13] = s.b_mid_l[12];
                s.b_mid_l[12] = s.b_mid_l[11];
                s.b_mid_l[11] = s.b_mid_l[10];
                s.b_mid_l[10] = s.b_mid_l[9];
                s.b_mid_l[9] = s.b_mid_l[8];
                s.b_mid_l[8] = s.b_mid_l[7];
                s.b_mid_l[7] = s.b_mid_l[6];
                s.b_mid_l[6] = s.b_mid_l[5];
                s.b_mid_l[5] = s.b_mid_l[4];
                s.b_mid_l[4] = s.b_mid_l[3];
                s.b_mid_l[3] = s.b_mid_l[2];
                s.b_mid_l[2] = s.b_mid_l[1];
                s.b_mid_l[1] = s.b_mid_l[0];
                accumulator_sample_l = input_sample_l - s.b_mid_prev_l;
                s.b_mid_l[0] = accumulator_sample_l;
                accumulator_sample_l *= s.f_mid[0];
                accumulator_sample_l += s.b_mid_l[1] * s.f_mid[1];
                accumulator_sample_l += s.b_mid_l[2] * s.f_mid[2];
                accumulator_sample_l += s.b_mid_l[3] * s.f_mid[3];
                accumulator_sample_l += s.b_mid_l[4] * s.f_mid[4];
                accumulator_sample_l += s.b_mid_l[5] * s.f_mid[5];
                accumulator_sample_l += s.b_mid_l[6] * s.f_mid[6];
                accumulator_sample_l += s.b_mid_l[7] * s.f_mid[7];
                accumulator_sample_l += s.b_mid_l[8] * s.f_mid[8];
                accumulator_sample_l += s.b_mid_l[9] * s.f_mid[9];
                accumulator_sample_l += s.b_mid_l[10] * s.f_mid[10];
                accumulator_sample_l += s.b_mid_l[11] * s.f_mid[11];
                accumulator_sample_l += s.b_mid_l[12] * s.f_mid[12];
                accumulator_sample_l += s.b_mid_l[13] * s.f_mid[13];
                accumulator_sample_l += s.b_mid_l[14] * s.f_mid[14];
                accumulator_sample_l += s.b_mid_l[15] * s.f_mid[15];
                accumulator_sample_l += s.b_mid_l[16] * s.f_mid[16];
                accumulator_sample_l += s.b_mid_l[17] * s.f_mid[17];
                accumulator_sample_l += s.b_mid_l[18] * s.f_mid[18];
                accumulator_sample_l += s.b_mid_l[19] * s.f_mid[19];
                correction_l = input_sample_l - s.b_mid_prev_l - accumulator_sample_l;
                s.b_mid_prev_l = input_sample_l;
                input_sample_l -= correction_l;
                input_sample_l = input_sample_l * b_wet + dry_sample_l * b_dry;
                dry_sample_l = input_sample_l;
                s.b_mid_r[19] = s.b_mid_r[18];
                s.b_mid_r[18] = s.b_mid_r[17];
                s.b_mid_r[17] = s.b_mid_r[16];
                s.b_mid_r[16] = s.b_mid_r[15];
                s.b_mid_r[15] = s.b_mid_r[14];
                s.b_mid_r[14] = s.b_mid_r[13];
                s.b_mid_r[13] = s.b_mid_r[12];
                s.b_mid_r[12] = s.b_mid_r[11];
                s.b_mid_r[11] = s.b_mid_r[10];
                s.b_mid_r[10] = s.b_mid_r[9];
                s.b_mid_r[9] = s.b_mid_r[8];
                s.b_mid_r[8] = s.b_mid_r[7];
                s.b_mid_r[7] = s.b_mid_r[6];
                s.b_mid_r[6] = s.b_mid_r[5];
                s.b_mid_r[5] = s.b_mid_r[4];
                s.b_mid_r[4] = s.b_mid_r[3];
                s.b_mid_r[3] = s.b_mid_r[2];
                s.b_mid_r[2] = s.b_mid_r[1];
                s.b_mid_r[1] = s.b_mid_r[0];
                accumulator_sample_r = input_sample_r - s.b_mid_prev_r;
                s.b_mid_r[0] = accumulator_sample_r;
                accumulator_sample_r *= s.f_mid[0];
                accumulator_sample_r += s.b_mid_r[1] * s.f_mid[1];
                accumulator_sample_r += s.b_mid_r[2] * s.f_mid[2];
                accumulator_sample_r += s.b_mid_r[3] * s.f_mid[3];
                accumulator_sample_r += s.b_mid_r[4] * s.f_mid[4];
                accumulator_sample_r += s.b_mid_r[5] * s.f_mid[5];
                accumulator_sample_r += s.b_mid_r[6] * s.f_mid[6];
                accumulator_sample_r += s.b_mid_r[7] * s.f_mid[7];
                accumulator_sample_r += s.b_mid_r[8] * s.f_mid[8];
                accumulator_sample_r += s.b_mid_r[9] * s.f_mid[9];
                accumulator_sample_r += s.b_mid_r[10] * s.f_mid[10];
                accumulator_sample_r += s.b_mid_r[11] * s.f_mid[11];
                accumulator_sample_r += s.b_mid_r[12] * s.f_mid[12];
                accumulator_sample_r += s.b_mid_r[13] * s.f_mid[13];
                accumulator_sample_r += s.b_mid_r[14] * s.f_mid[14];
                accumulator_sample_r += s.b_mid_r[15] * s.f_mid[15];
                accumulator_sample_r += s.b_mid_r[16] * s.f_mid[16];
                accumulator_sample_r += s.b_mid_r[17] * s.f_mid[17];
                accumulator_sample_r += s.b_mid_r[18] * s.f_mid[18];
                accumulator_sample_r += s.b_mid_r[19] * s.f_mid[19];
                correction_r = input_sample_r - s.b_mid_prev_r - accumulator_sample_r;
                s.b_mid_prev_r = input_sample_r;
                input_sample_r -= correction_r;
                input_sample_r = input_sample_r * b_wet + dry_sample_r * b_dry;
                dry_sample_r = input_sample_r;
            }
            if c_wet > 0.0 {
                s.c_mid_l[19] = s.c_mid_l[18];
                s.c_mid_l[18] = s.c_mid_l[17];
                s.c_mid_l[17] = s.c_mid_l[16];
                s.c_mid_l[16] = s.c_mid_l[15];
                s.c_mid_l[15] = s.c_mid_l[14];
                s.c_mid_l[14] = s.c_mid_l[13];
                s.c_mid_l[13] = s.c_mid_l[12];
                s.c_mid_l[12] = s.c_mid_l[11];
                s.c_mid_l[11] = s.c_mid_l[10];
                s.c_mid_l[10] = s.c_mid_l[9];
                s.c_mid_l[9] = s.c_mid_l[8];
                s.c_mid_l[8] = s.c_mid_l[7];
                s.c_mid_l[7] = s.c_mid_l[6];
                s.c_mid_l[6] = s.c_mid_l[5];
                s.c_mid_l[5] = s.c_mid_l[4];
                s.c_mid_l[4] = s.c_mid_l[3];
                s.c_mid_l[3] = s.c_mid_l[2];
                s.c_mid_l[2] = s.c_mid_l[1];
                s.c_mid_l[1] = s.c_mid_l[0];
                accumulator_sample_l = input_sample_l - s.c_mid_prev_l;
                s.c_mid_l[0] = accumulator_sample_l;
                accumulator_sample_l *= s.f_mid[0];
                accumulator_sample_l += s.c_mid_l[1] * s.f_mid[1];
                accumulator_sample_l += s.c_mid_l[2] * s.f_mid[2];
                accumulator_sample_l += s.c_mid_l[3] * s.f_mid[3];
                accumulator_sample_l += s.c_mid_l[4] * s.f_mid[4];
                accumulator_sample_l += s.c_mid_l[5] * s.f_mid[5];
                accumulator_sample_l += s.c_mid_l[6] * s.f_mid[6];
                accumulator_sample_l += s.c_mid_l[7] * s.f_mid[7];
                accumulator_sample_l += s.c_mid_l[8] * s.f_mid[8];
                accumulator_sample_l += s.c_mid_l[9] * s.f_mid[9];
                accumulator_sample_l += s.c_mid_l[10] * s.f_mid[10];
                accumulator_sample_l += s.c_mid_l[11] * s.f_mid[11];
                accumulator_sample_l += s.c_mid_l[12] * s.f_mid[12];
                accumulator_sample_l += s.c_mid_l[13] * s.f_mid[13];
                accumulator_sample_l += s.c_mid_l[14] * s.f_mid[14];
                accumulator_sample_l += s.c_mid_l[15] * s.f_mid[15];
                accumulator_sample_l += s.c_mid_l[16] * s.f_mid[16];
                accumulator_sample_l += s.c_mid_l[17] * s.f_mid[17];
                accumulator_sample_l += s.c_mid_l[18] * s.f_mid[18];
                accumulator_sample_l += s.c_mid_l[19] * s.f_mid[19];
                correction_l = input_sample_l - s.c_mid_prev_l - accumulator_sample_l;
                s.c_mid_prev_l = input_sample_l;
                input_sample_l -= correction_l;
                input_sample_l = input_sample_l * c_wet + dry_sample_l * c_dry;
                dry_sample_l = input_sample_l;
                s.c_mid_r[19] = s.c_mid_r[18];
                s.c_mid_r[18] = s.c_mid_r[17];
                s.c_mid_r[17] = s.c_mid_r[16];
                s.c_mid_r[16] = s.c_mid_r[15];
                s.c_mid_r[15] = s.c_mid_r[14];
                s.c_mid_r[14] = s.c_mid_r[13];
                s.c_mid_r[13] = s.c_mid_r[12];
                s.c_mid_r[12] = s.c_mid_r[11];
                s.c_mid_r[11] = s.c_mid_r[10];
                s.c_mid_r[10] = s.c_mid_r[9];
                s.c_mid_r[9] = s.c_mid_r[8];
                s.c_mid_r[8] = s.c_mid_r[7];
                s.c_mid_r[7] = s.c_mid_r[6];
                s.c_mid_r[6] = s.c_mid_r[5];
                s.c_mid_r[5] = s.c_mid_r[4];
                s.c_mid_r[4] = s.c_mid_r[3];
                s.c_mid_r[3] = s.c_mid_r[2];
                s.c_mid_r[2] = s.c_mid_r[1];
                s.c_mid_r[1] = s.c_mid_r[0];
                accumulator_sample_r = input_sample_r - s.c_mid_prev_r;
                s.c_mid_r[0] = accumulator_sample_r;
                accumulator_sample_r *= s.f_mid[0];
                accumulator_sample_r += s.c_mid_r[1] * s.f_mid[1];
                accumulator_sample_r += s.c_mid_r[2] * s.f_mid[2];
                accumulator_sample_r += s.c_mid_r[3] * s.f_mid[3];
                accumulator_sample_r += s.c_mid_r[4] * s.f_mid[4];
                accumulator_sample_r += s.c_mid_r[5] * s.f_mid[5];
                accumulator_sample_r += s.c_mid_r[6] * s.f_mid[6];
                accumulator_sample_r += s.c_mid_r[7] * s.f_mid[7];
                accumulator_sample_r += s.c_mid_r[8] * s.f_mid[8];
                accumulator_sample_r += s.c_mid_r[9] * s.f_mid[9];
                accumulator_sample_r += s.c_mid_r[10] * s.f_mid[10];
                accumulator_sample_r += s.c_mid_r[11] * s.f_mid[11];
                accumulator_sample_r += s.c_mid_r[12] * s.f_mid[12];
                accumulator_sample_r += s.c_mid_r[13] * s.f_mid[13];
                accumulator_sample_r += s.c_mid_r[14] * s.f_mid[14];
                accumulator_sample_r += s.c_mid_r[15] * s.f_mid[15];
                accumulator_sample_r += s.c_mid_r[16] * s.f_mid[16];
                accumulator_sample_r += s.c_mid_r[17] * s.f_mid[17];
                accumulator_sample_r += s.c_mid_r[18] * s.f_mid[18];
                accumulator_sample_r += s.c_mid_r[19] * s.f_mid[19];
                correction_r = input_sample_r - s.c_mid_prev_r - accumulator_sample_r;
                s.c_mid_prev_r = input_sample_r;
                input_sample_r -= correction_r;
                input_sample_r = input_sample_r * c_wet + dry_sample_r * c_dry;
                dry_sample_r = input_sample_r;
            }
            if d_wet > 0.0 {
                s.d_mid_l[19] = s.d_mid_l[18];
                s.d_mid_l[18] = s.d_mid_l[17];
                s.d_mid_l[17] = s.d_mid_l[16];
                s.d_mid_l[16] = s.d_mid_l[15];
                s.d_mid_l[15] = s.d_mid_l[14];
                s.d_mid_l[14] = s.d_mid_l[13];
                s.d_mid_l[13] = s.d_mid_l[12];
                s.d_mid_l[12] = s.d_mid_l[11];
                s.d_mid_l[11] = s.d_mid_l[10];
                s.d_mid_l[10] = s.d_mid_l[9];
                s.d_mid_l[9] = s.d_mid_l[8];
                s.d_mid_l[8] = s.d_mid_l[7];
                s.d_mid_l[7] = s.d_mid_l[6];
                s.d_mid_l[6] = s.d_mid_l[5];
                s.d_mid_l[5] = s.d_mid_l[4];
                s.d_mid_l[4] = s.d_mid_l[3];
                s.d_mid_l[3] = s.d_mid_l[2];
                s.d_mid_l[2] = s.d_mid_l[1];
                s.d_mid_l[1] = s.d_mid_l[0];
                accumulator_sample_l = input_sample_l - s.d_mid_prev_l;
                s.d_mid_l[0] = accumulator_sample_l;
                accumulator_sample_l *= s.f_mid[0];
                accumulator_sample_l += s.d_mid_l[1] * s.f_mid[1];
                accumulator_sample_l += s.d_mid_l[2] * s.f_mid[2];
                accumulator_sample_l += s.d_mid_l[3] * s.f_mid[3];
                accumulator_sample_l += s.d_mid_l[4] * s.f_mid[4];
                accumulator_sample_l += s.d_mid_l[5] * s.f_mid[5];
                accumulator_sample_l += s.d_mid_l[6] * s.f_mid[6];
                accumulator_sample_l += s.d_mid_l[7] * s.f_mid[7];
                accumulator_sample_l += s.d_mid_l[8] * s.f_mid[8];
                accumulator_sample_l += s.d_mid_l[9] * s.f_mid[9];
                accumulator_sample_l += s.d_mid_l[10] * s.f_mid[10];
                accumulator_sample_l += s.d_mid_l[11] * s.f_mid[11];
                accumulator_sample_l += s.d_mid_l[12] * s.f_mid[12];
                accumulator_sample_l += s.d_mid_l[13] * s.f_mid[13];
                accumulator_sample_l += s.d_mid_l[14] * s.f_mid[14];
                accumulator_sample_l += s.d_mid_l[15] * s.f_mid[15];
                accumulator_sample_l += s.d_mid_l[16] * s.f_mid[16];
                accumulator_sample_l += s.d_mid_l[17] * s.f_mid[17];
                accumulator_sample_l += s.d_mid_l[18] * s.f_mid[18];
                accumulator_sample_l += s.d_mid_l[19] * s.f_mid[19];
                correction_l = input_sample_l - s.d_mid_prev_l - accumulator_sample_l;
                s.d_mid_prev_l = input_sample_l;
                input_sample_l -= correction_l;
                input_sample_l = input_sample_l * d_wet + dry_sample_l * d_dry;
                dry_sample_l = input_sample_l;
                s.d_mid_r[19] = s.d_mid_r[18];
                s.d_mid_r[18] = s.d_mid_r[17];
                s.d_mid_r[17] = s.d_mid_r[16];
                s.d_mid_r[16] = s.d_mid_r[15];
                s.d_mid_r[15] = s.d_mid_r[14];
                s.d_mid_r[14] = s.d_mid_r[13];
                s.d_mid_r[13] = s.d_mid_r[12];
                s.d_mid_r[12] = s.d_mid_r[11];
                s.d_mid_r[11] = s.d_mid_r[10];
                s.d_mid_r[10] = s.d_mid_r[9];
                s.d_mid_r[9] = s.d_mid_r[8];
                s.d_mid_r[8] = s.d_mid_r[7];
                s.d_mid_r[7] = s.d_mid_r[6];
                s.d_mid_r[6] = s.d_mid_r[5];
                s.d_mid_r[5] = s.d_mid_r[4];
                s.d_mid_r[4] = s.d_mid_r[3];
                s.d_mid_r[3] = s.d_mid_r[2];
                s.d_mid_r[2] = s.d_mid_r[1];
                s.d_mid_r[1] = s.d_mid_r[0];
                accumulator_sample_r = input_sample_r - s.d_mid_prev_r;
                s.d_mid_r[0] = accumulator_sample_r;
                accumulator_sample_r *= s.f_mid[0];
                accumulator_sample_r += s.d_mid_r[1] * s.f_mid[1];
                accumulator_sample_r += s.d_mid_r[2] * s.f_mid[2];
                accumulator_sample_r += s.d_mid_r[3] * s.f_mid[3];
                accumulator_sample_r += s.d_mid_r[4] * s.f_mid[4];
                accumulator_sample_r += s.d_mid_r[5] * s.f_mid[5];
                accumulator_sample_r += s.d_mid_r[6] * s.f_mid[6];
                accumulator_sample_r += s.d_mid_r[7] * s.f_mid[7];
                accumulator_sample_r += s.d_mid_r[8] * s.f_mid[8];
                accumulator_sample_r += s.d_mid_r[9] * s.f_mid[9];
                accumulator_sample_r += s.d_mid_r[10] * s.f_mid[10];
                accumulator_sample_r += s.d_mid_r[11] * s.f_mid[11];
                accumulator_sample_r += s.d_mid_r[12] * s.f_mid[12];
                accumulator_sample_r += s.d_mid_r[13] * s.f_mid[13];
                accumulator_sample_r += s.d_mid_r[14] * s.f_mid[14];
                accumulator_sample_r += s.d_mid_r[15] * s.f_mid[15];
                accumulator_sample_r += s.d_mid_r[16] * s.f_mid[16];
                accumulator_sample_r += s.d_mid_r[17] * s.f_mid[17];
                accumulator_sample_r += s.d_mid_r[18] * s.f_mid[18];
                accumulator_sample_r += s.d_mid_r[19] * s.f_mid[19];
                correction_r = input_sample_r - s.d_mid_prev_r - accumulator_sample_r;
                s.d_mid_prev_r = input_sample_r;
                input_sample_r -= correction_r;
                input_sample_r = input_sample_r * d_wet + dry_sample_r * d_dry;
                dry_sample_r = input_sample_r;
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
