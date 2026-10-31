// Ported from HardVacuum (HardVacuumProc.cpp, HardVacuum.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Hard Vacuum (sub-effect id 33): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Drive", 0.0),
    pf("Warmth", 0.0),
    pf("Aura", 0.0),
    pf("Output", 1.0),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct HardVacuum {
    p: [f32; 5],
    sr: f32,
    fpd_l: u32,
    fpd_r: u32,
    last_sample_l: f64,
    last_sample_r: f64,
}

impl HardVacuum {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<HardVacuum> {
        let mut s = Box::new(HardVacuum {
            p: [0.0, 0.0, 0.0, 1.0, 1.0],
            sr: sr as f32,
            fpd_l: 0,
            fpd_r: 0,
            last_sample_l: 0.0,
            last_sample_r: 0.0,
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for HardVacuum {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut multistage: f64 = s.p[0] as f64 * 2.0;
        if multistage > 1.0 {
            multistage *= multistage;
        }
        let mut countdown: f64 = 0.0;
        let mut warmth: f64 = s.p[1] as f64;
        let mut invwarmth: f64 = 1.0 - warmth;
        warmth /= 1.57079633;
        let mut aura: f64 = s.p[2] as f64 * 3.1415926;
        let mut out: f64 = s.p[3] as f64;
        let mut wet: f64 = s.p[4] as f64;
        let mut drive: f64 = 0.0;
        let mut positive: f64 = 0.0;
        let mut negative: f64 = 0.0;
        let mut bridgerectifier_l: f64 = 0.0;
        let mut bridgerectifier_r: f64 = 0.0;
        let mut skew_l: f64 = 0.0;
        let mut skew_r: f64 = 0.0;
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
            skew_l = input_sample_l - s.last_sample_l;
            skew_r = input_sample_r - s.last_sample_r;
            s.last_sample_l = input_sample_l;
            s.last_sample_r = input_sample_r;
            bridgerectifier_l = skew_l.abs();
            bridgerectifier_r = skew_r.abs();
            if bridgerectifier_l > 3.1415926 {
                bridgerectifier_l = 3.1415926;
            }
            if bridgerectifier_r > 3.1415926 {
                bridgerectifier_r = 3.1415926;
            }
            bridgerectifier_l = bridgerectifier_l.sin();
            bridgerectifier_r = bridgerectifier_r.sin();
            if skew_l > 0.0 {
                skew_l = bridgerectifier_l * aura;
            } else {
                skew_l = -bridgerectifier_l * aura;
            }
            if skew_r > 0.0 {
                skew_r = bridgerectifier_r * aura;
            } else {
                skew_r = -bridgerectifier_r * aura;
            }
            skew_l *= input_sample_l;
            skew_r *= input_sample_r;
            skew_l *= 1.557079633;
            skew_r *= 1.557079633;
            countdown = multistage;
            while countdown > 0.0 {
                if countdown > 1.0 {
                    drive = 1.557079633;
                } else {
                    drive = countdown * (1.0 + 0.557079633 * invwarmth);
                }
                positive = drive - warmth;
                negative = drive + warmth;
                bridgerectifier_l = input_sample_l.abs();
                bridgerectifier_r = input_sample_r.abs();
                bridgerectifier_l += skew_l;
                bridgerectifier_r += skew_r;
                if bridgerectifier_l > 1.57079633 {
                    bridgerectifier_l = 1.57079633;
                }
                if bridgerectifier_r > 1.57079633 {
                    bridgerectifier_r = 1.57079633;
                }
                bridgerectifier_l = bridgerectifier_l.sin();
                bridgerectifier_r = bridgerectifier_r.sin();
                bridgerectifier_l *= drive;
                bridgerectifier_r *= drive;
                bridgerectifier_l += skew_l;
                bridgerectifier_r += skew_r;
                if bridgerectifier_l > 1.57079633 {
                    bridgerectifier_l = 1.57079633;
                }
                if bridgerectifier_r > 1.57079633 {
                    bridgerectifier_r = 1.57079633;
                }
                bridgerectifier_l = bridgerectifier_l.sin();
                bridgerectifier_r = bridgerectifier_r.sin();
                if input_sample_l > 0.0 {
                    input_sample_l = input_sample_l * (1.0 - positive + skew_l) + bridgerectifier_l * (positive + skew_l);
                } else {
                    input_sample_l = input_sample_l * (1.0 - negative + skew_l) - bridgerectifier_l * (negative + skew_l);
                }
                if input_sample_r > 0.0 {
                    input_sample_r = input_sample_r * (1.0 - positive + skew_r) + bridgerectifier_r * (positive + skew_r);
                } else {
                    input_sample_r = input_sample_r * (1.0 - negative + skew_r) - bridgerectifier_r * (negative + skew_r);
                }
                countdown -= 1.0;
            }
            if out != 1.0 {
                input_sample_l *= out;
                input_sample_r *= out;
            }
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
