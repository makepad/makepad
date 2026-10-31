// Ported from OneCornerClip (OneCornerClipProc.cpp, OneCornerClip.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! One Corner Clip (sub-effect id 6): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Input", 0.3333333333333333),
    pf("Pos Thr", 0.966),
    pf("Neg Thr", 0.966),
    pf("Voicing", 0.618),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct OneCornerClip {
    p: [f32; 5],
    sr: f32,
    fpd_l: u32,
    fpd_r: u32,
    last_sample_l: f64,
    limit_pos_l: f64,
    limit_neg_l: f64,
    last_sample_r: f64,
    limit_pos_r: f64,
    limit_neg_r: f64,
}

impl OneCornerClip {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<OneCornerClip> {
        let mut s = Box::new(OneCornerClip {
            p: [0.3333333333333333, 0.966, 0.966, 0.618, 1.0],
            sr: sr as f32,
            fpd_l: 0,
            fpd_r: 0,
            last_sample_l: 0.0,
            limit_pos_l: 0.0,
            limit_neg_l: 0.0,
            last_sample_r: 0.0,
            limit_pos_r: 0.0,
            limit_neg_r: 0.0,
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for OneCornerClip {
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
        let mut input_gain: f64 = 10.0_f64.powf((s.p[0] as f64 * 36.0 - 12.0) / 20.0);
        let mut pos_threshold: f64 = s.p[1] as f64;
        let mut pos_target_l: f64 = pos_threshold;
        let mut pos_target_r: f64 = pos_threshold;
        let mut neg_threshold: f64 = -s.p[2] as f64;
        let mut neg_target_l: f64 = neg_threshold;
        let mut neg_target_r: f64 = neg_threshold;
        let mut voicing: f64 = s.p[3] as f64;
        if voicing == 0.618 {
            voicing = 0.618033988749894848204586;
        }
        if overallscale > 0.0 {
            voicing /= overallscale;
        }
        if voicing < 0.0 {
            voicing = 0.0;
        }
        if voicing > 1.0 {
            voicing = 1.0;
        }
        let mut inverse_hardness: f64 = 1.0 - voicing;
        let mut clip_engage: bool = false;
        let mut wet: f64 = s.p[4] as f64;
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
            if input_gain != 1.0 {
                input_sample_l *= input_gain;
                input_sample_r *= input_gain;
                clip_engage = true;
            } else {
                clip_engage = false;
            }
            if input_sample_l > pos_target_l {
                input_sample_l = s.last_sample_l * voicing + pos_threshold * inverse_hardness;
                pos_target_l = input_sample_l;
                clip_engage = true;
            } else {
                pos_target_l = pos_threshold;
            }
            if input_sample_r > pos_target_r {
                input_sample_r = s.last_sample_r * voicing + pos_threshold * inverse_hardness;
                pos_target_r = input_sample_r;
                clip_engage = true;
            } else {
                pos_target_r = pos_threshold;
            }
            if input_sample_l < neg_target_l {
                input_sample_l = s.last_sample_l * voicing + neg_threshold * inverse_hardness;
                neg_target_l = input_sample_l;
                clip_engage = true;
            } else {
                neg_target_l = neg_threshold;
            }
            if input_sample_r < neg_target_r {
                input_sample_r = s.last_sample_r * voicing + neg_threshold * inverse_hardness;
                neg_target_r = input_sample_r;
                clip_engage = true;
            } else {
                neg_target_r = neg_threshold;
            }
            s.last_sample_l = input_sample_l;
            s.last_sample_r = input_sample_r;
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
            if clip_engage as i32 == false as i32 {
                input_sample_l = l[smp] as f64;
                input_sample_r = r[smp] as f64;
            }
            l[smp] = input_sample_l as f32;
            r[smp] = input_sample_r as f32;
        }
    }
}
