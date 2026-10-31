// Ported from BassDrive (BassDriveProc.cpp, BassDrive.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Bass Drive (sub-effect id 27): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Presnce", 0.5),
    pf("High", 0.5),
    pf("Mid", 0.5),
    pf("Low", 0.5),
    pf("Drive", 0.5),
];

#[allow(dead_code)]
pub struct BassDrive {
    p: [f32; 5],
    sr: f32,
    fpd_l: u32,
    fpd_r: u32,
    presence_in_al: [f64; 7],
    presence_out_al: [f64; 7],
    high_in_al: [f64; 7],
    high_out_al: [f64; 7],
    mid_in_al: [f64; 7],
    mid_out_al: [f64; 7],
    low_in_al: [f64; 7],
    low_out_al: [f64; 7],
    presence_in_bl: [f64; 7],
    presence_out_bl: [f64; 7],
    high_in_bl: [f64; 7],
    high_out_bl: [f64; 7],
    mid_in_bl: [f64; 7],
    mid_out_bl: [f64; 7],
    low_in_bl: [f64; 7],
    low_out_bl: [f64; 7],
    presence_in_ar: [f64; 7],
    presence_out_ar: [f64; 7],
    high_in_ar: [f64; 7],
    high_out_ar: [f64; 7],
    mid_in_ar: [f64; 7],
    mid_out_ar: [f64; 7],
    low_in_ar: [f64; 7],
    low_out_ar: [f64; 7],
    presence_in_br: [f64; 7],
    presence_out_br: [f64; 7],
    high_in_br: [f64; 7],
    high_out_br: [f64; 7],
    mid_in_br: [f64; 7],
    mid_out_br: [f64; 7],
    low_in_br: [f64; 7],
    low_out_br: [f64; 7],
    flip: bool,
}

impl BassDrive {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<BassDrive> {
        let mut s = Box::new(BassDrive {
            p: [0.5, 0.5, 0.5, 0.5, 0.5],
            sr: sr as f32,
            fpd_l: 0,
            fpd_r: 0,
            presence_in_al: [0.0; 7],
            presence_out_al: [0.0; 7],
            high_in_al: [0.0; 7],
            high_out_al: [0.0; 7],
            mid_in_al: [0.0; 7],
            mid_out_al: [0.0; 7],
            low_in_al: [0.0; 7],
            low_out_al: [0.0; 7],
            presence_in_bl: [0.0; 7],
            presence_out_bl: [0.0; 7],
            high_in_bl: [0.0; 7],
            high_out_bl: [0.0; 7],
            mid_in_bl: [0.0; 7],
            mid_out_bl: [0.0; 7],
            low_in_bl: [0.0; 7],
            low_out_bl: [0.0; 7],
            presence_in_ar: [0.0; 7],
            presence_out_ar: [0.0; 7],
            high_in_ar: [0.0; 7],
            high_out_ar: [0.0; 7],
            mid_in_ar: [0.0; 7],
            mid_out_ar: [0.0; 7],
            low_in_ar: [0.0; 7],
            low_out_ar: [0.0; 7],
            presence_in_br: [0.0; 7],
            presence_out_br: [0.0; 7],
            high_in_br: [0.0; 7],
            high_out_br: [0.0; 7],
            mid_in_br: [0.0; 7],
            mid_out_br: [0.0; 7],
            low_in_br: [0.0; 7],
            low_out_br: [0.0; 7],
            flip: false,
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for BassDrive {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut sum_l: f64 = 0.0;
        let mut sum_r: f64 = 0.0;
        let mut presence: f64 = (s.p[0] as f64).powf(5.0) * 8.0;
        let mut high: f64 = (s.p[1] as f64).powf(3.0) * 4.0;
        let mut mid: f64 = (s.p[2] as f64).powf(2.0);
        let mut low: f64 = s.p[3] as f64 / 4.0;
        let mut drive: f64 = s.p[4] as f64 * 2.0;
        let mut bridgerectifier: f64 = 0.0;
        for smp in 0..l.len() {
            let mut input_sample_l: f64 = l[smp] as f64;
            let mut input_sample_r: f64 = r[smp] as f64;
            if input_sample_l.abs() < 1.18e-23 {
                input_sample_l = s.fpd_l as f64 * 1.18e-17;
            }
            if input_sample_r.abs() < 1.18e-23 {
                input_sample_r = s.fpd_r as f64 * 1.18e-17;
            }
            sum_l = 0.0;
            sum_r = 0.0;
            if s.flip {
                s.presence_in_al[0] = s.presence_in_al[1];
                s.presence_in_al[1] = s.presence_in_al[2];
                s.presence_in_al[2] = s.presence_in_al[3];
                s.presence_in_al[3] = s.presence_in_al[4];
                s.presence_in_al[4] = s.presence_in_al[5];
                s.presence_in_al[5] = s.presence_in_al[6];
                s.presence_in_al[6] = input_sample_l * presence;
                s.presence_out_al[2] = s.presence_out_al[3];
                s.presence_out_al[3] = s.presence_out_al[4];
                s.presence_out_al[4] = s.presence_out_al[5];
                s.presence_out_al[5] = s.presence_out_al[6];
                s.presence_out_al[6] = s.presence_in_al[0] + s.presence_in_al[6] + 1.9152966321 * (s.presence_in_al[1] + s.presence_in_al[5]) - (s.presence_in_al[2] + s.presence_in_al[4]) - 3.8305932641 * s.presence_in_al[3] + -0.2828214615 * s.presence_out_al[2] + 0.2613069963 * s.presence_out_al[3] + -0.8628193852 * s.presence_out_al[4] + 0.5387164389 * s.presence_out_al[5];
                bridgerectifier = s.presence_out_al[6].abs();
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                bridgerectifier = bridgerectifier.sin();
                if s.presence_out_al[6] > 0.0 {
                    sum_l += bridgerectifier;
                } else {
                    sum_l -= bridgerectifier;
                }
                s.presence_in_ar[0] = s.presence_in_ar[1];
                s.presence_in_ar[1] = s.presence_in_ar[2];
                s.presence_in_ar[2] = s.presence_in_ar[3];
                s.presence_in_ar[3] = s.presence_in_ar[4];
                s.presence_in_ar[4] = s.presence_in_ar[5];
                s.presence_in_ar[5] = s.presence_in_ar[6];
                s.presence_in_ar[6] = input_sample_r * presence;
                s.presence_out_ar[2] = s.presence_out_ar[3];
                s.presence_out_ar[3] = s.presence_out_ar[4];
                s.presence_out_ar[4] = s.presence_out_ar[5];
                s.presence_out_ar[5] = s.presence_out_ar[6];
                s.presence_out_ar[6] = s.presence_in_ar[0] + s.presence_in_ar[6] + 1.9152966321 * (s.presence_in_ar[1] + s.presence_in_ar[5]) - (s.presence_in_ar[2] + s.presence_in_ar[4]) - 3.8305932641 * s.presence_in_ar[3] + -0.2828214615 * s.presence_out_ar[2] + 0.2613069963 * s.presence_out_ar[3] + -0.8628193852 * s.presence_out_ar[4] + 0.5387164389 * s.presence_out_ar[5];
                bridgerectifier = s.presence_out_ar[6].abs();
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                bridgerectifier = bridgerectifier.sin();
                if s.presence_out_ar[6] > 0.0 {
                    sum_r += bridgerectifier;
                } else {
                    sum_r -= bridgerectifier;
                }
                s.high_in_al[0] = s.high_in_al[1];
                s.high_in_al[1] = s.high_in_al[2];
                s.high_in_al[2] = s.high_in_al[3];
                s.high_in_al[3] = s.high_in_al[4];
                s.high_in_al[4] = s.high_in_al[5];
                s.high_in_al[5] = s.high_in_al[6];
                bridgerectifier = input_sample_l.abs() * high;
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                bridgerectifier = bridgerectifier.sin();
                if input_sample_l > 0.0 {
                    s.high_in_al[6] = bridgerectifier;
                } else {
                    s.high_in_al[6] = -bridgerectifier;
                }
                s.high_in_al[6] *= high;
                s.high_out_al[2] = s.high_out_al[3];
                s.high_out_al[3] = s.high_out_al[4];
                s.high_out_al[4] = s.high_out_al[5];
                s.high_out_al[5] = s.high_out_al[6];
                s.high_out_al[6] = s.high_in_al[0] + s.high_in_al[6] - 0.5141967433 * (s.high_in_al[1] + s.high_in_al[5]) - (s.high_in_al[2] + s.high_in_al[4]) + 1.0283934866 * s.high_in_al[3] + -0.2828214615 * s.high_out_al[2] + 1.0195930909 * s.high_out_al[3] + -1.9633013869 * s.high_out_al[4] + 2.1020162751 * s.high_out_al[5];
                bridgerectifier = s.high_out_al[6].abs();
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                bridgerectifier = bridgerectifier.sin();
                if s.high_out_al[6] > 0.0 {
                    sum_l += bridgerectifier;
                } else {
                    sum_l -= bridgerectifier;
                }
                s.high_in_ar[0] = s.high_in_ar[1];
                s.high_in_ar[1] = s.high_in_ar[2];
                s.high_in_ar[2] = s.high_in_ar[3];
                s.high_in_ar[3] = s.high_in_ar[4];
                s.high_in_ar[4] = s.high_in_ar[5];
                s.high_in_ar[5] = s.high_in_ar[6];
                bridgerectifier = input_sample_r.abs() * high;
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                bridgerectifier = bridgerectifier.sin();
                if input_sample_r > 0.0 {
                    s.high_in_ar[6] = bridgerectifier;
                } else {
                    s.high_in_ar[6] = -bridgerectifier;
                }
                s.high_in_ar[6] *= high;
                s.high_out_ar[2] = s.high_out_ar[3];
                s.high_out_ar[3] = s.high_out_ar[4];
                s.high_out_ar[4] = s.high_out_ar[5];
                s.high_out_ar[5] = s.high_out_ar[6];
                s.high_out_ar[6] = s.high_in_ar[0] + s.high_in_ar[6] - 0.5141967433 * (s.high_in_ar[1] + s.high_in_ar[5]) - (s.high_in_ar[2] + s.high_in_ar[4]) + 1.0283934866 * s.high_in_ar[3] + -0.2828214615 * s.high_out_ar[2] + 1.0195930909 * s.high_out_ar[3] + -1.9633013869 * s.high_out_ar[4] + 2.1020162751 * s.high_out_ar[5];
                bridgerectifier = s.high_out_ar[6].abs();
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                bridgerectifier = bridgerectifier.sin();
                if s.high_out_ar[6] > 0.0 {
                    sum_r += bridgerectifier;
                } else {
                    sum_r -= bridgerectifier;
                }
                s.mid_in_al[0] = s.mid_in_al[1];
                s.mid_in_al[1] = s.mid_in_al[2];
                s.mid_in_al[2] = s.mid_in_al[3];
                s.mid_in_al[3] = s.mid_in_al[4];
                s.mid_in_al[4] = s.mid_in_al[5];
                s.mid_in_al[5] = s.mid_in_al[6];
                bridgerectifier = input_sample_l.abs() * mid;
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                bridgerectifier = bridgerectifier.sin();
                if input_sample_l > 0.0 {
                    s.mid_in_al[6] = bridgerectifier;
                } else {
                    s.mid_in_al[6] = -bridgerectifier;
                }
                s.mid_in_al[6] *= mid;
                s.mid_out_al[2] = s.mid_out_al[3];
                s.mid_out_al[3] = s.mid_out_al[4];
                s.mid_out_al[4] = s.mid_out_al[5];
                s.mid_out_al[5] = s.mid_out_al[6];
                s.mid_out_al[6] = s.mid_in_al[0] + s.mid_in_al[6] - 1.1790257790 * (s.mid_in_al[1] + s.mid_in_al[5]) - (s.mid_in_al[2] + s.mid_in_al[4]) + 2.3580515580 * s.mid_in_al[3] + -0.6292082828 * s.mid_out_al[2] + 2.7785843605 * s.mid_out_al[3] + -4.6638295236 * s.mid_out_al[4] + 3.5142515802 * s.mid_out_al[5];
                sum_l += s.mid_out_al[6];
                s.mid_in_ar[0] = s.mid_in_ar[1];
                s.mid_in_ar[1] = s.mid_in_ar[2];
                s.mid_in_ar[2] = s.mid_in_ar[3];
                s.mid_in_ar[3] = s.mid_in_ar[4];
                s.mid_in_ar[4] = s.mid_in_ar[5];
                s.mid_in_ar[5] = s.mid_in_ar[6];
                bridgerectifier = input_sample_r.abs() * mid;
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                bridgerectifier = bridgerectifier.sin();
                if input_sample_r > 0.0 {
                    s.mid_in_ar[6] = bridgerectifier;
                } else {
                    s.mid_in_ar[6] = -bridgerectifier;
                }
                s.mid_in_ar[6] *= mid;
                s.mid_out_ar[2] = s.mid_out_ar[3];
                s.mid_out_ar[3] = s.mid_out_ar[4];
                s.mid_out_ar[4] = s.mid_out_ar[5];
                s.mid_out_ar[5] = s.mid_out_ar[6];
                s.mid_out_ar[6] = s.mid_in_ar[0] + s.mid_in_ar[6] - 1.1790257790 * (s.mid_in_ar[1] + s.mid_in_ar[5]) - (s.mid_in_ar[2] + s.mid_in_ar[4]) + 2.3580515580 * s.mid_in_ar[3] + -0.6292082828 * s.mid_out_ar[2] + 2.7785843605 * s.mid_out_ar[3] + -4.6638295236 * s.mid_out_ar[4] + 3.5142515802 * s.mid_out_ar[5];
                sum_r += s.mid_out_ar[6];
                s.low_in_al[0] = s.low_in_al[1];
                s.low_in_al[1] = s.low_in_al[2];
                s.low_in_al[2] = s.low_in_al[3];
                s.low_in_al[3] = s.low_in_al[4];
                s.low_in_al[4] = s.low_in_al[5];
                s.low_in_al[5] = s.low_in_al[6];
                bridgerectifier = input_sample_l.abs() * low;
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                bridgerectifier = bridgerectifier.sin();
                if input_sample_l > 0.0 {
                    s.low_in_al[6] = bridgerectifier;
                } else {
                    s.low_in_al[6] = -bridgerectifier;
                }
                s.low_in_al[6] *= low;
                s.low_out_al[2] = s.low_out_al[3];
                s.low_out_al[3] = s.low_out_al[4];
                s.low_out_al[4] = s.low_out_al[5];
                s.low_out_al[5] = s.low_out_al[6];
                s.low_out_al[6] = s.low_in_al[0] + s.low_in_al[6] - 1.9193504547 * (s.low_in_al[1] + s.low_in_al[5]) - (s.low_in_al[2] + s.low_in_al[4]) + 3.8387009093 * s.low_in_al[3] + -0.9195964462 * s.low_out_al[2] + 3.7538173833 * s.low_out_al[3] + -5.7487775603 * s.low_out_al[4] + 3.9145559258 * s.low_out_al[5];
                sum_l += s.low_out_al[6];
                s.low_in_ar[0] = s.low_in_ar[1];
                s.low_in_ar[1] = s.low_in_ar[2];
                s.low_in_ar[2] = s.low_in_ar[3];
                s.low_in_ar[3] = s.low_in_ar[4];
                s.low_in_ar[4] = s.low_in_ar[5];
                s.low_in_ar[5] = s.low_in_ar[6];
                bridgerectifier = input_sample_r.abs() * low;
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                bridgerectifier = bridgerectifier.sin();
                if input_sample_r > 0.0 {
                    s.low_in_ar[6] = bridgerectifier;
                } else {
                    s.low_in_ar[6] = -bridgerectifier;
                }
                s.low_in_ar[6] *= low;
                s.low_out_ar[2] = s.low_out_ar[3];
                s.low_out_ar[3] = s.low_out_ar[4];
                s.low_out_ar[4] = s.low_out_ar[5];
                s.low_out_ar[5] = s.low_out_ar[6];
                s.low_out_ar[6] = s.low_in_ar[0] + s.low_in_ar[6] - 1.9193504547 * (s.low_in_ar[1] + s.low_in_ar[5]) - (s.low_in_ar[2] + s.low_in_ar[4]) + 3.8387009093 * s.low_in_ar[3] + -0.9195964462 * s.low_out_ar[2] + 3.7538173833 * s.low_out_ar[3] + -5.7487775603 * s.low_out_ar[4] + 3.9145559258 * s.low_out_ar[5];
                sum_r += s.low_out_ar[6];
            } else {
                s.presence_in_bl[0] = s.presence_in_bl[1];
                s.presence_in_bl[1] = s.presence_in_bl[2];
                s.presence_in_bl[2] = s.presence_in_bl[3];
                s.presence_in_bl[3] = s.presence_in_bl[4];
                s.presence_in_bl[4] = s.presence_in_bl[5];
                s.presence_in_bl[5] = s.presence_in_bl[6];
                s.presence_in_bl[6] = input_sample_l * presence;
                s.presence_out_bl[2] = s.presence_out_bl[3];
                s.presence_out_bl[3] = s.presence_out_bl[4];
                s.presence_out_bl[4] = s.presence_out_bl[5];
                s.presence_out_bl[5] = s.presence_out_bl[6];
                s.presence_out_bl[6] = s.presence_in_bl[0] + s.presence_in_bl[6] + 1.9152966321 * (s.presence_in_bl[1] + s.presence_in_bl[5]) - (s.presence_in_bl[2] + s.presence_in_bl[4]) - 3.8305932641 * s.presence_in_bl[3] + -0.2828214615 * s.presence_out_bl[2] + 0.2613069963 * s.presence_out_bl[3] + -0.8628193852 * s.presence_out_bl[4] + 0.5387164389 * s.presence_out_bl[5];
                bridgerectifier = s.presence_out_bl[6].abs();
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                bridgerectifier = bridgerectifier.sin();
                if s.presence_out_bl[6] > 0.0 {
                    sum_l += bridgerectifier;
                } else {
                    sum_l -= bridgerectifier;
                }
                s.presence_in_br[0] = s.presence_in_br[1];
                s.presence_in_br[1] = s.presence_in_br[2];
                s.presence_in_br[2] = s.presence_in_br[3];
                s.presence_in_br[3] = s.presence_in_br[4];
                s.presence_in_br[4] = s.presence_in_br[5];
                s.presence_in_br[5] = s.presence_in_br[6];
                s.presence_in_br[6] = input_sample_r * presence;
                s.presence_out_br[2] = s.presence_out_br[3];
                s.presence_out_br[3] = s.presence_out_br[4];
                s.presence_out_br[4] = s.presence_out_br[5];
                s.presence_out_br[5] = s.presence_out_br[6];
                s.presence_out_br[6] = s.presence_in_br[0] + s.presence_in_br[6] + 1.9152966321 * (s.presence_in_br[1] + s.presence_in_br[5]) - (s.presence_in_br[2] + s.presence_in_br[4]) - 3.8305932641 * s.presence_in_br[3] + -0.2828214615 * s.presence_out_br[2] + 0.2613069963 * s.presence_out_br[3] + -0.8628193852 * s.presence_out_br[4] + 0.5387164389 * s.presence_out_br[5];
                bridgerectifier = s.presence_out_br[6].abs();
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                bridgerectifier = bridgerectifier.sin();
                if s.presence_out_br[6] > 0.0 {
                    sum_r += bridgerectifier;
                } else {
                    sum_r -= bridgerectifier;
                }
                s.high_in_bl[0] = s.high_in_bl[1];
                s.high_in_bl[1] = s.high_in_bl[2];
                s.high_in_bl[2] = s.high_in_bl[3];
                s.high_in_bl[3] = s.high_in_bl[4];
                s.high_in_bl[4] = s.high_in_bl[5];
                s.high_in_bl[5] = s.high_in_bl[6];
                bridgerectifier = input_sample_l.abs() * high;
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                bridgerectifier = bridgerectifier.sin();
                if input_sample_l > 0.0 {
                    s.high_in_bl[6] = bridgerectifier;
                } else {
                    s.high_in_bl[6] = -bridgerectifier;
                }
                s.high_in_bl[6] *= high;
                s.high_out_bl[2] = s.high_out_bl[3];
                s.high_out_bl[3] = s.high_out_bl[4];
                s.high_out_bl[4] = s.high_out_bl[5];
                s.high_out_bl[5] = s.high_out_bl[6];
                s.high_out_bl[6] = s.high_in_bl[0] + s.high_in_bl[6] - 0.5141967433 * (s.high_in_bl[1] + s.high_in_bl[5]) - (s.high_in_bl[2] + s.high_in_bl[4]) + 1.0283934866 * s.high_in_bl[3] + -0.2828214615 * s.high_out_bl[2] + 1.0195930909 * s.high_out_bl[3] + -1.9633013869 * s.high_out_bl[4] + 2.1020162751 * s.high_out_bl[5];
                bridgerectifier = s.high_out_bl[6].abs();
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                bridgerectifier = bridgerectifier.sin();
                if s.high_out_bl[6] > 0.0 {
                    sum_l += bridgerectifier;
                } else {
                    sum_l -= bridgerectifier;
                }
                s.high_in_br[0] = s.high_in_br[1];
                s.high_in_br[1] = s.high_in_br[2];
                s.high_in_br[2] = s.high_in_br[3];
                s.high_in_br[3] = s.high_in_br[4];
                s.high_in_br[4] = s.high_in_br[5];
                s.high_in_br[5] = s.high_in_br[6];
                bridgerectifier = input_sample_r.abs() * high;
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                bridgerectifier = bridgerectifier.sin();
                if input_sample_r > 0.0 {
                    s.high_in_br[6] = bridgerectifier;
                } else {
                    s.high_in_br[6] = -bridgerectifier;
                }
                s.high_in_br[6] *= high;
                s.high_out_br[2] = s.high_out_br[3];
                s.high_out_br[3] = s.high_out_br[4];
                s.high_out_br[4] = s.high_out_br[5];
                s.high_out_br[5] = s.high_out_br[6];
                s.high_out_br[6] = s.high_in_br[0] + s.high_in_br[6] - 0.5141967433 * (s.high_in_br[1] + s.high_in_br[5]) - (s.high_in_br[2] + s.high_in_br[4]) + 1.0283934866 * s.high_in_br[3] + -0.2828214615 * s.high_out_br[2] + 1.0195930909 * s.high_out_br[3] + -1.9633013869 * s.high_out_br[4] + 2.1020162751 * s.high_out_br[5];
                bridgerectifier = s.high_out_br[6].abs();
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                bridgerectifier = bridgerectifier.sin();
                if s.high_out_br[6] > 0.0 {
                    sum_r += bridgerectifier;
                } else {
                    sum_r -= bridgerectifier;
                }
                s.mid_in_bl[0] = s.mid_in_bl[1];
                s.mid_in_bl[1] = s.mid_in_bl[2];
                s.mid_in_bl[2] = s.mid_in_bl[3];
                s.mid_in_bl[3] = s.mid_in_bl[4];
                s.mid_in_bl[4] = s.mid_in_bl[5];
                s.mid_in_bl[5] = s.mid_in_bl[6];
                bridgerectifier = input_sample_l.abs() * mid;
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                bridgerectifier = bridgerectifier.sin();
                if input_sample_l > 0.0 {
                    s.mid_in_bl[6] = bridgerectifier;
                } else {
                    s.mid_in_bl[6] = -bridgerectifier;
                }
                s.mid_in_bl[6] *= mid;
                s.mid_out_bl[2] = s.mid_out_bl[3];
                s.mid_out_bl[3] = s.mid_out_bl[4];
                s.mid_out_bl[4] = s.mid_out_bl[5];
                s.mid_out_bl[5] = s.mid_out_bl[6];
                s.mid_out_bl[6] = s.mid_in_bl[0] + s.mid_in_bl[6] - 1.1790257790 * (s.mid_in_bl[1] + s.mid_in_bl[5]) - (s.mid_in_bl[2] + s.mid_in_bl[4]) + 2.3580515580 * s.mid_in_bl[3] + -0.6292082828 * s.mid_out_bl[2] + 2.7785843605 * s.mid_out_bl[3] + -4.6638295236 * s.mid_out_bl[4] + 3.5142515802 * s.mid_out_bl[5];
                sum_l += s.mid_out_bl[6];
                s.mid_in_br[0] = s.mid_in_br[1];
                s.mid_in_br[1] = s.mid_in_br[2];
                s.mid_in_br[2] = s.mid_in_br[3];
                s.mid_in_br[3] = s.mid_in_br[4];
                s.mid_in_br[4] = s.mid_in_br[5];
                s.mid_in_br[5] = s.mid_in_br[6];
                bridgerectifier = input_sample_r.abs() * mid;
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                bridgerectifier = bridgerectifier.sin();
                if input_sample_r > 0.0 {
                    s.mid_in_br[6] = bridgerectifier;
                } else {
                    s.mid_in_br[6] = -bridgerectifier;
                }
                s.mid_in_br[6] *= mid;
                s.mid_out_br[2] = s.mid_out_br[3];
                s.mid_out_br[3] = s.mid_out_br[4];
                s.mid_out_br[4] = s.mid_out_br[5];
                s.mid_out_br[5] = s.mid_out_br[6];
                s.mid_out_br[6] = s.mid_in_br[0] + s.mid_in_br[6] - 1.1790257790 * (s.mid_in_br[1] + s.mid_in_br[5]) - (s.mid_in_br[2] + s.mid_in_br[4]) + 2.3580515580 * s.mid_in_br[3] + -0.6292082828 * s.mid_out_br[2] + 2.7785843605 * s.mid_out_br[3] + -4.6638295236 * s.mid_out_br[4] + 3.5142515802 * s.mid_out_br[5];
                sum_r += s.mid_out_br[6];
                s.low_in_bl[0] = s.low_in_bl[1];
                s.low_in_bl[1] = s.low_in_bl[2];
                s.low_in_bl[2] = s.low_in_bl[3];
                s.low_in_bl[3] = s.low_in_bl[4];
                s.low_in_bl[4] = s.low_in_bl[5];
                s.low_in_bl[5] = s.low_in_bl[6];
                bridgerectifier = input_sample_l.abs() * low;
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                bridgerectifier = bridgerectifier.sin();
                if input_sample_l > 0.0 {
                    s.low_in_bl[6] = bridgerectifier;
                } else {
                    s.low_in_bl[6] = -bridgerectifier;
                }
                s.low_in_bl[6] *= low;
                s.low_out_bl[2] = s.low_out_bl[3];
                s.low_out_bl[3] = s.low_out_bl[4];
                s.low_out_bl[4] = s.low_out_bl[5];
                s.low_out_bl[5] = s.low_out_bl[6];
                s.low_out_bl[6] = s.low_in_bl[0] + s.low_in_bl[6] - 1.9193504547 * (s.low_in_bl[1] + s.low_in_bl[5]) - (s.low_in_bl[2] + s.low_in_bl[4]) + 3.8387009093 * s.low_in_bl[3] + -0.9195964462 * s.low_out_bl[2] + 3.7538173833 * s.low_out_bl[3] + -5.7487775603 * s.low_out_bl[4] + 3.9145559258 * s.low_out_bl[5];
                sum_l += s.low_out_bl[6];
                s.low_in_br[0] = s.low_in_br[1];
                s.low_in_br[1] = s.low_in_br[2];
                s.low_in_br[2] = s.low_in_br[3];
                s.low_in_br[3] = s.low_in_br[4];
                s.low_in_br[4] = s.low_in_br[5];
                s.low_in_br[5] = s.low_in_br[6];
                bridgerectifier = input_sample_r.abs() * low;
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                bridgerectifier = bridgerectifier.sin();
                if input_sample_r > 0.0 {
                    s.low_in_br[6] = bridgerectifier;
                } else {
                    s.low_in_br[6] = -bridgerectifier;
                }
                s.low_in_br[6] *= low;
                s.low_out_br[2] = s.low_out_br[3];
                s.low_out_br[3] = s.low_out_br[4];
                s.low_out_br[4] = s.low_out_br[5];
                s.low_out_br[5] = s.low_out_br[6];
                s.low_out_br[6] = s.low_in_br[0] + s.low_in_br[6] - 1.9193504547 * (s.low_in_br[1] + s.low_in_br[5]) - (s.low_in_br[2] + s.low_in_br[4]) + 3.8387009093 * s.low_in_br[3] + -0.9195964462 * s.low_out_br[2] + 3.7538173833 * s.low_out_br[3] + -5.7487775603 * s.low_out_br[4] + 3.9145559258 * s.low_out_br[5];
                sum_r += s.low_out_br[6];
            }
            input_sample_l = sum_l.abs() * drive;
            if input_sample_l > 1.57079633 {
                input_sample_l = 1.57079633;
            }
            input_sample_l = input_sample_l.sin();
            if sum_l < 0.0 {
                input_sample_l = -input_sample_l;
            }
            input_sample_r = sum_r.abs() * drive;
            if input_sample_r > 1.57079633 {
                input_sample_r = 1.57079633;
            }
            input_sample_r = input_sample_r.sin();
            if sum_r < 0.0 {
                input_sample_r = -input_sample_r;
            }
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
