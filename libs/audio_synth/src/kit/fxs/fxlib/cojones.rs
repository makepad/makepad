// Ported from Cojones (CojonesProc.cpp, Cojones.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Cojones (sub-effect id 28): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Breathy", 0.5),
    pf("Cojones", 0.5),
    pf("Body", 0.5),
    pf("Output", 1.0),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct Cojones {
    p: [f32; 5],
    sr: f32,
    fpd_l: u32,
    fpd_r: u32,
    stored_l: [f64; 2],
    diff_l: [f64; 6],
    stored_r: [f64; 2],
    diff_r: [f64; 6],
}

impl Cojones {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Cojones> {
        let mut s = Box::new(Cojones {
            p: [0.5, 0.5, 0.5, 1.0, 1.0],
            sr: sr as f32,
            fpd_l: 0,
            fpd_r: 0,
            stored_l: [0.0; 2],
            diff_l: [0.0; 6],
            stored_r: [0.0; 2],
            diff_r: [0.0; 6],
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for Cojones {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut breathy: f64 = s.p[0] as f64 * 2.0;
        let mut cojones: f64 = s.p[1] as f64 * 2.0;
        let mut body: f64 = s.p[2] as f64 * 2.0;
        let mut output: f64 = s.p[3] as f64;
        let mut wet: f64 = s.p[4] as f64;
        let mut average_l = [0.0_f64; 5];
        let mut average_r = [0.0_f64; 5];
        for smp in 0..l.len() {
            let mut input_sample_l: f64 = l[smp] as f64;
            let mut input_sample_r: f64 = r[smp] as f64;
            if input_sample_l.abs() < 1.18e-23 {
                input_sample_l = s.fpd_l as f64 * 1.18e-17;
            }
            if input_sample_r.abs() < 1.18e-23 {
                input_sample_r = s.fpd_r as f64 * 1.18e-17;
            }
            let mut dry_sample_l: f64 = input_sample_l;
            let mut dry_sample_r: f64 = input_sample_r;
            s.stored_l[1] = s.stored_l[0];
            s.stored_l[0] = input_sample_l;
            s.diff_l[5] = s.diff_l[4];
            s.diff_l[4] = s.diff_l[3];
            s.diff_l[3] = s.diff_l[2];
            s.diff_l[2] = s.diff_l[1];
            s.diff_l[1] = s.diff_l[0];
            s.diff_l[0] = s.stored_l[0] - s.stored_l[1];
            average_l[0] = s.diff_l[0] + s.diff_l[1];
            average_l[1] = average_l[0] + s.diff_l[2];
            average_l[2] = average_l[1] + s.diff_l[3];
            average_l[3] = average_l[2] + s.diff_l[4];
            average_l[4] = average_l[3] + s.diff_l[5];
            average_l[0] /= 2.0;
            average_l[1] /= 3.0;
            average_l[2] /= 4.0;
            average_l[3] /= 5.0;
            average_l[4] /= 6.0;
            let mut mean_a: f64 = s.diff_l[0];
            let mut mean_b: f64 = s.diff_l[0];
            if average_l[4].abs() < mean_b.abs() {
                mean_a = mean_b;
                mean_b = average_l[4];
            }
            if average_l[3].abs() < mean_b.abs() {
                mean_a = mean_b;
                mean_b = average_l[3];
            }
            if average_l[2].abs() < mean_b.abs() {
                mean_a = mean_b;
                mean_b = average_l[2];
            }
            if average_l[1].abs() < mean_b.abs() {
                mean_a = mean_b;
                mean_b = average_l[1];
            }
            if average_l[0].abs() < mean_b.abs() {
                mean_a = mean_b;
                mean_b = average_l[0];
            }
            let mut mean_out: f64 = (mean_a + mean_b) / 2.0;
            s.stored_l[0] = s.stored_l[1] + mean_out;
            let mut output_sample: f64 = s.stored_l[0] * body;
            output_sample += (input_sample_l - s.stored_l[0] - average_l[1]) * cojones;
            output_sample += average_l[1] * breathy;
            input_sample_l = output_sample;
            s.stored_r[1] = s.stored_r[0];
            s.stored_r[0] = input_sample_r;
            s.diff_r[5] = s.diff_r[4];
            s.diff_r[4] = s.diff_r[3];
            s.diff_r[3] = s.diff_r[2];
            s.diff_r[2] = s.diff_r[1];
            s.diff_r[1] = s.diff_r[0];
            s.diff_r[0] = s.stored_r[0] - s.stored_r[1];
            average_r[0] = s.diff_r[0] + s.diff_r[1];
            average_r[1] = average_r[0] + s.diff_r[2];
            average_r[2] = average_r[1] + s.diff_r[3];
            average_r[3] = average_r[2] + s.diff_r[4];
            average_r[4] = average_r[3] + s.diff_r[5];
            average_r[0] /= 2.0;
            average_r[1] /= 3.0;
            average_r[2] /= 4.0;
            average_r[3] /= 5.0;
            average_r[4] /= 6.0;
            mean_a = s.diff_r[0];
            mean_b = s.diff_r[0];
            if average_r[4].abs() < mean_b.abs() {
                mean_a = mean_b;
                mean_b = average_r[4];
            }
            if average_r[3].abs() < mean_b.abs() {
                mean_a = mean_b;
                mean_b = average_r[3];
            }
            if average_r[2].abs() < mean_b.abs() {
                mean_a = mean_b;
                mean_b = average_r[2];
            }
            if average_r[1].abs() < mean_b.abs() {
                mean_a = mean_b;
                mean_b = average_r[1];
            }
            if average_r[0].abs() < mean_b.abs() {
                mean_a = mean_b;
                mean_b = average_r[0];
            }
            mean_out = (mean_a + mean_b) / 2.0;
            s.stored_r[0] = s.stored_r[1] + mean_out;
            output_sample = s.stored_r[0] * body;
            output_sample += (input_sample_r - s.stored_r[0] - average_r[1]) * cojones;
            output_sample += average_r[1] * breathy;
            input_sample_r = output_sample;
            if output < 1.0 {
                input_sample_l *= output;
                input_sample_r *= output;
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
