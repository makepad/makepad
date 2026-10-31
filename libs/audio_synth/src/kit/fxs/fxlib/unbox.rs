// Ported from UnBox (UnBoxProc.cpp, UnBox.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Unbox (sub-effect id 37): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Input", 0.5),
    pf("UnBox", 0.0),
    pf("Output", 0.5),
];

#[allow(dead_code)]
pub struct UnBox {
    p: [f32; 3],
    sr: f32,
    fpd_l: u32,
    fpd_r: u32,
    a_l: [f64; 5],
    b_l: [f64; 5],
    c_l: [f64; 11],
    a_r: [f64; 5],
    b_r: [f64; 5],
    c_r: [f64; 11],
    e: [f64; 5],
    f: [f64; 11],
    iir_sample_al: f64,
    iir_sample_bl: f64,
    iir_sample_ar: f64,
    iir_sample_br: f64,
}

impl UnBox {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<UnBox> {
        let mut s = Box::new(UnBox {
            p: [0.5, 0.0, 0.5],
            sr: sr as f32,
            fpd_l: 0,
            fpd_r: 0,
            a_l: [0.0; 5],
            b_l: [0.0; 5],
            c_l: [0.0; 11],
            a_r: [0.0; 5],
            b_r: [0.0; 5],
            c_r: [0.0; 11],
            e: [0.0; 5],
            f: [0.0; 11],
            iir_sample_al: 0.0,
            iir_sample_bl: 0.0,
            iir_sample_ar: 0.0,
            iir_sample_br: 0.0,
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for UnBox {
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
        let mut input: f64 = s.p[0] as f64 * 2.0;
        let mut unbox: f64 = s.p[1] as f64 + 1.0;
        unbox *= unbox;
        let mut iir_amount: f64 = unbox * 0.00052 / overallscale;
        let mut output: f64 = s.p[2] as f64 * 2.0;
        let mut treble: f64 = unbox;
        let mut gain: f64 = treble;
        if gain > 1.0 {
            s.e[0] = 1.0;
            gain -= 1.0;
        } else {
            s.e[0] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.e[1] = 1.0;
            gain -= 1.0;
        } else {
            s.e[1] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.e[2] = 1.0;
            gain -= 1.0;
        } else {
            s.e[2] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.e[3] = 1.0;
            gain -= 1.0;
        } else {
            s.e[3] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.e[4] = 1.0;
            gain -= 1.0;
        } else {
            s.e[4] = gain;
            gain = 0.0;
        }
        if treble < 1.0 {
            treble = 1.0;
        }
        s.e[0] /= treble;
        s.e[1] /= treble;
        s.e[2] /= treble;
        s.e[3] /= treble;
        s.e[4] /= treble;
        treble = unbox * 2.0;
        gain = treble;
        if gain > 1.0 {
            s.f[0] = 1.0;
            gain -= 1.0;
        } else {
            s.f[0] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f[1] = 1.0;
            gain -= 1.0;
        } else {
            s.f[1] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f[2] = 1.0;
            gain -= 1.0;
        } else {
            s.f[2] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f[3] = 1.0;
            gain -= 1.0;
        } else {
            s.f[3] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f[4] = 1.0;
            gain -= 1.0;
        } else {
            s.f[4] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f[5] = 1.0;
            gain -= 1.0;
        } else {
            s.f[5] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f[6] = 1.0;
            gain -= 1.0;
        } else {
            s.f[6] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f[7] = 1.0;
            gain -= 1.0;
        } else {
            s.f[7] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f[8] = 1.0;
            gain -= 1.0;
        } else {
            s.f[8] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f[9] = 1.0;
            gain -= 1.0;
        } else {
            s.f[9] = gain;
            gain = 0.0;
        }
        if treble < 1.0 {
            treble = 1.0;
        }
        s.f[0] /= treble;
        s.f[1] /= treble;
        s.f[2] /= treble;
        s.f[3] /= treble;
        s.f[4] /= treble;
        s.f[5] /= treble;
        s.f[6] /= treble;
        s.f[7] /= treble;
        s.f[8] /= treble;
        s.f[9] /= treble;
        for smp in 0..l.len() {
            let mut input_sample_l: f64 = l[smp] as f64;
            let mut input_sample_r: f64 = r[smp] as f64;
            if input != 1.0 {
                input_sample_l *= input;
                input_sample_r *= input;
            }
            if input_sample_l.abs() < 1.18e-23 {
                input_sample_l = s.fpd_l as f64 * 1.18e-17;
            }
            if input_sample_r.abs() < 1.18e-23 {
                input_sample_r = s.fpd_r as f64 * 1.18e-17;
            }
            let mut dry_sample_l: f64 = input_sample_l;
            let mut dry_sample_r: f64 = input_sample_r;
            s.a_l[4] = s.a_l[3];
            s.a_l[3] = s.a_l[2];
            s.a_l[2] = s.a_l[1];
            s.a_l[1] = s.a_l[0];
            s.a_l[0] = input_sample_l;
            input_sample_l *= s.e[0];
            input_sample_l += s.a_l[1] * s.e[1];
            input_sample_l += s.a_l[2] * s.e[2];
            input_sample_l += s.a_l[3] * s.e[3];
            input_sample_l += s.a_l[4] * s.e[4];
            s.a_r[4] = s.a_r[3];
            s.a_r[3] = s.a_r[2];
            s.a_r[2] = s.a_r[1];
            s.a_r[1] = s.a_r[0];
            s.a_r[0] = input_sample_r;
            input_sample_r *= s.e[0];
            input_sample_r += s.a_r[1] * s.e[1];
            input_sample_r += s.a_r[2] * s.e[2];
            input_sample_r += s.a_r[3] * s.e[3];
            input_sample_r += s.a_r[4] * s.e[4];
            s.b_l[4] = s.b_l[3];
            s.b_l[3] = s.b_l[2];
            s.b_l[2] = s.b_l[1];
            s.b_l[1] = s.b_l[0];
            s.b_l[0] = input_sample_l;
            input_sample_l *= s.e[0];
            input_sample_l += s.b_l[1] * s.e[1];
            input_sample_l += s.b_l[2] * s.e[2];
            input_sample_l += s.b_l[3] * s.e[3];
            input_sample_l += s.b_l[4] * s.e[4];
            s.b_r[4] = s.b_r[3];
            s.b_r[3] = s.b_r[2];
            s.b_r[2] = s.b_r[1];
            s.b_r[1] = s.b_r[0];
            s.b_r[0] = input_sample_r;
            input_sample_r *= s.e[0];
            input_sample_r += s.b_r[1] * s.e[1];
            input_sample_r += s.b_r[2] * s.e[2];
            input_sample_r += s.b_r[3] * s.e[3];
            input_sample_r += s.b_r[4] * s.e[4];
            input_sample_l *= unbox;
            input_sample_r *= unbox;
            if input_sample_l > 1.2533141373155 {
                input_sample_l = 1.2533141373155;
            }
            if input_sample_l < -1.2533141373155 {
                input_sample_l = -1.2533141373155;
            }
            input_sample_l = (input_sample_l * input_sample_l.abs()).sin() / (if input_sample_l.abs() == 0.0 { 1.0 } else { input_sample_l.abs() });
            if input_sample_r > 1.2533141373155 {
                input_sample_r = 1.2533141373155;
            }
            if input_sample_r < -1.2533141373155 {
                input_sample_r = -1.2533141373155;
            }
            input_sample_r = (input_sample_r * input_sample_r.abs()).sin() / (if input_sample_r.abs() == 0.0 { 1.0 } else { input_sample_r.abs() });
            input_sample_l /= unbox;
            input_sample_r /= unbox;
            let mut accumulator_sample_l: f64 = dry_sample_l - input_sample_l;
            s.c_l[9] = s.c_l[8];
            s.c_l[8] = s.c_l[7];
            s.c_l[7] = s.c_l[6];
            s.c_l[6] = s.c_l[5];
            s.c_l[5] = s.c_l[4];
            s.c_l[4] = s.c_l[3];
            s.c_l[3] = s.c_l[2];
            s.c_l[2] = s.c_l[1];
            s.c_l[1] = s.c_l[0];
            s.c_l[0] = accumulator_sample_l;
            accumulator_sample_l *= s.f[0];
            accumulator_sample_l += s.c_l[1] * s.f[1];
            accumulator_sample_l += s.c_l[2] * s.f[2];
            accumulator_sample_l += s.c_l[3] * s.f[3];
            accumulator_sample_l += s.c_l[4] * s.f[4];
            accumulator_sample_l += s.c_l[5] * s.f[5];
            accumulator_sample_l += s.c_l[6] * s.f[6];
            accumulator_sample_l += s.c_l[7] * s.f[7];
            accumulator_sample_l += s.c_l[8] * s.f[8];
            accumulator_sample_l += s.c_l[9] * s.f[9];
            let mut accumulator_sample_r: f64 = dry_sample_r - input_sample_r;
            s.c_r[9] = s.c_r[8];
            s.c_r[8] = s.c_r[7];
            s.c_r[7] = s.c_r[6];
            s.c_r[6] = s.c_r[5];
            s.c_r[5] = s.c_r[4];
            s.c_r[4] = s.c_r[3];
            s.c_r[3] = s.c_r[2];
            s.c_r[2] = s.c_r[1];
            s.c_r[1] = s.c_r[0];
            s.c_r[0] = accumulator_sample_r;
            accumulator_sample_r *= s.f[0];
            accumulator_sample_r += s.c_r[1] * s.f[1];
            accumulator_sample_r += s.c_r[2] * s.f[2];
            accumulator_sample_r += s.c_r[3] * s.f[3];
            accumulator_sample_r += s.c_r[4] * s.f[4];
            accumulator_sample_r += s.c_r[5] * s.f[5];
            accumulator_sample_r += s.c_r[6] * s.f[6];
            accumulator_sample_r += s.c_r[7] * s.f[7];
            accumulator_sample_r += s.c_r[8] * s.f[8];
            accumulator_sample_r += s.c_r[9] * s.f[9];
            s.iir_sample_al = s.iir_sample_al * (1.0 - iir_amount) + accumulator_sample_l * iir_amount;
            accumulator_sample_l -= s.iir_sample_al;
            s.iir_sample_ar = s.iir_sample_ar * (1.0 - iir_amount) + accumulator_sample_r * iir_amount;
            accumulator_sample_r -= s.iir_sample_ar;
            s.iir_sample_bl = s.iir_sample_bl * (1.0 - iir_amount) + accumulator_sample_l * iir_amount;
            accumulator_sample_l -= s.iir_sample_bl;
            s.iir_sample_br = s.iir_sample_br * (1.0 - iir_amount) + accumulator_sample_r * iir_amount;
            accumulator_sample_r -= s.iir_sample_br;
            input_sample_l = dry_sample_l - accumulator_sample_l;
            input_sample_r = dry_sample_r - accumulator_sample_r;
            if output != 1.0 {
                input_sample_l *= output;
                input_sample_r *= output;
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
