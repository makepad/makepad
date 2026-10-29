// Ported from Spiral2 (Spiral2Proc.cpp, Spiral2.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Spiral (sub-effect id 36): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Input", 0.5),
    pf("Highpass", 0.0),
    pf("Presence", 0.5),
    pf("Output", 1.0),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct Spiral2 {
    p: [f32; 5],
    sr: f32,
    iir_sample_al: f64,
    iir_sample_bl: f64,
    prev_sample_l: f64,
    fpd_l: u32,
    fpd_r: u32,
    iir_sample_ar: f64,
    iir_sample_br: f64,
    prev_sample_r: f64,
    flip: bool,
}

impl Spiral2 {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Spiral2> {
        let mut s = Box::new(Spiral2 {
            p: [0.5, 0.0, 0.5, 1.0, 1.0],
            sr: sr as f32,
            iir_sample_al: 0.0,
            iir_sample_bl: 0.0,
            prev_sample_l: 0.0,
            fpd_l: 0,
            fpd_r: 0,
            iir_sample_ar: 0.0,
            iir_sample_br: 0.0,
            prev_sample_r: 0.0,
            flip: false,
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s.flip = true;
        s
    }
}

impl Sub for Spiral2 {
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
        let mut gain: f64 = (s.p[0] as f64 * 2.0).powf(2.0);
        let mut iir_amount: f64 = (s.p[1] as f64).powf(3.0) / overallscale;
        let mut presence: f64 = s.p[2] as f64;
        let mut output: f64 = s.p[3] as f64;
        let mut wet: f64 = s.p[4] as f64;
        for smp in 0..l.len() {
            let mut input_sample_l: f64 = l[smp] as f64;
            let mut input_sample_r: f64 = r[smp] as f64;
            if input_sample_l.abs() < 1.18e-23 {
                input_sample_l = s.fpd_l as f64 * 1.18e-17;
            }
            if input_sample_r.abs() < 1.18e-23 {
                input_sample_r = s.fpd_r as f64 * 1.18e-17;
            }
            let mut dry_sample_l: f64 = input_sample_l;
            let mut dry_sample_r: f64 = input_sample_r;
            if gain != 1.0 {
                input_sample_l *= gain;
                input_sample_r *= gain;
                s.prev_sample_l *= gain;
                s.prev_sample_r *= gain;
            }
            if s.flip {
                s.iir_sample_al = s.iir_sample_al * (1.0 - iir_amount) + input_sample_l * iir_amount;
                s.iir_sample_ar = s.iir_sample_ar * (1.0 - iir_amount) + input_sample_r * iir_amount;
                input_sample_l -= s.iir_sample_al;
                input_sample_r -= s.iir_sample_ar;
            } else {
                s.iir_sample_bl = s.iir_sample_bl * (1.0 - iir_amount) + input_sample_l * iir_amount;
                s.iir_sample_br = s.iir_sample_br * (1.0 - iir_amount) + input_sample_r * iir_amount;
                input_sample_l -= s.iir_sample_bl;
                input_sample_r -= s.iir_sample_br;
            }
            let mut presence_sample_l: f64 = (input_sample_l * s.prev_sample_l.abs()).sin() / (if s.prev_sample_l == 0.0 { 1.0 } else { s.prev_sample_l.abs() });
            let mut presence_sample_r: f64 = (input_sample_r * s.prev_sample_r.abs()).sin() / (if s.prev_sample_r == 0.0 { 1.0 } else { s.prev_sample_r.abs() });
            input_sample_l = (input_sample_l * input_sample_l.abs()).sin() / (if input_sample_l.abs() == 0.0 { 1.0 } else { input_sample_l.abs() });
            input_sample_r = (input_sample_r * input_sample_r.abs()).sin() / (if input_sample_r.abs() == 0.0 { 1.0 } else { input_sample_r.abs() });
            if output < 1.0 {
                input_sample_l *= output;
                input_sample_r *= output;
                presence_sample_l *= output;
                presence_sample_r *= output;
            }
            if presence > 0.0 {
                input_sample_l = input_sample_l * (1.0 - presence) + presence_sample_l * presence;
                input_sample_r = input_sample_r * (1.0 - presence) + presence_sample_r * presence;
            }
            if wet < 1.0 {
                input_sample_l = dry_sample_l * (1.0 - wet) + input_sample_l * wet;
                input_sample_r = dry_sample_r * (1.0 - wet) + input_sample_r * wet;
            }
            s.prev_sample_l = dry_sample_l;
            s.prev_sample_r = dry_sample_r;
            s.flip = !s.flip;
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
