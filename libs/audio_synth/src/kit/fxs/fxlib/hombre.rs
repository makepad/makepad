// Ported from Hombre (HombreProc.cpp, Hombre.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Hombre (sub-effect id 22): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Voicing", 0.421),
    pf("Intensity", 0.5),
];

#[allow(dead_code)]
pub struct Hombre {
    p: [f32; 2],
    sr: f32,
    p_l: Vec<f64>,
    p_r: Vec<f64>,
    slide: f64,
    gcount: i32,
    fpd_l: u32,
    fpd_r: u32,
}

impl Hombre {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Hombre> {
        let mut s = Box::new(Hombre {
            p: [0.421, 0.5],
            sr: sr as f32,
            p_l: vec![0.0; 4001],
            p_r: vec![0.0; 4001],
            slide: 0.0,
            gcount: 0,
            fpd_l: 0,
            fpd_r: 0,
        });
        s.slide = 0.421;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for Hombre {
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
        let mut target: f64 = s.p[0] as f64;
        let mut offset_a: f64 = 0.0;
        let mut offset_b: f64 = 0.0;
        let mut width_a: i32 = (1.0 * overallscale) as i32;
        let mut width_b: i32 = (7.0 * overallscale) as i32;
        let mut wet: f64 = s.p[1] as f64;
        let mut total_l: f64 = 0.0;
        let mut total_r: f64 = 0.0;
        let mut count: i32 = 0;
        let mut input_sample_l: f64 = 0.0;
        let mut input_sample_r: f64 = 0.0;
        let mut dry_sample_l: f64 = 0.0;
        let mut dry_sample_r: f64 = 0.0;
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
            s.slide = s.slide * 0.9997 + target * 0.0003;
            offset_a = s.slide.powf(2.0) * 77.0 + 3.2;
            offset_b = 3.85 * offset_a + 41.0;
            offset_a *= overallscale;
            offset_b *= overallscale;
            if s.gcount < 1 || s.gcount > 2000 {
                s.gcount = 2000;
            }
            count = s.gcount;
            s.p_l[count as usize] = input_sample_l;
            s.p_l[(count + 2000) as usize] = s.p_l[count as usize];
            s.p_r[count as usize] = input_sample_r;
            s.p_r[(count + 2000) as usize] = s.p_r[count as usize];
            count = (s.gcount as f64 + offset_a.floor()) as i32;
            total_l = s.p_l[count as usize] * 0.391;
            total_l += s.p_l[(count + width_a) as usize];
            total_l += s.p_l[(count + width_a + width_a) as usize] * 0.391;
            total_r = s.p_r[count as usize] * 0.391;
            total_r += s.p_r[(count + width_a) as usize];
            total_r += s.p_r[(count + width_a + width_a) as usize] * 0.391;
            input_sample_l += total_l * 0.274;
            input_sample_r += total_r * 0.274;
            count = (s.gcount as f64 + offset_b.floor()) as i32;
            total_l = s.p_l[count as usize] * 0.918;
            total_l += s.p_l[(count + width_b) as usize];
            total_l += s.p_l[(count + width_b + width_b) as usize] * 0.918;
            total_r = s.p_r[count as usize] * 0.918;
            total_r += s.p_r[(count + width_b) as usize];
            total_r += s.p_r[(count + width_b + width_b) as usize] * 0.918;
            input_sample_l -= total_l * 0.629;
            input_sample_r -= total_r * 0.629;
            input_sample_l /= 4.0;
            input_sample_r /= 4.0;
            s.gcount -= 1;
            if wet != 1.0 {
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
