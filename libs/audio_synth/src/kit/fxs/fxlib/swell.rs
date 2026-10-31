// Ported from the collection's dynamics plugin with stored id 11 (its Proc.cpp and .cpp), part of the
// MIT-licensed effects collection by Chris Johnson.
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

//! Swell (sub-effect id 11): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Swell", 0.0),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct Swell {
    p: [f32; 2],
    sr: f32,
    fpd_l: u32,
    fpd_r: u32,
    flip: bool,
    chase_a: f64,
    chase_b: f64,
    chase_c: f64,
    chase_d: f64,
    chase_max: f64,
}

impl Swell {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Swell> {
        let mut s = Box::new(Swell {
            p: [0.0, 1.0],
            sr: sr as f32,
            fpd_l: 0,
            fpd_r: 0,
            flip: false,
            chase_a: 0.0,
            chase_b: 0.0,
            chase_c: 0.0,
            chase_d: 0.0,
            chase_max: 0.0,
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s.flip = true;
        s
    }
}

impl Sub for Swell {
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
        let mut input_sample_l: f64 = 0.0;
        let mut input_sample_r: f64 = 0.0;
        let mut dry_sample_l: f64 = 0.0;
        let mut dry_sample_r: f64 = 0.0;
        let mut chase_max: f64 = 0.0;
        let mut intensity: f64 = (1.0 - (1.0 - s.p[0] as f64).powf(2.0)) * 0.7;
        let mut attack: f64 = (intensity + 0.1) * 0.0005 / overallscale;
        let mut decay: f64 = (intensity + 0.001) * 0.00005 / overallscale;
        let mut wet: f64 = s.p[1] as f64;
        let mut input_sense: f64 = 0.0;
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
            input_sample_l *= 8.0;
            input_sample_r *= 8.0;
            input_sample_l *= intensity;
            input_sample_r *= intensity;
            input_sense = input_sample_l.abs();
            if input_sample_r.abs() > input_sense {
                input_sense = input_sample_r.abs();
            }
            if chase_max < input_sense {
                s.chase_a += attack;
            }
            if chase_max > input_sense {
                s.chase_a -= decay;
            }
            if s.chase_a > decay {
                s.chase_a = decay;
            }
            if s.chase_a < -attack {
                s.chase_a = -attack;
            }
            s.chase_b += s.chase_a / overallscale;
            if s.chase_b > decay {
                s.chase_b = decay;
            }
            if s.chase_b < -attack {
                s.chase_b = -attack;
            }
            s.chase_c += s.chase_b / overallscale;
            if s.chase_c > decay {
                s.chase_c = decay;
            }
            if s.chase_c < -attack {
                s.chase_c = -attack;
            }
            s.chase_d += s.chase_c / overallscale;
            if s.chase_d > 1.0 {
                s.chase_d = 1.0;
            }
            if s.chase_d < 0.0 {
                s.chase_d = 0.0;
            }
            chase_max = s.chase_a;
            if chase_max < s.chase_b {
                chase_max = s.chase_b;
            }
            if chase_max < s.chase_c {
                chase_max = s.chase_c;
            }
            if chase_max < s.chase_d {
                chase_max = s.chase_d;
            }
            input_sample_l *= chase_max;
            input_sample_l = dry_sample_l - input_sample_l * intensity;
            input_sample_l = dry_sample_l * (1.0 - wet) + input_sample_l * wet;
            input_sample_r *= chase_max;
            input_sample_r = dry_sample_r - input_sample_r * intensity;
            input_sample_r = dry_sample_r * (1.0 - wet) + input_sample_r * wet;
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
