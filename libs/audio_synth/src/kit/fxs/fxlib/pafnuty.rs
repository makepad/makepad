// Ported from Pafnuty (PafnutyProc.cpp, Pafnuty.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Pafnuty (sub-effect id 65): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Second", 0.5),
    pf("Third", 0.5),
    pf("Fourth", 0.5),
    pf("Fifth", 0.5),
    pf("Sixth", 0.5),
    pf("Seventh", 0.5),
    pf("Eighth", 0.5),
    pf("Ninth", 0.5),
    pf("Tenth", 0.5),
    pf("Elevnth", 0.5),
    pf("Twelvth", 0.5),
];

#[allow(dead_code)]
pub struct Pafnuty {
    p: [f32; 13],
    sr: f32,
    fpd_l: u32,
    fpd_r: u32,
}

impl Pafnuty {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Pafnuty> {
        let mut s = Box::new(Pafnuty {
            p: [0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 1.0],
            sr: sr as f32,
            fpd_l: 0,
            fpd_r: 0,
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for Pafnuty {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut chebyshev: f64 = 0.0;
        let mut effxct: f64 = 0.0;
        let mut in_p2: f64 = 0.0;
        let mut in_p3: f64 = 0.0;
        let mut in_p4: f64 = 0.0;
        let mut in_p5: f64 = 0.0;
        let mut in_p6: f64 = 0.0;
        let mut in_p7: f64 = 0.0;
        let mut in_p8: f64 = 0.0;
        let mut in_p9: f64 = 0.0;
        let mut in_p10: f64 = 0.0;
        let mut in_p11: f64 = 0.0;
        let mut in_p12: f64 = 0.0;
        let mut in_p13: f64 = 0.0;
        let mut second: f64 = (s.p[0] as f64 * 2.0 - 1.0) * 1.0;
        second = second * second.abs();
        let mut third: f64 = -((s.p[1] as f64 * 2.0 - 1.0) * 0.60);
        third = third * third.abs();
        let mut fourth: f64 = -((s.p[2] as f64 * 2.0 - 1.0) * 0.60);
        fourth = fourth * fourth.abs();
        let mut fifth: f64 = (s.p[3] as f64 * 2.0 - 1.0) * 0.45;
        fifth = fifth * fifth.abs();
        let mut sixth: f64 = (s.p[4] as f64 * 2.0 - 1.0) * 0.45;
        sixth = sixth * sixth.abs();
        let mut seventh: f64 = -((s.p[5] as f64 * 2.0 - 1.0) * 0.38);
        seventh = seventh * seventh.abs();
        let mut eighth: f64 = -((s.p[6] as f64 * 2.0 - 1.0) * 0.38);
        eighth = eighth * eighth.abs();
        let mut ninth: f64 = (s.p[7] as f64 * 2.0 - 1.0) * 0.35;
        ninth = ninth * ninth.abs();
        let mut tenth: f64 = (s.p[8] as f64 * 2.0 - 1.0) * 0.35;
        tenth = tenth * tenth.abs();
        let mut eleventh: f64 = -((s.p[9] as f64 * 2.0 - 1.0) * 0.32);
        eleventh = eleventh * eleventh.abs();
        let mut twelvth: f64 = -((s.p[10] as f64 * 2.0 - 1.0) * 0.32);
        twelvth = twelvth * twelvth.abs();
        let mut thirteenth: f64 = (s.p[11] as f64 * 2.0 - 1.0) * 0.30;
        thirteenth = thirteenth * thirteenth.abs();
        let mut amount: f64 = s.p[12] as f64 * 2.0 - 1.0;
        amount = amount * amount.abs();
        for smp in 0..l.len() {
            let mut input_sample_l: f64 = l[smp] as f64;
            let mut input_sample_r: f64 = r[smp] as f64;
            if input_sample_l.abs() < 1.18e-23 {
                input_sample_l = s.fpd_l as f64 * 1.18e-17;
            }
            if input_sample_r.abs() < 1.18e-23 {
                input_sample_r = s.fpd_r as f64 * 1.18e-17;
            }
            effxct = 0.0;
            in_p2 = input_sample_l * input_sample_l;
            in_p3 = in_p2 * input_sample_l;
            in_p4 = in_p3 * input_sample_l;
            in_p5 = in_p4 * input_sample_l;
            in_p6 = in_p5 * input_sample_l;
            in_p7 = in_p6 * input_sample_l;
            in_p8 = in_p7 * input_sample_l;
            in_p9 = in_p8 * input_sample_l;
            in_p10 = in_p9 * input_sample_l;
            in_p11 = in_p10 * input_sample_l;
            in_p12 = in_p11 * input_sample_l;
            in_p13 = in_p12 * input_sample_l;
            if second != 0.0 {
                chebyshev = 2.0 * in_p2;
                effxct += chebyshev * second;
            }
            if third != 0.0 {
                chebyshev = 4.0 * in_p3 - 3.0 * input_sample_l;
                effxct += chebyshev * third;
            }
            if fourth != 0.0 {
                chebyshev = 8.0 * in_p4 - 8.0 * in_p2;
                effxct += chebyshev * fourth;
            }
            if fifth != 0.0 {
                chebyshev = 16.0 * in_p5 - 20.0 * in_p3 + 5.0 * input_sample_l;
                effxct += chebyshev * fifth;
            }
            if sixth != 0.0 {
                chebyshev = 32.0 * in_p6 - 48.0 * in_p4 + 18.0 * in_p2;
                effxct += chebyshev * sixth;
            }
            if seventh != 0.0 {
                chebyshev = 64.0 * in_p7 - 112.0 * in_p5 + 56.0 * in_p3 - 7.0 * input_sample_l;
                effxct += chebyshev * seventh;
            }
            if eighth != 0.0 {
                chebyshev = 128.0 * in_p8 - 256.0 * in_p6 + 160.0 * in_p4 - 32.0 * in_p2;
                effxct += chebyshev * eighth;
            }
            if ninth != 0.0 {
                chebyshev = 256.0 * in_p9 - 576.0 * in_p7 + 432.0 * in_p5 - 120.0 * in_p3 + 9.0 * input_sample_l;
                effxct += chebyshev * ninth;
            }
            if tenth != 0.0 {
                chebyshev = 512.0 * in_p10 - 1280.0 * in_p8 + 1120.0 * in_p6 - 400.0 * in_p4 + 50.0 * in_p2;
                effxct += chebyshev * tenth;
            }
            if eleventh != 0.0 {
                chebyshev = 1024.0 * in_p11 - 2816.0 * in_p9 + 2816.0 * in_p7 - 1232.0 * in_p5 + 220.0 * in_p3 - 11.0 * input_sample_l;
                effxct += chebyshev * eleventh;
            }
            if twelvth != 0.0 {
                chebyshev = 2048.0 * in_p12 - 6144.0 * in_p10 + 6912.0 * in_p8 - 3584.0 * in_p6 + 840.0 * in_p4 - 72.0 * in_p2;
                effxct += chebyshev * twelvth;
            }
            if thirteenth != 0.0 {
                chebyshev = 4096.0 * in_p13 - 13312.0 * in_p11 + 16640.0 * in_p9 - 9984.0 * in_p7 + 2912.0 * in_p5 - 364.0 * in_p3 + 13.0 * input_sample_l;
                effxct += chebyshev * thirteenth;
            }
            input_sample_l += effxct * amount;
            effxct = 0.0;
            in_p2 = input_sample_r * input_sample_r;
            in_p3 = in_p2 * input_sample_r;
            in_p4 = in_p3 * input_sample_r;
            in_p5 = in_p4 * input_sample_r;
            in_p6 = in_p5 * input_sample_r;
            in_p7 = in_p6 * input_sample_r;
            in_p8 = in_p7 * input_sample_r;
            in_p9 = in_p8 * input_sample_r;
            in_p10 = in_p9 * input_sample_r;
            in_p11 = in_p10 * input_sample_r;
            in_p12 = in_p11 * input_sample_r;
            in_p13 = in_p12 * input_sample_r;
            if second != 0.0 {
                chebyshev = 2.0 * in_p2;
                effxct += chebyshev * second;
            }
            if third != 0.0 {
                chebyshev = 4.0 * in_p3 - 3.0 * input_sample_r;
                effxct += chebyshev * third;
            }
            if fourth != 0.0 {
                chebyshev = 8.0 * in_p4 - 8.0 * in_p2;
                effxct += chebyshev * fourth;
            }
            if fifth != 0.0 {
                chebyshev = 16.0 * in_p5 - 20.0 * in_p3 + 5.0 * input_sample_r;
                effxct += chebyshev * fifth;
            }
            if sixth != 0.0 {
                chebyshev = 32.0 * in_p6 - 48.0 * in_p4 + 18.0 * in_p2;
                effxct += chebyshev * sixth;
            }
            if seventh != 0.0 {
                chebyshev = 64.0 * in_p7 - 112.0 * in_p5 + 56.0 * in_p3 - 7.0 * input_sample_r;
                effxct += chebyshev * seventh;
            }
            if eighth != 0.0 {
                chebyshev = 128.0 * in_p8 - 256.0 * in_p6 + 160.0 * in_p4 - 32.0 * in_p2;
                effxct += chebyshev * eighth;
            }
            if ninth != 0.0 {
                chebyshev = 256.0 * in_p9 - 576.0 * in_p7 + 432.0 * in_p5 - 120.0 * in_p3 + 9.0 * input_sample_r;
                effxct += chebyshev * ninth;
            }
            if tenth != 0.0 {
                chebyshev = 512.0 * in_p10 - 1280.0 * in_p8 + 1120.0 * in_p6 - 400.0 * in_p4 + 50.0 * in_p2;
                effxct += chebyshev * tenth;
            }
            if eleventh != 0.0 {
                chebyshev = 1024.0 * in_p11 - 2816.0 * in_p9 + 2816.0 * in_p7 - 1232.0 * in_p5 + 220.0 * in_p3 - 11.0 * input_sample_r;
                effxct += chebyshev * eleventh;
            }
            if twelvth != 0.0 {
                chebyshev = 2048.0 * in_p12 - 6144.0 * in_p10 + 6912.0 * in_p8 - 3584.0 * in_p6 + 840.0 * in_p4 - 72.0 * in_p2;
                effxct += chebyshev * twelvth;
            }
            if thirteenth != 0.0 {
                chebyshev = 4096.0 * in_p13 - 13312.0 * in_p11 + 16640.0 * in_p9 - 9984.0 * in_p7 + 2912.0 * in_p5 - 364.0 * in_p3 + 13.0 * input_sample_r;
                effxct += chebyshev * thirteenth;
            }
            input_sample_r += effxct * amount;
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
