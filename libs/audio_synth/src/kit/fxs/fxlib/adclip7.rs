// Ported from ADClip7 (ADClip7Proc.cpp, ADClip7.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! AD Clip (sub-effect id 0): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    SlotDef::float("Boost", 0.0, 1.0, 0.0, crate::kit::core::SlotUnit::None),
    SlotDef::float("Soften", 0.0, 1.0, 0.5, crate::kit::core::SlotUnit::None),
    SlotDef::float("Enhance", 0.0, 1.0, 0.5, crate::kit::core::SlotUnit::None),
    SlotDef::choice("Mode", &["Normal", "Atten", "Clips"], 0.0),
];

#[allow(dead_code)]
pub struct AdClip7 {
    p: [f32; 4],
    sr: f32,
    fpd_l: u32,
    fpd_r: u32,
    last_sample_l: f64,
    last_sample_r: f64,
    b_l: Vec<f32>,
    b_r: Vec<f32>,
    gcount: i32,
    lows_l: f64,
    lows_r: f64,
    iir_lows_al: f64,
    iir_lows_ar: f64,
    iir_lows_bl: f64,
    iir_lows_br: f64,
    refclip_l: f64,
    refclip_r: f64,
}

impl AdClip7 {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<AdClip7> {
        let mut s = Box::new(AdClip7 {
            p: [0.0, 0.5, 0.5, 0.0],
            sr: sr as f32,
            fpd_l: 0,
            fpd_r: 0,
            last_sample_l: 0.0,
            last_sample_r: 0.0,
            b_l: vec![0.0; 22200],
            b_r: vec![0.0; 22200],
            gcount: 0,
            lows_l: 0.0,
            lows_r: 0.0,
            iir_lows_al: 0.0,
            iir_lows_ar: 0.0,
            iir_lows_bl: 0.0,
            iir_lows_br: 0.0,
            refclip_l: 0.0,
            refclip_r: 0.0,
        });
        s.refclip_l = 0.99;
        s.refclip_r = 0.99;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for AdClip7 {
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
        let mut input_gain: f64 = 10.0_f64.powf(s.p[0] as f64 * 18.0 / 20.0);
        let mut softness: f64 = s.p[1] as f64 * fp_new;
        let mut hardness: f64 = 1.0 - softness;
        let mut highslift: f64 = 0.307 * s.p[2] as f64;
        let mut adjust: f64 = highslift.powf(3.0) * 0.416;
        let mut subslift: f64 = 0.796 * s.p[2] as f64;
        let mut calibsubs: f64 = subslift / 53.0;
        let mut invcalibsubs: f64 = 1.0 - calibsubs;
        let mut subs: f64 = 0.81 + calibsubs * 2.0;
        let mut bridgerectifier: f64 = 0.0;
        let mut mode: i32 = (s.p[3] as f64 * 2.999).floor() as i32 + 1;
        let mut overshoot_l: f64 = 0.0;
        let mut overshoot_r: f64 = 0.0;
        let mut offset_h1: f64 = 1.84;
        offset_h1 *= overallscale;
        let mut offset_h2: f64 = offset_h1 * 1.9;
        let mut offset_h3: f64 = offset_h1 * 2.7;
        let mut offset_l1: f64 = 612.0;
        offset_l1 *= overallscale;
        let mut offset_l2: f64 = offset_l1 * 2.0;
        let mut ref_h1: i32 = offset_h1.floor() as i32;
        let mut ref_h2: i32 = offset_h2.floor() as i32;
        let mut ref_h3: i32 = offset_h3.floor() as i32;
        let mut ref_l1: i32 = offset_l1.floor() as i32;
        let mut ref_l2: i32 = offset_l2.floor() as i32;
        let mut temp: i32 = 0;
        let mut fraction_h1: f64 = offset_h1 - offset_h1.floor();
        let mut fraction_h2: f64 = offset_h2 - offset_h2.floor();
        let mut fraction_h3: f64 = offset_h3 - offset_h3.floor();
        let mut minus_h1: f64 = 1.0 - fraction_h1;
        let mut minus_h2: f64 = 1.0 - fraction_h2;
        let mut minus_h3: f64 = 1.0 - fraction_h3;
        let mut highs_l: f64 = 0.0;
        let mut highs_r: f64 = 0.0;
        let mut count: i32 = 0;
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
            if input_gain != 1.0 {
                input_sample_l *= input_gain;
                input_sample_r *= input_gain;
            }
            overshoot_l = input_sample_l.abs() - s.refclip_l;
            overshoot_r = input_sample_r.abs() - s.refclip_r;
            if overshoot_l < 0.0 {
                overshoot_l = 0.0;
            }
            if overshoot_r < 0.0 {
                overshoot_r = 0.0;
            }
            if s.gcount < 0 || s.gcount > 11020 {
                s.gcount = 11020;
            }
            count = s.gcount;
            s.b_l[count as usize] = overshoot_l as f32;
            s.b_l[(count + 11020) as usize] = s.b_l[count as usize];
            s.b_r[count as usize] = overshoot_r as f32;
            s.b_r[(count + 11020) as usize] = s.b_r[count as usize];
            s.gcount -= 1;
            if highslift > 0.0 {
                temp = count + ref_h3;
                highs_l = -(s.b_l[temp as usize] as f64 * minus_h3);
                highs_l -= s.b_l[(temp + 1) as usize] as f64;
                highs_l -= s.b_l[(temp + 2) as usize] as f64 * fraction_h3;
                highs_l += ((s.b_l[temp as usize] - s.b_l[(temp + 1) as usize] - (s.b_l[(temp + 1) as usize] - s.b_l[(temp + 2) as usize])) / 50.0) as f64;
                highs_l *= adjust;
                highs_r = -(s.b_r[temp as usize] as f64 * minus_h3);
                highs_r -= s.b_r[(temp + 1) as usize] as f64;
                highs_r -= s.b_r[(temp + 2) as usize] as f64 * fraction_h3;
                highs_r += ((s.b_r[temp as usize] - s.b_r[(temp + 1) as usize] - (s.b_r[(temp + 1) as usize] - s.b_r[(temp + 2) as usize])) / 50.0) as f64;
                highs_r *= adjust;
                temp = count + ref_h2;
                highs_l += s.b_l[temp as usize] as f64 * minus_h2;
                highs_l += s.b_l[(temp + 1) as usize] as f64;
                highs_l += s.b_l[(temp + 2) as usize] as f64 * fraction_h2;
                highs_l -= ((s.b_l[temp as usize] - s.b_l[(temp + 1) as usize] - (s.b_l[(temp + 1) as usize] - s.b_l[(temp + 2) as usize])) / 50.0) as f64;
                highs_l *= adjust;
                highs_r += s.b_r[temp as usize] as f64 * minus_h2;
                highs_r += s.b_r[(temp + 1) as usize] as f64;
                highs_r += s.b_r[(temp + 2) as usize] as f64 * fraction_h2;
                highs_r -= ((s.b_r[temp as usize] - s.b_r[(temp + 1) as usize] - (s.b_r[(temp + 1) as usize] - s.b_r[(temp + 2) as usize])) / 50.0) as f64;
                highs_r *= adjust;
                temp = count + ref_h1;
                highs_l -= s.b_l[temp as usize] as f64 * minus_h1;
                highs_l -= s.b_l[(temp + 1) as usize] as f64;
                highs_l -= s.b_l[(temp + 2) as usize] as f64 * fraction_h1;
                highs_l += ((s.b_l[temp as usize] - s.b_l[(temp + 1) as usize] - (s.b_l[(temp + 1) as usize] - s.b_l[(temp + 2) as usize])) / 50.0) as f64;
                highs_l *= adjust;
                highs_r -= s.b_r[temp as usize] as f64 * minus_h1;
                highs_r -= s.b_r[(temp + 1) as usize] as f64;
                highs_r -= s.b_r[(temp + 2) as usize] as f64 * fraction_h1;
                highs_r += ((s.b_r[temp as usize] - s.b_r[(temp + 1) as usize] - (s.b_r[(temp + 1) as usize] - s.b_r[(temp + 2) as usize])) / 50.0) as f64;
                highs_r *= adjust;
            }
            bridgerectifier = (highs_l.abs() * hardness).sin();
            if highs_l > 0.0 {
                highs_l = bridgerectifier;
            } else {
                highs_l = -bridgerectifier;
            }
            bridgerectifier = (highs_r.abs() * hardness).sin();
            if highs_r > 0.0 {
                highs_r = bridgerectifier;
            } else {
                highs_r = -bridgerectifier;
            }
            if subslift > 0.0 {
                s.lows_l *= subs;
                s.lows_r *= subs;
                temp = count + ref_l1;
                s.lows_l -= s.b_l[(temp + 127) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 113) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 109) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 107) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 103) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 101) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 97) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 89) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 83) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 79) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 73) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 71) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 67) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 61) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 59) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 53) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 47) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 43) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 41) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 37) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 31) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 29) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 23) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 19) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 17) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 13) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 11) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 7) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 5) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 3) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 2) as usize] as f64;
                s.lows_l -= s.b_l[(temp + 1) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 127) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 113) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 109) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 107) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 103) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 101) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 97) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 89) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 83) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 79) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 73) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 71) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 67) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 61) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 59) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 53) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 47) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 43) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 41) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 37) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 31) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 29) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 23) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 19) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 17) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 13) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 11) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 7) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 5) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 3) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 2) as usize] as f64;
                s.lows_r -= s.b_r[(temp + 1) as usize] as f64;
                s.lows_l *= subs;
                s.lows_l *= subs;
                s.lows_r *= subs;
                s.lows_r *= subs;
                temp = count + ref_l2;
                s.lows_l += s.b_l[(temp + 127) as usize] as f64;
                s.lows_l += s.b_l[(temp + 113) as usize] as f64;
                s.lows_l += s.b_l[(temp + 109) as usize] as f64;
                s.lows_l += s.b_l[(temp + 107) as usize] as f64;
                s.lows_l += s.b_l[(temp + 103) as usize] as f64;
                s.lows_l += s.b_l[(temp + 101) as usize] as f64;
                s.lows_l += s.b_l[(temp + 97) as usize] as f64;
                s.lows_l += s.b_l[(temp + 89) as usize] as f64;
                s.lows_l += s.b_l[(temp + 83) as usize] as f64;
                s.lows_l += s.b_l[(temp + 79) as usize] as f64;
                s.lows_l += s.b_l[(temp + 73) as usize] as f64;
                s.lows_l += s.b_l[(temp + 71) as usize] as f64;
                s.lows_l += s.b_l[(temp + 67) as usize] as f64;
                s.lows_l += s.b_l[(temp + 61) as usize] as f64;
                s.lows_l += s.b_l[(temp + 59) as usize] as f64;
                s.lows_l += s.b_l[(temp + 53) as usize] as f64;
                s.lows_l += s.b_l[(temp + 47) as usize] as f64;
                s.lows_l += s.b_l[(temp + 43) as usize] as f64;
                s.lows_l += s.b_l[(temp + 41) as usize] as f64;
                s.lows_l += s.b_l[(temp + 37) as usize] as f64;
                s.lows_l += s.b_l[(temp + 31) as usize] as f64;
                s.lows_l += s.b_l[(temp + 29) as usize] as f64;
                s.lows_l += s.b_l[(temp + 23) as usize] as f64;
                s.lows_l += s.b_l[(temp + 19) as usize] as f64;
                s.lows_l += s.b_l[(temp + 17) as usize] as f64;
                s.lows_l += s.b_l[(temp + 13) as usize] as f64;
                s.lows_l += s.b_l[(temp + 11) as usize] as f64;
                s.lows_l += s.b_l[(temp + 7) as usize] as f64;
                s.lows_l += s.b_l[(temp + 5) as usize] as f64;
                s.lows_l += s.b_l[(temp + 3) as usize] as f64;
                s.lows_l += s.b_l[(temp + 2) as usize] as f64;
                s.lows_l += s.b_l[(temp + 1) as usize] as f64;
                s.lows_r += s.b_r[(temp + 127) as usize] as f64;
                s.lows_r += s.b_r[(temp + 113) as usize] as f64;
                s.lows_r += s.b_r[(temp + 109) as usize] as f64;
                s.lows_r += s.b_r[(temp + 107) as usize] as f64;
                s.lows_r += s.b_r[(temp + 103) as usize] as f64;
                s.lows_r += s.b_r[(temp + 101) as usize] as f64;
                s.lows_r += s.b_r[(temp + 97) as usize] as f64;
                s.lows_r += s.b_r[(temp + 89) as usize] as f64;
                s.lows_r += s.b_r[(temp + 83) as usize] as f64;
                s.lows_r += s.b_r[(temp + 79) as usize] as f64;
                s.lows_r += s.b_r[(temp + 73) as usize] as f64;
                s.lows_r += s.b_r[(temp + 71) as usize] as f64;
                s.lows_r += s.b_r[(temp + 67) as usize] as f64;
                s.lows_r += s.b_r[(temp + 61) as usize] as f64;
                s.lows_r += s.b_r[(temp + 59) as usize] as f64;
                s.lows_r += s.b_r[(temp + 53) as usize] as f64;
                s.lows_r += s.b_r[(temp + 47) as usize] as f64;
                s.lows_r += s.b_r[(temp + 43) as usize] as f64;
                s.lows_r += s.b_r[(temp + 41) as usize] as f64;
                s.lows_r += s.b_r[(temp + 37) as usize] as f64;
                s.lows_r += s.b_r[(temp + 31) as usize] as f64;
                s.lows_r += s.b_r[(temp + 29) as usize] as f64;
                s.lows_r += s.b_r[(temp + 23) as usize] as f64;
                s.lows_r += s.b_r[(temp + 19) as usize] as f64;
                s.lows_r += s.b_r[(temp + 17) as usize] as f64;
                s.lows_r += s.b_r[(temp + 13) as usize] as f64;
                s.lows_r += s.b_r[(temp + 11) as usize] as f64;
                s.lows_r += s.b_r[(temp + 7) as usize] as f64;
                s.lows_r += s.b_r[(temp + 5) as usize] as f64;
                s.lows_r += s.b_r[(temp + 3) as usize] as f64;
                s.lows_r += s.b_r[(temp + 2) as usize] as f64;
                s.lows_r += s.b_r[(temp + 1) as usize] as f64;
                s.lows_l *= subs;
                s.lows_r *= subs;
            }
            bridgerectifier = (s.lows_l.abs() * softness).sin();
            if s.lows_l > 0.0 {
                s.lows_l = bridgerectifier;
            } else {
                s.lows_l = -bridgerectifier;
            }
            bridgerectifier = (s.lows_r.abs() * softness).sin();
            if s.lows_r > 0.0 {
                s.lows_r = bridgerectifier;
            } else {
                s.lows_r = -bridgerectifier;
            }
            s.iir_lows_al = s.iir_lows_al * invcalibsubs + s.lows_l * calibsubs;
            s.lows_l = s.iir_lows_al;
            bridgerectifier = s.lows_l.abs().sin();
            if s.lows_l > 0.0 {
                s.lows_l = bridgerectifier;
            } else {
                s.lows_l = -bridgerectifier;
            }
            s.iir_lows_ar = s.iir_lows_ar * invcalibsubs + s.lows_r * calibsubs;
            s.lows_r = s.iir_lows_ar;
            bridgerectifier = s.lows_r.abs().sin();
            if s.lows_r > 0.0 {
                s.lows_r = bridgerectifier;
            } else {
                s.lows_r = -bridgerectifier;
            }
            s.iir_lows_bl = s.iir_lows_bl * invcalibsubs + s.lows_l * calibsubs;
            s.lows_l = s.iir_lows_bl;
            bridgerectifier = s.lows_l.abs().sin() * 2.0;
            if s.lows_l > 0.0 {
                s.lows_l = bridgerectifier;
            } else {
                s.lows_l = -bridgerectifier;
            }
            s.iir_lows_br = s.iir_lows_br * invcalibsubs + s.lows_r * calibsubs;
            s.lows_r = s.iir_lows_br;
            bridgerectifier = s.lows_r.abs().sin() * 2.0;
            if s.lows_r > 0.0 {
                s.lows_r = bridgerectifier;
            } else {
                s.lows_r = -bridgerectifier;
            }
            if highslift > 0.0 {
                input_sample_l += highs_l * (1.0 - (input_sample_l * hardness).abs());
            }
            if subslift > 0.0 {
                input_sample_l += s.lows_l * (1.0 - (input_sample_l * softness).abs());
            }
            if highslift > 0.0 {
                input_sample_r += highs_r * (1.0 - (input_sample_r * hardness).abs());
            }
            if subslift > 0.0 {
                input_sample_r += s.lows_r * (1.0 - (input_sample_r * softness).abs());
            }
            if input_sample_l > s.refclip_l && s.refclip_l > 0.9 {
                s.refclip_l -= 0.01;
            }
            if input_sample_l < -s.refclip_l && s.refclip_l > 0.9 {
                s.refclip_l -= 0.01;
            }
            if s.refclip_l < 0.99 {
                s.refclip_l += 0.00001;
            }
            if input_sample_r > s.refclip_r && s.refclip_r > 0.9 {
                s.refclip_r -= 0.01;
            }
            if input_sample_r < -s.refclip_r && s.refclip_r > 0.9 {
                s.refclip_r -= 0.01;
            }
            if s.refclip_r < 0.99 {
                s.refclip_r += 0.00001;
            }
            if s.last_sample_l >= s.refclip_l {
                if input_sample_l < s.refclip_l {
                    s.last_sample_l = s.refclip_l * hardness + input_sample_l * softness;
                } else {
                    s.last_sample_l = s.refclip_l;
                }
            }
            if s.last_sample_r >= s.refclip_r {
                if input_sample_r < s.refclip_r {
                    s.last_sample_r = s.refclip_r * hardness + input_sample_r * softness;
                } else {
                    s.last_sample_r = s.refclip_r;
                }
            }
            if s.last_sample_l <= -s.refclip_l {
                if input_sample_l > -s.refclip_l {
                    s.last_sample_l = -s.refclip_l * hardness + input_sample_l * softness;
                } else {
                    s.last_sample_l = -s.refclip_l;
                }
            }
            if s.last_sample_r <= -s.refclip_r {
                if input_sample_r > -s.refclip_r {
                    s.last_sample_r = -s.refclip_r * hardness + input_sample_r * softness;
                } else {
                    s.last_sample_r = -s.refclip_r;
                }
            }
            if input_sample_l > s.refclip_l {
                if s.last_sample_l < s.refclip_l {
                    input_sample_l = s.refclip_l * hardness + s.last_sample_l * softness;
                } else {
                    input_sample_l = s.refclip_l;
                }
            }
            if input_sample_r > s.refclip_r {
                if s.last_sample_r < s.refclip_r {
                    input_sample_r = s.refclip_r * hardness + s.last_sample_r * softness;
                } else {
                    input_sample_r = s.refclip_r;
                }
            }
            if input_sample_l < -s.refclip_l {
                if s.last_sample_l > -s.refclip_l {
                    input_sample_l = -s.refclip_l * hardness + s.last_sample_l * softness;
                } else {
                    input_sample_l = -s.refclip_l;
                }
            }
            if input_sample_r < -s.refclip_r {
                if s.last_sample_r > -s.refclip_r {
                    input_sample_r = -s.refclip_r * hardness + s.last_sample_r * softness;
                } else {
                    input_sample_r = -s.refclip_r;
                }
            }
            s.last_sample_l = input_sample_l;
            s.last_sample_r = input_sample_r;
            match mode {
                1 => {
                }
                2 => {
                    input_sample_l /= input_gain;
                    input_sample_r /= input_gain;
                }
                3 => {
                    input_sample_l = overshoot_l + highs_l + s.lows_l;
                    input_sample_r = overshoot_r + highs_r + s.lows_r;
                }
                _ => {}
            }
            if input_sample_l > s.refclip_l {
                input_sample_l = s.refclip_l;
            }
            if input_sample_l < -s.refclip_l {
                input_sample_l = -s.refclip_l;
            }
            if input_sample_r > s.refclip_r {
                input_sample_r = s.refclip_r;
            }
            if input_sample_r < -s.refclip_r {
                input_sample_r = -s.refclip_r;
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
