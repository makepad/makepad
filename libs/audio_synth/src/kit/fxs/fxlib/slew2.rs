// Ported from Slew2 (Slew2Proc.cpp, Slew2.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Slew 2 (sub-effect id 48): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Clamping", 0.0),
];

#[allow(dead_code)]
pub struct Slew2 {
    p: [f32; 1],
    sr: f32,
    lata_last3_sample: f64,
    lata_last2_sample: f64,
    lata_last1_sample: f64,
    lata_halfway_sample: f64,
    lata_half_dry_sample: f64,
    lata_half_diff_sample: f64,
    lata_a: f64,
    lata_b: f64,
    lata_c: f64,
    lata_decay: f64,
    lata_upsample_high_tweak: f64,
    lata_dry_sample: f64,
    lata_diff_sample: f64,
    lata_prev_diff_sample: f64,
    rata_last3_sample: f64,
    rata_last2_sample: f64,
    rata_last1_sample: f64,
    rata_halfway_sample: f64,
    rata_half_dry_sample: f64,
    rata_half_diff_sample: f64,
    rata_a: f64,
    rata_b: f64,
    rata_c: f64,
    rata_decay: f64,
    rata_upsample_high_tweak: f64,
    rata_dry_sample: f64,
    rata_diff_sample: f64,
    rata_prev_diff_sample: f64,
    lata_flip: bool,
    rata_flip: bool,
    last_sample_l: f64,
    last_sample_r: f64,
    fpd_l: u32,
    fpd_r: u32,
}

impl Slew2 {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Slew2> {
        let mut s = Box::new(Slew2 {
            p: [0.0],
            sr: sr as f32,
            lata_last3_sample: 0.0,
            lata_last2_sample: 0.0,
            lata_last1_sample: 0.0,
            lata_halfway_sample: 0.0,
            lata_half_dry_sample: 0.0,
            lata_half_diff_sample: 0.0,
            lata_a: 0.0,
            lata_b: 0.0,
            lata_c: 0.0,
            lata_decay: 0.0,
            lata_upsample_high_tweak: 0.0,
            lata_dry_sample: 0.0,
            lata_diff_sample: 0.0,
            lata_prev_diff_sample: 0.0,
            rata_last3_sample: 0.0,
            rata_last2_sample: 0.0,
            rata_last1_sample: 0.0,
            rata_halfway_sample: 0.0,
            rata_half_dry_sample: 0.0,
            rata_half_diff_sample: 0.0,
            rata_a: 0.0,
            rata_b: 0.0,
            rata_c: 0.0,
            rata_decay: 0.0,
            rata_upsample_high_tweak: 0.0,
            rata_dry_sample: 0.0,
            rata_diff_sample: 0.0,
            rata_prev_diff_sample: 0.0,
            lata_flip: false,
            rata_flip: false,
            last_sample_l: 0.0,
            last_sample_r: 0.0,
            fpd_l: 0,
            fpd_r: 0,
        });
        s.lata_upsample_high_tweak = 0.0414213562373095048801688;
        s.lata_decay = 0.915965594177219015;
        s.rata_upsample_high_tweak = 0.0414213562373095048801688;
        s.rata_decay = 0.915965594177219015;
        s
    }
}

impl Sub for Slew2 {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut overallscale: f64 = 2.0;
        overallscale /= 44100.0;
        overallscale *= s.sr as f64;
        let mut clamp: f64 = 0.0;
        let mut threshold: f64 = ((1.0 - s.p[0]) as f64).powf(4.0) / overallscale;
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
            s.lata_dry_sample = input_sample_l;
            s.rata_dry_sample = input_sample_r;
            s.lata_halfway_sample = (input_sample_l + s.lata_last1_sample + (-s.lata_last2_sample + s.lata_last3_sample) * s.lata_upsample_high_tweak) / 2.0;
            s.lata_half_dry_sample = s.lata_halfway_sample;
            s.lata_last3_sample = s.lata_last2_sample;
            s.lata_last2_sample = s.lata_last1_sample;
            s.lata_last1_sample = input_sample_l;
            clamp = s.lata_halfway_sample - s.lata_half_dry_sample;
            if clamp > threshold {
                s.lata_halfway_sample = s.last_sample_l + threshold;
            }
            if -clamp > threshold {
                s.lata_halfway_sample = s.last_sample_l - threshold;
            }
            s.last_sample_l = s.lata_halfway_sample;
            s.lata_c = s.lata_halfway_sample - s.lata_half_dry_sample;
            if s.lata_flip {
                s.lata_a *= s.lata_decay;
                s.lata_b *= s.lata_decay;
                s.lata_a += s.lata_c;
                s.lata_b -= s.lata_c;
                s.lata_c = s.lata_a;
            } else {
                s.lata_b *= s.lata_decay;
                s.lata_a *= s.lata_decay;
                s.lata_b += s.lata_c;
                s.lata_a -= s.lata_c;
                s.lata_c = s.lata_b;
            }
            s.lata_half_diff_sample = s.lata_c * s.lata_decay;
            s.lata_flip = !s.lata_flip;
            clamp = input_sample_l - s.last_sample_l;
            if clamp > threshold {
                input_sample_l = s.last_sample_l + threshold;
            }
            if -clamp > threshold {
                input_sample_l = s.last_sample_l - threshold;
            }
            s.last_sample_l = input_sample_l;
            s.lata_c = input_sample_l - s.lata_dry_sample;
            if s.lata_flip {
                s.lata_a *= s.lata_decay;
                s.lata_b *= s.lata_decay;
                s.lata_a += s.lata_c;
                s.lata_b -= s.lata_c;
                s.lata_c = s.lata_a;
            } else {
                s.lata_b *= s.lata_decay;
                s.lata_a *= s.lata_decay;
                s.lata_b += s.lata_c;
                s.lata_a -= s.lata_c;
                s.lata_c = s.lata_b;
            }
            s.lata_diff_sample = s.lata_c * s.lata_decay;
            s.lata_flip = !s.lata_flip;
            input_sample_l = s.lata_dry_sample;
            input_sample_l += (s.lata_diff_sample + s.lata_half_diff_sample + s.lata_prev_diff_sample) / 0.734;
            s.lata_prev_diff_sample = s.lata_diff_sample / 2.0;
            s.rata_halfway_sample = (input_sample_r + s.rata_last1_sample + (-s.rata_last2_sample + s.rata_last3_sample) * s.rata_upsample_high_tweak) / 2.0;
            s.rata_half_dry_sample = s.rata_halfway_sample;
            s.rata_last3_sample = s.rata_last2_sample;
            s.rata_last2_sample = s.rata_last1_sample;
            s.rata_last1_sample = input_sample_r;
            clamp = s.rata_halfway_sample - s.rata_half_dry_sample;
            if clamp > threshold {
                s.rata_halfway_sample = s.last_sample_r + threshold;
            }
            if -clamp > threshold {
                s.rata_halfway_sample = s.last_sample_r - threshold;
            }
            s.last_sample_r = s.rata_halfway_sample;
            s.rata_c = s.rata_halfway_sample - s.rata_half_dry_sample;
            if s.rata_flip {
                s.rata_a *= s.rata_decay;
                s.rata_b *= s.rata_decay;
                s.rata_a += s.rata_c;
                s.rata_b -= s.rata_c;
                s.rata_c = s.rata_a;
            } else {
                s.rata_b *= s.rata_decay;
                s.rata_a *= s.rata_decay;
                s.rata_b += s.rata_c;
                s.rata_a -= s.rata_c;
                s.rata_c = s.rata_b;
            }
            s.rata_half_diff_sample = s.rata_c * s.rata_decay;
            s.rata_flip = !s.rata_flip;
            clamp = input_sample_r - s.last_sample_r;
            if clamp > threshold {
                input_sample_r = s.last_sample_r + threshold;
            }
            if -clamp > threshold {
                input_sample_r = s.last_sample_r - threshold;
            }
            s.last_sample_r = input_sample_r;
            s.rata_c = input_sample_r - s.rata_dry_sample;
            if s.rata_flip {
                s.rata_a *= s.rata_decay;
                s.rata_b *= s.rata_decay;
                s.rata_a += s.rata_c;
                s.rata_b -= s.rata_c;
                s.rata_c = s.rata_a;
            } else {
                s.rata_b *= s.rata_decay;
                s.rata_a *= s.rata_decay;
                s.rata_b += s.rata_c;
                s.rata_a -= s.rata_c;
                s.rata_c = s.rata_b;
            }
            s.rata_diff_sample = s.rata_c * s.rata_decay;
            s.rata_flip = !s.rata_flip;
            input_sample_r = s.rata_dry_sample;
            input_sample_r += (s.rata_diff_sample + s.rata_half_diff_sample + s.rata_prev_diff_sample) / 0.734;
            s.rata_prev_diff_sample = s.rata_diff_sample / 2.0;
            l[smp] = input_sample_l as f32;
            r[smp] = input_sample_r as f32;
        }
    }
}
