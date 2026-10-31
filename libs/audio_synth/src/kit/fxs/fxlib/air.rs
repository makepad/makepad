// Ported from Air (AirProc.cpp, Air.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Air (sub-effect id 43): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("22K tap", 0.5),
    pf("15K tap", 0.5),
    pf("11K tap", 0.5),
    pf("filters Q", 0.0),
    pf("Output Level", 1.0),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct Air {
    p: [f32; 6],
    sr: f32,
    air_prev_al: f64,
    air_even_al: f64,
    air_odd_al: f64,
    air_factor_al: f64,
    air_prev_bl: f64,
    air_even_bl: f64,
    air_odd_bl: f64,
    air_factor_bl: f64,
    air_prev_cl: f64,
    air_even_cl: f64,
    air_odd_cl: f64,
    air_factor_cl: f64,
    triplet_prev_l: f64,
    triplet_mid_l: f64,
    triplet_al: f64,
    triplet_bl: f64,
    triplet_cl: f64,
    triplet_factor_l: f64,
    air_prev_ar: f64,
    air_even_ar: f64,
    air_odd_ar: f64,
    air_factor_ar: f64,
    air_prev_br: f64,
    air_even_br: f64,
    air_odd_br: f64,
    air_factor_br: f64,
    air_prev_cr: f64,
    air_even_cr: f64,
    air_odd_cr: f64,
    air_factor_cr: f64,
    triplet_prev_r: f64,
    triplet_mid_r: f64,
    triplet_ar: f64,
    triplet_br: f64,
    triplet_cr: f64,
    triplet_factor_r: f64,
    flip_a: bool,
    flip_b: bool,
    flop: bool,
    count: i32,
    fpd_l: u32,
    fpd_r: u32,
}

impl Air {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Air> {
        let mut s = Box::new(Air {
            p: [0.5, 0.5, 0.5, 0.0, 1.0, 1.0],
            sr: sr as f32,
            air_prev_al: 0.0,
            air_even_al: 0.0,
            air_odd_al: 0.0,
            air_factor_al: 0.0,
            air_prev_bl: 0.0,
            air_even_bl: 0.0,
            air_odd_bl: 0.0,
            air_factor_bl: 0.0,
            air_prev_cl: 0.0,
            air_even_cl: 0.0,
            air_odd_cl: 0.0,
            air_factor_cl: 0.0,
            triplet_prev_l: 0.0,
            triplet_mid_l: 0.0,
            triplet_al: 0.0,
            triplet_bl: 0.0,
            triplet_cl: 0.0,
            triplet_factor_l: 0.0,
            air_prev_ar: 0.0,
            air_even_ar: 0.0,
            air_odd_ar: 0.0,
            air_factor_ar: 0.0,
            air_prev_br: 0.0,
            air_even_br: 0.0,
            air_odd_br: 0.0,
            air_factor_br: 0.0,
            air_prev_cr: 0.0,
            air_even_cr: 0.0,
            air_odd_cr: 0.0,
            air_factor_cr: 0.0,
            triplet_prev_r: 0.0,
            triplet_mid_r: 0.0,
            triplet_ar: 0.0,
            triplet_br: 0.0,
            triplet_cr: 0.0,
            triplet_factor_r: 0.0,
            flip_a: false,
            flip_b: false,
            flop: false,
            count: 0,
            fpd_l: 0,
            fpd_r: 0,
        });
        s.count = 1;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for Air {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut hi_intensity: f64 = -(s.p[0] as f64 * 2.0 - 1.0).powf(3.0) * 2.0;
        let mut tripletintensity: f64 = -(s.p[1] as f64 * 2.0 - 1.0).powf(3.0);
        let mut air_intensity: f64 = -(s.p[2] as f64 * 2.0 - 1.0).powf(3.0) / 2.0;
        let mut filter_q: f64 = 2.1 - s.p[3] as f64;
        let mut output: f64 = s.p[4] as f64;
        let mut wet: f64 = s.p[5] as f64;
        let mut dry: f64 = 1.0 - wet;
        let mut input_sample_l: f64 = 0.0;
        let mut input_sample_r: f64 = 0.0;
        let mut dry_sample_l: f64 = 0.0;
        let mut dry_sample_r: f64 = 0.0;
        let mut correction_l: f64 = 0.0;
        let mut correction_r: f64 = 0.0;
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
            correction_l = 0.0;
            correction_r = 0.0;
            if s.count < 1 || s.count > 3 {
                s.count = 1;
            }
            s.triplet_factor_l = s.triplet_prev_l - input_sample_l;
            s.triplet_factor_r = s.triplet_prev_r - input_sample_r;
            match s.count {
                1 => {
                    s.triplet_al += s.triplet_factor_l;
                    s.triplet_cl -= s.triplet_factor_l;
                    s.triplet_factor_l = s.triplet_al * tripletintensity;
                    s.triplet_prev_l = s.triplet_mid_l;
                    s.triplet_mid_l = input_sample_l;
                    s.triplet_ar += s.triplet_factor_r;
                    s.triplet_cr -= s.triplet_factor_r;
                    s.triplet_factor_r = s.triplet_ar * tripletintensity;
                    s.triplet_prev_r = s.triplet_mid_r;
                    s.triplet_mid_r = input_sample_r;
                }
                2 => {
                    s.triplet_bl += s.triplet_factor_l;
                    s.triplet_al -= s.triplet_factor_l;
                    s.triplet_factor_l = s.triplet_bl * tripletintensity;
                    s.triplet_prev_l = s.triplet_mid_l;
                    s.triplet_mid_l = input_sample_l;
                    s.triplet_br += s.triplet_factor_r;
                    s.triplet_ar -= s.triplet_factor_r;
                    s.triplet_factor_r = s.triplet_br * tripletintensity;
                    s.triplet_prev_r = s.triplet_mid_r;
                    s.triplet_mid_r = input_sample_r;
                }
                3 => {
                    s.triplet_cl += s.triplet_factor_l;
                    s.triplet_bl -= s.triplet_factor_l;
                    s.triplet_factor_l = s.triplet_cl * tripletintensity;
                    s.triplet_prev_l = s.triplet_mid_l;
                    s.triplet_mid_l = input_sample_l;
                    s.triplet_cr += s.triplet_factor_r;
                    s.triplet_br -= s.triplet_factor_r;
                    s.triplet_factor_r = s.triplet_cr * tripletintensity;
                    s.triplet_prev_r = s.triplet_mid_r;
                    s.triplet_mid_r = input_sample_r;
                }
                _ => {}
            }
            s.triplet_al /= filter_q;
            s.triplet_bl /= filter_q;
            s.triplet_cl /= filter_q;
            correction_l = correction_l + s.triplet_factor_l;
            s.triplet_ar /= filter_q;
            s.triplet_br /= filter_q;
            s.triplet_cr /= filter_q;
            correction_r = correction_r + s.triplet_factor_r;
            s.count += 1;
            if s.flop {
                s.air_factor_al = s.air_prev_al - input_sample_l;
                s.air_factor_ar = s.air_prev_ar - input_sample_r;
                if s.flip_a {
                    s.air_even_al += s.air_factor_al;
                    s.air_odd_al -= s.air_factor_al;
                    s.air_factor_al = s.air_even_al * air_intensity;
                    s.air_even_ar += s.air_factor_ar;
                    s.air_odd_ar -= s.air_factor_ar;
                    s.air_factor_ar = s.air_even_ar * air_intensity;
                } else {
                    s.air_odd_al += s.air_factor_al;
                    s.air_even_al -= s.air_factor_al;
                    s.air_factor_al = s.air_odd_al * air_intensity;
                    s.air_odd_ar += s.air_factor_ar;
                    s.air_even_ar -= s.air_factor_ar;
                    s.air_factor_ar = s.air_odd_ar * air_intensity;
                }
                s.air_odd_al = (s.air_odd_al - (s.air_odd_al - s.air_even_al) / 256.0) / filter_q;
                s.air_even_al = (s.air_even_al - (s.air_even_al - s.air_odd_al) / 256.0) / filter_q;
                s.air_prev_al = input_sample_l;
                correction_l = correction_l + s.air_factor_al;
                s.air_odd_ar = (s.air_odd_ar - (s.air_odd_ar - s.air_even_ar) / 256.0) / filter_q;
                s.air_even_ar = (s.air_even_ar - (s.air_even_ar - s.air_odd_ar) / 256.0) / filter_q;
                s.air_prev_ar = input_sample_r;
                correction_r = correction_r + s.air_factor_ar;
                s.flip_a = !s.flip_a;
            } else {
                s.air_factor_bl = s.air_prev_bl - input_sample_l;
                s.air_factor_br = s.air_prev_br - input_sample_r;
                if s.flip_b {
                    s.air_even_bl += s.air_factor_bl;
                    s.air_odd_bl -= s.air_factor_bl;
                    s.air_factor_bl = s.air_even_bl * air_intensity;
                    s.air_even_br += s.air_factor_br;
                    s.air_odd_br -= s.air_factor_br;
                    s.air_factor_br = s.air_even_br * air_intensity;
                } else {
                    s.air_odd_bl += s.air_factor_bl;
                    s.air_even_bl -= s.air_factor_bl;
                    s.air_factor_bl = s.air_odd_bl * air_intensity;
                    s.air_odd_br += s.air_factor_br;
                    s.air_even_br -= s.air_factor_br;
                    s.air_factor_br = s.air_odd_br * air_intensity;
                }
                s.air_odd_bl = (s.air_odd_bl - (s.air_odd_bl - s.air_even_bl) / 256.0) / filter_q;
                s.air_even_bl = (s.air_even_bl - (s.air_even_bl - s.air_odd_bl) / 256.0) / filter_q;
                s.air_prev_bl = input_sample_l;
                correction_l = correction_l + s.air_factor_bl;
                s.air_odd_br = (s.air_odd_br - (s.air_odd_br - s.air_even_br) / 256.0) / filter_q;
                s.air_even_br = (s.air_even_br - (s.air_even_br - s.air_odd_br) / 256.0) / filter_q;
                s.air_prev_br = input_sample_r;
                correction_r = correction_r + s.air_factor_br;
                s.flip_b = !s.flip_b;
            }
            s.air_factor_cl = s.air_prev_cl - input_sample_l;
            s.air_factor_cr = s.air_prev_cr - input_sample_r;
            if s.flop {
                s.air_even_cl += s.air_factor_cl;
                s.air_odd_cl -= s.air_factor_cl;
                s.air_factor_cl = s.air_even_cl * hi_intensity;
                s.air_even_cr += s.air_factor_cr;
                s.air_odd_cr -= s.air_factor_cr;
                s.air_factor_cr = s.air_even_cr * hi_intensity;
            } else {
                s.air_odd_cl += s.air_factor_cl;
                s.air_even_cl -= s.air_factor_cl;
                s.air_factor_cl = s.air_odd_cl * hi_intensity;
                s.air_odd_cr += s.air_factor_cr;
                s.air_even_cr -= s.air_factor_cr;
                s.air_factor_cr = s.air_odd_cr * hi_intensity;
            }
            s.air_odd_cl = (s.air_odd_cl - (s.air_odd_cl - s.air_even_cl) / 256.0) / filter_q;
            s.air_even_cl = (s.air_even_cl - (s.air_even_cl - s.air_odd_cl) / 256.0) / filter_q;
            s.air_prev_cl = input_sample_l;
            correction_l = correction_l + s.air_factor_cl;
            s.air_odd_cr = (s.air_odd_cr - (s.air_odd_cr - s.air_even_cr) / 256.0) / filter_q;
            s.air_even_cr = (s.air_even_cr - (s.air_even_cr - s.air_odd_cr) / 256.0) / filter_q;
            s.air_prev_cr = input_sample_r;
            correction_r = correction_r + s.air_factor_cr;
            s.flop = !s.flop;
            input_sample_l += correction_l;
            input_sample_r += correction_r;
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
