// Ported from PowerSag (PowerSagProc.cpp, PowerSag.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Power Sag (sub-effect id 66): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Depth", 0.0),
    pf("Speed", 0.3),
];

#[allow(dead_code)]
pub struct PowerSag {
    p: [f32; 2],
    sr: f32,
    fpd_l: u32,
    fpd_r: u32,
    d_l: Vec<f64>,
    d_r: Vec<f64>,
    control_l: f64,
    control_r: f64,
    gcount: i32,
}

impl PowerSag {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<PowerSag> {
        let mut s = Box::new(PowerSag {
            p: [0.0, 0.3],
            sr: sr as f32,
            fpd_l: 0,
            fpd_r: 0,
            d_l: vec![0.0; 9000],
            d_r: vec![0.0; 9000],
            control_l: 0.0,
            control_r: 0.0,
            gcount: 0,
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for PowerSag {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut intensity: f64 = (s.p[0] as f64).powf(5.0) * 80.0;
        let mut depth_a: f64 = (s.p[1] as f64).powf(2.0);
        let mut offset_a: i32 = (depth_a * 3900.0) as i32 + 1;
        let mut clamp: f64 = 0.0;
        let mut thickness: f64 = 0.0;
        let mut out: f64 = 0.0;
        let mut bridgerectifier: f64 = 0.0;
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
            if s.gcount < 0 || s.gcount > 4000 {
                s.gcount = 4000;
            }
            s.d_l[s.gcount as usize] = input_sample_l.abs() * intensity;
            s.d_l[(s.gcount + 4000) as usize] = s.d_l[s.gcount as usize];
            s.control_l += s.d_l[s.gcount as usize] / offset_a as f64;
            s.control_l -= s.d_l[(s.gcount + offset_a) as usize] / offset_a as f64;
            s.control_l -= 0.000001;
            clamp = 1.0;
            if s.control_l < 0.0 {
                s.control_l = 0.0;
            }
            if s.control_l > 1.0 {
                clamp -= s.control_l - 1.0;
                s.control_l = 1.0;
            }
            if clamp < 0.5 {
                clamp = 0.5;
            }
            thickness = (1.0 - s.control_l) * 2.0 - 1.0;
            out = thickness.abs();
            bridgerectifier = input_sample_l.abs();
            if bridgerectifier > 1.57079633 {
                bridgerectifier = 1.57079633;
            }
            if thickness > 0.0 {
                bridgerectifier = bridgerectifier.sin();
            } else {
                bridgerectifier = 1.0 - bridgerectifier.cos();
            }
            if input_sample_l > 0.0 {
                input_sample_l = input_sample_l * (1.0 - out) + bridgerectifier * out;
            } else {
                input_sample_l = input_sample_l * (1.0 - out) - bridgerectifier * out;
            }
            input_sample_l *= clamp;
            s.d_r[s.gcount as usize] = input_sample_r.abs() * intensity;
            s.d_r[(s.gcount + 4000) as usize] = s.d_r[s.gcount as usize];
            s.control_r += s.d_r[s.gcount as usize] / offset_a as f64;
            s.control_r -= s.d_r[(s.gcount + offset_a) as usize] / offset_a as f64;
            s.control_r -= 0.000001;
            clamp = 1.0;
            if s.control_r < 0.0 {
                s.control_r = 0.0;
            }
            if s.control_r > 1.0 {
                clamp -= s.control_r - 1.0;
                s.control_r = 1.0;
            }
            if clamp < 0.5 {
                clamp = 0.5;
            }
            thickness = (1.0 - s.control_r) * 2.0 - 1.0;
            out = thickness.abs();
            bridgerectifier = input_sample_r.abs();
            if bridgerectifier > 1.57079633 {
                bridgerectifier = 1.57079633;
            }
            if thickness > 0.0 {
                bridgerectifier = bridgerectifier.sin();
            } else {
                bridgerectifier = 1.0 - bridgerectifier.cos();
            }
            if input_sample_r > 0.0 {
                input_sample_r = input_sample_r * (1.0 - out) + bridgerectifier * out;
            } else {
                input_sample_r = input_sample_r * (1.0 - out) - bridgerectifier * out;
            }
            input_sample_r *= clamp;
            s.gcount -= 1;
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
