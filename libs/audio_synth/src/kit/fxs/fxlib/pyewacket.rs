// Ported from Pyewacket (PyewacketProc.cpp, Pyewacket.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Pye Wacket (sub-effect id 10): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Input Gain", 0.5),
    pf("Release", 0.5),
    pf("Output Gain", 0.5),
];

#[allow(dead_code)]
pub struct Pyewacket {
    p: [f32; 3],
    sr: f32,
    fpd_l: u32,
    fpd_r: u32,
    chase: f64,
    lastrectifier_l: f64,
    lastrectifier_r: f64,
}

impl Pyewacket {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Pyewacket> {
        let mut s = Box::new(Pyewacket {
            p: [0.5, 0.5, 0.5],
            sr: sr as f32,
            fpd_l: 0,
            fpd_r: 0,
            chase: 0.0,
            lastrectifier_l: 0.0,
            lastrectifier_r: 0.0,
        });
        s.chase = 1.0;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for Pyewacket {
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
        let mut fp_old: f64 = 0.618033988749894848204586;
        let mut fp_new: f64 = 1.0 - fp_old;
        let mut input_sample_l: f64 = 0.0;
        let mut input_sample_r: f64 = 0.0;
        let mut dry_sample_l: f64 = 0.0;
        let mut dry_sample_r: f64 = 0.0;
        let mut bridgerectifier: f64 = 0.0;
        let mut temprectifier: f64 = 0.0;
        let mut input_sense: f64 = 0.0;
        let mut input_gain: f64 = 10.0_f64.powf((s.p[0] as f64 * 24.0 - 12.0) / 20.0);
        let mut attack: f64 = (s.p[1] as f64 + 0.5) * 0.006 / overallscale;
        let mut decay: f64 = (s.p[1] as f64 + 0.01) * 0.0004 / overallscale;
        let mut output_gain: f64 = 10.0_f64.powf((s.p[2] as f64 * 24.0 - 12.0) / 20.0);
        let mut wet: f64 = 0.0;
        let mut maxblur: f64 = 0.0;
        let mut blurdry: f64 = 0.0;
        let mut out: f64 = 0.0;
        let mut dry: f64 = 0.0;
        for smp in 0..l.len() {
            input_sample_l = l[smp] as f64;
            input_sample_r = r[smp] as f64;
            if input_sample_l.abs() < 1.18e-23 {
                input_sample_l = s.fpd_l as f64 * 1.18e-17;
            }
            if input_sample_r.abs() < 1.18e-23 {
                input_sample_r = s.fpd_r as f64 * 1.18e-17;
            }
            if input_gain != 1.0 {
                input_sample_l *= input_gain;
                input_sample_r *= input_gain;
            }
            dry_sample_l = input_sample_l;
            dry_sample_r = input_sample_r;
            input_sense = input_sample_l.abs();
            if input_sample_r.abs() > input_sense {
                input_sense = input_sample_r.abs();
            }
            if s.chase < input_sense {
                s.chase += attack;
            }
            if s.chase > 1.0 {
                s.chase = 1.0;
            }
            if s.chase > input_sense {
                s.chase -= decay;
            }
            if s.chase < 0.0 {
                s.chase = 0.0;
            }
            wet = s.chase;
            out = wet;
            if wet > 1.0 {
                wet = 1.0;
            }
            maxblur = wet * fp_new;
            blurdry = 1.0 - maxblur;
            if out > fp_old {
                out = fp_old - (out - fp_old);
            }
            if out < 0.0 {
                out = 0.0;
            }
            dry = 1.0 - wet;
            if input_sample_l > 1.57079633 {
                input_sample_l = 1.57079633;
            }
            if input_sample_l < -1.57079633 {
                input_sample_l = -1.57079633;
            }
            if input_sample_r > 1.57079633 {
                input_sample_r = 1.57079633;
            }
            if input_sample_r < -1.57079633 {
                input_sample_r = -1.57079633;
            }
            bridgerectifier = input_sample_l.abs();
            if bridgerectifier > 1.57079633 {
                bridgerectifier = 1.57079633;
            }
            temprectifier = 1.0 - bridgerectifier.cos();
            bridgerectifier = s.lastrectifier_l * maxblur + temprectifier * blurdry;
            s.lastrectifier_l = temprectifier;
            if input_sample_l > 0.0 {
                input_sample_l = input_sample_l * dry + bridgerectifier * out;
            } else {
                input_sample_l = input_sample_l * dry - bridgerectifier * out;
            }
            bridgerectifier = input_sample_r.abs();
            if bridgerectifier > 1.57079633 {
                bridgerectifier = 1.57079633;
            }
            temprectifier = 1.0 - bridgerectifier.cos();
            bridgerectifier = s.lastrectifier_r * maxblur + temprectifier * blurdry;
            s.lastrectifier_r = temprectifier;
            if input_sample_r > 0.0 {
                input_sample_r = input_sample_r * dry + bridgerectifier * out;
            } else {
                input_sample_r = input_sample_r * dry - bridgerectifier * out;
            }
            if output_gain != 1.0 {
                input_sample_l *= output_gain;
                input_sample_r *= output_gain;
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
