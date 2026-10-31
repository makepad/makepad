// Ported from Melt (MeltProc.cpp, Melt.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Melt (sub-effect id 23): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Depth", 0.25),
    pf("Range", 0.75),
    pf("Output", 1.0),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct Melt {
    p: [f32; 4],
    sr: f32,
    d_l: Vec<f32>,
    d_r: Vec<f32>,
    combine_l: f32,
    combine_r: f32,
    scalefactor_l: f32,
    scalefactor_r: f32,
    min_tap: [i32; 32],
    max_tap: [i32; 32],
    position: [i32; 32],
    step_tap: [i32; 32],
    step_count: i32,
    slow_count: i32,
    gcount: i32,
    fpd_l: u32,
    fpd_r: u32,
}

impl Melt {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Melt> {
        let mut s = Box::new(Melt {
            p: [0.25, 0.75, 1.0, 1.0],
            sr: sr as f32,
            d_l: vec![0.0; 32002],
            d_r: vec![0.0; 32002],
            combine_l: 0.0,
            combine_r: 0.0,
            scalefactor_l: 0.0,
            scalefactor_r: 0.0,
            min_tap: [0; 32],
            max_tap: [0; 32],
            position: [0; 32],
            step_tap: [0; 32],
            step_count: 0,
            slow_count: 0,
            gcount: 0,
            fpd_l: 0,
            fpd_r: 0,
        });
        s.scalefactor_l = 0.999;
        s.scalefactor_r = 0.999;
        {
            let mut count: i32 = 0;
            while count < 31 {
                s.min_tap[count as usize] = 0;
                s.max_tap[count as usize] = 0;
                s.position[count as usize] = 1;
                s.step_tap[count as usize] = 1;
                count += 1;
            }
        }
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for Melt {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut rate: f64 = 1.0 / ((s.p[0] as f64).powf(2.0) + 0.001);
        let mut depth_b: f64 = s.p[1] as f64 * 139.5 + 2.0;
        let mut depth_a: f64 = depth_b * (1.0 - s.p[0] as f64);
        let mut output: f64 = s.p[2] as f64 * 0.05;
        let mut wet: f64 = s.p[3] as f64;
        let mut dry: f64 = 1.0 - wet;
        let mut input_sample_l: f64 = 0.0;
        let mut input_sample_r: f64 = 0.0;
        let mut dry_sample_l: f64 = 0.0;
        let mut dry_sample_r: f64 = 0.0;
        s.min_tap[0] = (2.0 * depth_a).floor() as i32;
        s.max_tap[0] = (2.0 * depth_b).floor() as i32;
        s.min_tap[1] = (3.0 * depth_a).floor() as i32;
        s.max_tap[1] = (3.0 * depth_b).floor() as i32;
        s.min_tap[2] = (5.0 * depth_a).floor() as i32;
        s.max_tap[2] = (5.0 * depth_b).floor() as i32;
        s.min_tap[3] = (7.0 * depth_a).floor() as i32;
        s.max_tap[3] = (7.0 * depth_b).floor() as i32;
        s.min_tap[4] = (11.0 * depth_a).floor() as i32;
        s.max_tap[4] = (11.0 * depth_b).floor() as i32;
        s.min_tap[5] = (13.0 * depth_a).floor() as i32;
        s.max_tap[5] = (13.0 * depth_b).floor() as i32;
        s.min_tap[6] = (17.0 * depth_a).floor() as i32;
        s.max_tap[6] = (17.0 * depth_b).floor() as i32;
        s.min_tap[7] = (19.0 * depth_a).floor() as i32;
        s.max_tap[7] = (19.0 * depth_b).floor() as i32;
        s.min_tap[8] = (23.0 * depth_a).floor() as i32;
        s.max_tap[8] = (23.0 * depth_b).floor() as i32;
        s.min_tap[9] = (29.0 * depth_a).floor() as i32;
        s.max_tap[9] = (29.0 * depth_b).floor() as i32;
        s.min_tap[10] = (31.0 * depth_a).floor() as i32;
        s.max_tap[10] = (31.0 * depth_b).floor() as i32;
        s.min_tap[11] = (37.0 * depth_a).floor() as i32;
        s.max_tap[11] = (37.0 * depth_b).floor() as i32;
        s.min_tap[12] = (41.0 * depth_a).floor() as i32;
        s.max_tap[12] = (41.0 * depth_b).floor() as i32;
        s.min_tap[13] = (43.0 * depth_a).floor() as i32;
        s.max_tap[13] = (43.0 * depth_b).floor() as i32;
        s.min_tap[14] = (47.0 * depth_a).floor() as i32;
        s.max_tap[14] = (47.0 * depth_b).floor() as i32;
        s.min_tap[15] = (53.0 * depth_a).floor() as i32;
        s.max_tap[15] = (53.0 * depth_b).floor() as i32;
        s.min_tap[16] = (59.0 * depth_a).floor() as i32;
        s.max_tap[16] = (59.0 * depth_b).floor() as i32;
        s.min_tap[17] = (61.0 * depth_a).floor() as i32;
        s.max_tap[17] = (61.0 * depth_b).floor() as i32;
        s.min_tap[18] = (67.0 * depth_a).floor() as i32;
        s.max_tap[18] = (67.0 * depth_b).floor() as i32;
        s.min_tap[19] = (71.0 * depth_a).floor() as i32;
        s.max_tap[19] = (71.0 * depth_b).floor() as i32;
        s.min_tap[20] = (73.0 * depth_a).floor() as i32;
        s.max_tap[20] = (73.0 * depth_b).floor() as i32;
        s.min_tap[21] = (79.0 * depth_a).floor() as i32;
        s.max_tap[21] = (79.0 * depth_b).floor() as i32;
        s.min_tap[22] = (83.0 * depth_a).floor() as i32;
        s.max_tap[22] = (83.0 * depth_b).floor() as i32;
        s.min_tap[23] = (89.0 * depth_a).floor() as i32;
        s.max_tap[23] = (89.0 * depth_b).floor() as i32;
        s.min_tap[24] = (97.0 * depth_a).floor() as i32;
        s.max_tap[24] = (97.0 * depth_b).floor() as i32;
        s.min_tap[25] = (101.0 * depth_a).floor() as i32;
        s.max_tap[25] = (101.0 * depth_b).floor() as i32;
        s.min_tap[26] = (103.0 * depth_a).floor() as i32;
        s.max_tap[26] = (103.0 * depth_b).floor() as i32;
        s.min_tap[27] = (107.0 * depth_a).floor() as i32;
        s.max_tap[27] = (107.0 * depth_b).floor() as i32;
        s.min_tap[28] = (109.0 * depth_a).floor() as i32;
        s.max_tap[28] = (109.0 * depth_b).floor() as i32;
        s.min_tap[29] = (113.0 * depth_a).floor() as i32;
        s.max_tap[29] = (113.0 * depth_b).floor() as i32;
        s.min_tap[30] = (117.0 * depth_a).floor() as i32;
        s.max_tap[30] = (117.0 * depth_b).floor() as i32;
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
            if s.gcount < 0 || s.gcount > 16000 {
                s.gcount = 16000;
            }
            s.d_l[s.gcount as usize] = input_sample_l as f32;
            s.d_l[(s.gcount + 16000) as usize] = s.d_l[s.gcount as usize];
            s.d_r[s.gcount as usize] = input_sample_r as f32;
            s.d_r[(s.gcount + 16000) as usize] = s.d_r[s.gcount as usize];
            if s.slow_count as f64 > rate || s.slow_count < 0 {
                s.slow_count = 0;
                s.step_count += 1;
                if s.step_count > 29 || s.step_count < 0 {
                    s.step_count = 0;
                }
                s.position[s.step_count as usize] += s.step_tap[s.step_count as usize];
                if s.position[s.step_count as usize] < s.min_tap[s.step_count as usize] {
                    s.position[s.step_count as usize] = s.min_tap[s.step_count as usize];
                    s.step_tap[s.step_count as usize] = 1;
                }
                if s.position[s.step_count as usize] > s.max_tap[s.step_count as usize] {
                    s.position[s.step_count as usize] = s.max_tap[s.step_count as usize];
                    s.step_tap[s.step_count as usize] = -1;
                }
            }
            s.scalefactor_l = (s.scalefactor_l as f64 * 0.9999) as f32;
            s.scalefactor_l = (s.scalefactor_l as f64 + (100.0 - s.combine_l.abs() as f64) * 0.000001) as f32;
            s.combine_l *= s.scalefactor_l;
            s.combine_l -= s.d_l[(s.gcount + s.position[29]) as usize];
            s.combine_l += s.d_l[(s.gcount + s.position[28]) as usize];
            s.combine_l *= s.scalefactor_l;
            s.combine_l -= s.d_l[(s.gcount + s.position[27]) as usize];
            s.combine_l += s.d_l[(s.gcount + s.position[26]) as usize];
            s.combine_l *= s.scalefactor_l;
            s.combine_l -= s.d_l[(s.gcount + s.position[25]) as usize];
            s.combine_l += s.d_l[(s.gcount + s.position[24]) as usize];
            s.combine_l *= s.scalefactor_l;
            s.combine_l -= s.d_l[(s.gcount + s.position[23]) as usize];
            s.combine_l += s.d_l[(s.gcount + s.position[22]) as usize];
            s.combine_l *= s.scalefactor_l;
            s.combine_l -= s.d_l[(s.gcount + s.position[21]) as usize];
            s.combine_l += s.d_l[(s.gcount + s.position[20]) as usize];
            s.combine_l *= s.scalefactor_l;
            s.combine_l -= s.d_l[(s.gcount + s.position[19]) as usize];
            s.combine_l += s.d_l[(s.gcount + s.position[18]) as usize];
            s.combine_l *= s.scalefactor_l;
            s.combine_l -= s.d_l[(s.gcount + s.position[17]) as usize];
            s.combine_l += s.d_l[(s.gcount + s.position[16]) as usize];
            s.combine_l *= s.scalefactor_l;
            s.combine_l -= s.d_l[(s.gcount + s.position[15]) as usize];
            s.combine_l += s.d_l[(s.gcount + s.position[14]) as usize];
            s.combine_l *= s.scalefactor_l;
            s.combine_l -= s.d_l[(s.gcount + s.position[13]) as usize];
            s.combine_l += s.d_l[(s.gcount + s.position[12]) as usize];
            s.combine_l *= s.scalefactor_l;
            s.combine_l -= s.d_l[(s.gcount + s.position[11]) as usize];
            s.combine_l += s.d_l[(s.gcount + s.position[10]) as usize];
            s.combine_l *= s.scalefactor_l;
            s.combine_l -= s.d_l[(s.gcount + s.position[9]) as usize];
            s.combine_l += s.d_l[(s.gcount + s.position[8]) as usize];
            s.combine_l *= s.scalefactor_l;
            s.combine_l -= s.d_l[(s.gcount + s.position[7]) as usize];
            s.combine_l += s.d_l[(s.gcount + s.position[6]) as usize];
            s.combine_l *= s.scalefactor_l;
            s.combine_l -= s.d_l[(s.gcount + s.position[5]) as usize];
            s.combine_l += s.d_l[(s.gcount + s.position[4]) as usize];
            s.combine_l *= s.scalefactor_l;
            s.combine_l -= s.d_l[(s.gcount + s.position[3]) as usize];
            s.combine_l += s.d_l[(s.gcount + s.position[2]) as usize];
            s.combine_l *= s.scalefactor_l;
            s.combine_l -= s.d_l[(s.gcount + s.position[1]) as usize];
            s.combine_l += s.d_l[(s.gcount + s.position[0]) as usize];
            input_sample_l = s.combine_l as f64;
            s.scalefactor_r = (s.scalefactor_r as f64 * 0.9999) as f32;
            s.scalefactor_r = (s.scalefactor_r as f64 + (100.0 - s.combine_r.abs() as f64) * 0.000001) as f32;
            s.combine_r *= s.scalefactor_r;
            s.combine_r -= s.d_r[(s.gcount + s.position[29]) as usize];
            s.combine_r += s.d_r[(s.gcount + s.position[28]) as usize];
            s.combine_r *= s.scalefactor_r;
            s.combine_r -= s.d_r[(s.gcount + s.position[27]) as usize];
            s.combine_r += s.d_r[(s.gcount + s.position[26]) as usize];
            s.combine_r *= s.scalefactor_r;
            s.combine_r -= s.d_r[(s.gcount + s.position[25]) as usize];
            s.combine_r += s.d_r[(s.gcount + s.position[24]) as usize];
            s.combine_r *= s.scalefactor_r;
            s.combine_r -= s.d_r[(s.gcount + s.position[23]) as usize];
            s.combine_r += s.d_r[(s.gcount + s.position[22]) as usize];
            s.combine_r *= s.scalefactor_r;
            s.combine_r -= s.d_r[(s.gcount + s.position[21]) as usize];
            s.combine_r += s.d_r[(s.gcount + s.position[20]) as usize];
            s.combine_r *= s.scalefactor_r;
            s.combine_r -= s.d_r[(s.gcount + s.position[19]) as usize];
            s.combine_r += s.d_r[(s.gcount + s.position[18]) as usize];
            s.combine_r *= s.scalefactor_r;
            s.combine_r -= s.d_r[(s.gcount + s.position[17]) as usize];
            s.combine_r += s.d_r[(s.gcount + s.position[16]) as usize];
            s.combine_r *= s.scalefactor_r;
            s.combine_r -= s.d_r[(s.gcount + s.position[15]) as usize];
            s.combine_r += s.d_r[(s.gcount + s.position[14]) as usize];
            s.combine_r *= s.scalefactor_r;
            s.combine_r -= s.d_r[(s.gcount + s.position[13]) as usize];
            s.combine_r += s.d_r[(s.gcount + s.position[12]) as usize];
            s.combine_r *= s.scalefactor_r;
            s.combine_r -= s.d_r[(s.gcount + s.position[11]) as usize];
            s.combine_r += s.d_r[(s.gcount + s.position[10]) as usize];
            s.combine_r *= s.scalefactor_r;
            s.combine_r -= s.d_r[(s.gcount + s.position[9]) as usize];
            s.combine_r += s.d_r[(s.gcount + s.position[8]) as usize];
            s.combine_r *= s.scalefactor_r;
            s.combine_r -= s.d_r[(s.gcount + s.position[7]) as usize];
            s.combine_r += s.d_r[(s.gcount + s.position[6]) as usize];
            s.combine_r *= s.scalefactor_r;
            s.combine_r -= s.d_r[(s.gcount + s.position[5]) as usize];
            s.combine_r += s.d_r[(s.gcount + s.position[4]) as usize];
            s.combine_r *= s.scalefactor_r;
            s.combine_r -= s.d_r[(s.gcount + s.position[3]) as usize];
            s.combine_r += s.d_r[(s.gcount + s.position[2]) as usize];
            s.combine_r *= s.scalefactor_r;
            s.combine_r -= s.d_r[(s.gcount + s.position[1]) as usize];
            s.combine_r += s.d_r[(s.gcount + s.position[0]) as usize];
            input_sample_r = s.combine_r as f64;
            s.gcount -= 1;
            s.slow_count += 1;
            if output < 1.0 {
                input_sample_l *= output;
                input_sample_r *= output;
            }
            if wet < 1.0 {
                input_sample_l = dry_sample_l * dry + input_sample_l * wet;
                input_sample_r = dry_sample_r * dry + input_sample_r * wet;
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
