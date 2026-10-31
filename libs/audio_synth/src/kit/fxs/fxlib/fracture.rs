// Ported from Fracture (FractureProc.cpp, Fracture.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Fracture (sub-effect id 32): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    SlotDef::float("Drive", 0.0, 1.0, 0.25, crate::kit::core::SlotUnit::None),
    SlotDef::choice("Fracture", &["1", "2", "3"], 1.0),
    SlotDef::float("Out Lvl", 0.0, 1.0, 1.0, crate::kit::core::SlotUnit::None),
    SlotDef::float("Dry/Wet", 0.0, 1.0, 1.0, crate::kit::core::SlotUnit::None),
];

#[allow(dead_code)]
pub struct Fracture {
    p: [f32; 4],
    sr: f32,
    fpd_l: u32,
    fpd_r: u32,
}

impl Fracture {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Fracture> {
        let mut s = Box::new(Fracture {
            p: [0.25, 0.5, 1.0, 1.0],
            sr: sr as f32,
            fpd_l: 0,
            fpd_r: 0,
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for Fracture {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut density: f64 = (s.p[0] * 4.0) as f64;
        let mut fracture: f64 = (s.p[1] as f64 * 2.999 + 1.0) * 3.14159265358979;
        let mut output: f64 = s.p[2] as f64;
        let mut wet: f64 = s.p[3] as f64;
        let mut dry: f64 = 1.0 - wet;
        let mut bridgerectifier: f64 = 0.0;
        density = density * density.abs();
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
            input_sample_l *= density;
            input_sample_r *= density;
            bridgerectifier = input_sample_l.abs() * fracture;
            if bridgerectifier > fracture {
                bridgerectifier = fracture;
            }
            bridgerectifier = bridgerectifier.sin();
            if input_sample_l > 0.0 {
                input_sample_l = bridgerectifier;
            } else {
                input_sample_l = -bridgerectifier;
            }
            bridgerectifier = input_sample_r.abs() * fracture;
            if bridgerectifier > fracture {
                bridgerectifier = fracture;
            }
            bridgerectifier = bridgerectifier.sin();
            if input_sample_r > 0.0 {
                input_sample_r = bridgerectifier;
            } else {
                input_sample_r = -bridgerectifier;
            }
            input_sample_l *= output;
            input_sample_r *= output;
            input_sample_l = dry_sample_l * dry + input_sample_l * wet;
            input_sample_r = dry_sample_r * dry + input_sample_r * wet;
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
