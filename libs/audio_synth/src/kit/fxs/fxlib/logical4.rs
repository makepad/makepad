// Ported from Logical4 (Logical4Proc.cpp, Logical4.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Logical (sub-effect id 4): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Threshold", 0.5),
    pf("Ratio", 0.2),
    pf("Speed", 0.192020202020202),
    pf("MakeupGn", 0.5),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct Logical4 {
    p: [f32; 5],
    sr: f32,
    control_apos_l: f64,
    control_aneg_l: f64,
    control_bpos_l: f64,
    control_bneg_l: f64,
    targetpos_l: f64,
    targetneg_l: f64,
    control_apos_bl: f64,
    control_aneg_bl: f64,
    control_bpos_bl: f64,
    control_bneg_bl: f64,
    targetpos_bl: f64,
    targetneg_bl: f64,
    control_apos_cl: f64,
    control_aneg_cl: f64,
    control_bpos_cl: f64,
    control_bneg_cl: f64,
    targetpos_cl: f64,
    targetneg_cl: f64,
    avg_al: f64,
    avg_bl: f64,
    avg_cl: f64,
    avg_dl: f64,
    avg_el: f64,
    avg_fl: f64,
    nvg_al: f64,
    nvg_bl: f64,
    nvg_cl: f64,
    nvg_dl: f64,
    nvg_el: f64,
    nvg_fl: f64,
    d_l: Vec<f64>,
    control_l: f64,
    b_l: Vec<f64>,
    control_bl: f64,
    c_l: Vec<f64>,
    control_cl: f64,
    control_apos_r: f64,
    control_aneg_r: f64,
    control_bpos_r: f64,
    control_bneg_r: f64,
    targetpos_r: f64,
    targetneg_r: f64,
    control_apos_br: f64,
    control_aneg_br: f64,
    control_bpos_br: f64,
    control_bneg_br: f64,
    targetpos_br: f64,
    targetneg_br: f64,
    control_apos_cr: f64,
    control_aneg_cr: f64,
    control_bpos_cr: f64,
    control_bneg_cr: f64,
    targetpos_cr: f64,
    targetneg_cr: f64,
    avg_ar: f64,
    avg_br: f64,
    avg_cr: f64,
    avg_dr: f64,
    avg_er: f64,
    avg_fr: f64,
    nvg_ar: f64,
    nvg_br: f64,
    nvg_cr: f64,
    nvg_dr: f64,
    nvg_er: f64,
    nvg_fr: f64,
    d_r: Vec<f64>,
    control_r: f64,
    b_r: Vec<f64>,
    control_br: f64,
    c_r: Vec<f64>,
    control_cr: f64,
    gcount: i32,
    fp_flip: bool,
    fpd_l: u32,
    fpd_r: u32,
}

impl Logical4 {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Logical4> {
        let mut s = Box::new(Logical4 {
            p: [0.5, 0.2, 0.192020202020202, 0.5, 1.0],
            sr: sr as f32,
            control_apos_l: 0.0,
            control_aneg_l: 0.0,
            control_bpos_l: 0.0,
            control_bneg_l: 0.0,
            targetpos_l: 0.0,
            targetneg_l: 0.0,
            control_apos_bl: 0.0,
            control_aneg_bl: 0.0,
            control_bpos_bl: 0.0,
            control_bneg_bl: 0.0,
            targetpos_bl: 0.0,
            targetneg_bl: 0.0,
            control_apos_cl: 0.0,
            control_aneg_cl: 0.0,
            control_bpos_cl: 0.0,
            control_bneg_cl: 0.0,
            targetpos_cl: 0.0,
            targetneg_cl: 0.0,
            avg_al: 0.0,
            avg_bl: 0.0,
            avg_cl: 0.0,
            avg_dl: 0.0,
            avg_el: 0.0,
            avg_fl: 0.0,
            nvg_al: 0.0,
            nvg_bl: 0.0,
            nvg_cl: 0.0,
            nvg_dl: 0.0,
            nvg_el: 0.0,
            nvg_fl: 0.0,
            d_l: vec![0.0; 1000],
            control_l: 0.0,
            b_l: vec![0.0; 1000],
            control_bl: 0.0,
            c_l: vec![0.0; 1000],
            control_cl: 0.0,
            control_apos_r: 0.0,
            control_aneg_r: 0.0,
            control_bpos_r: 0.0,
            control_bneg_r: 0.0,
            targetpos_r: 0.0,
            targetneg_r: 0.0,
            control_apos_br: 0.0,
            control_aneg_br: 0.0,
            control_bpos_br: 0.0,
            control_bneg_br: 0.0,
            targetpos_br: 0.0,
            targetneg_br: 0.0,
            control_apos_cr: 0.0,
            control_aneg_cr: 0.0,
            control_bpos_cr: 0.0,
            control_bneg_cr: 0.0,
            targetpos_cr: 0.0,
            targetneg_cr: 0.0,
            avg_ar: 0.0,
            avg_br: 0.0,
            avg_cr: 0.0,
            avg_dr: 0.0,
            avg_er: 0.0,
            avg_fr: 0.0,
            nvg_ar: 0.0,
            nvg_br: 0.0,
            nvg_cr: 0.0,
            nvg_dr: 0.0,
            nvg_er: 0.0,
            nvg_fr: 0.0,
            d_r: vec![0.0; 1000],
            control_r: 0.0,
            b_r: vec![0.0; 1000],
            control_br: 0.0,
            c_r: vec![0.0; 1000],
            control_cr: 0.0,
            gcount: 0,
            fp_flip: false,
            fpd_l: 0,
            fpd_r: 0,
        });
        s.control_apos_l = 1.0;
        s.control_aneg_l = 1.0;
        s.control_bpos_l = 1.0;
        s.control_bneg_l = 1.0;
        s.targetpos_l = 1.0;
        s.targetneg_l = 1.0;
        s.control_apos_bl = 1.0;
        s.control_aneg_bl = 1.0;
        s.control_bpos_bl = 1.0;
        s.control_bneg_bl = 1.0;
        s.targetpos_bl = 1.0;
        s.targetneg_bl = 1.0;
        s.control_apos_cl = 1.0;
        s.control_aneg_cl = 1.0;
        s.control_bpos_cl = 1.0;
        s.control_bneg_cl = 1.0;
        s.targetpos_cl = 1.0;
        s.targetneg_cl = 1.0;
        s.control_apos_r = 1.0;
        s.control_aneg_r = 1.0;
        s.control_bpos_r = 1.0;
        s.control_bneg_r = 1.0;
        s.targetpos_r = 1.0;
        s.targetneg_r = 1.0;
        s.control_apos_br = 1.0;
        s.control_aneg_br = 1.0;
        s.control_bpos_br = 1.0;
        s.control_bneg_br = 1.0;
        s.targetpos_br = 1.0;
        s.targetneg_br = 1.0;
        s.control_apos_cr = 1.0;
        s.control_aneg_cr = 1.0;
        s.control_bpos_cr = 1.0;
        s.control_bneg_cr = 1.0;
        s.targetpos_cr = 1.0;
        s.targetneg_cr = 1.0;
        s.fp_flip = true;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for Logical4 {
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
        let mut dry_sample_l: f32 = 0.0;
        let mut dry_sample_r: f32 = 0.0;
        let mut input_sample_l: f64 = 0.0;
        let mut input_sample_r: f64 = 0.0;
        let mut inputpos: f64 = 0.0;
        let mut inputneg: f64 = 0.0;
        let mut calcpos: f64 = 0.0;
        let mut calcneg: f64 = 0.0;
        let mut outputpos: f64 = 0.0;
        let mut outputneg: f64 = 0.0;
        let mut totalmultiplier: f64 = 0.0;
        let mut inputgain: f64 = 10.0_f64.powf(-(s.p[0] as f64 * 40.0 - 20.0) / 20.0);
        let mut compoutgain: f64 = inputgain;
        let mut attackspeed: f64 = (s.p[2] * s.p[2]) as f64 * 99.0 + 1.0;
        attackspeed = 10.0 / attackspeed.sqrt();
        let mut divisor: f64 = 0.000782 * attackspeed;
        divisor /= overallscale;
        let mut remainder: f64 = divisor;
        divisor = 1.0 - divisor;
        let mut divisor_b: f64 = 0.000819 * attackspeed;
        divisor_b /= overallscale;
        let mut remainder_b: f64 = divisor_b;
        divisor_b = 1.0 - divisor_b;
        let mut divisor_c: f64 = 0.000857;
        divisor_c /= overallscale;
        let mut remainder_c: f64 = divisor_c * attackspeed;
        divisor_c = 1.0 - divisor_c;
        let mut dynamic_divisor: f64 = 0.0;
        let mut dynamic_remainder: f64 = 0.0;
        let mut intensity: f64 = 0.0445556;
        let mut depth_a: f64 = 2.42;
        let mut offset_a: i32 = (depth_a * overallscale) as i32;
        if offset_a < 1 {
            offset_a = 1;
        }
        if offset_a > 498 {
            offset_a = 498;
        }
        let mut depth_b: f64 = 2.42;
        let mut offset_b: i32 = (depth_b * overallscale) as i32;
        if offset_b < 1 {
            offset_b = 1;
        }
        if offset_b > 498 {
            offset_b = 498;
        }
        let mut depth_c: f64 = 2.42;
        let mut offset_c: i32 = (depth_c * overallscale) as i32;
        if offset_c < 1 {
            offset_c = 1;
        }
        if offset_c > 498 {
            offset_c = 498;
        }
        let mut clamp: f64 = 0.0;
        let mut thickness: f64 = 0.0;
        let mut out: f64 = 0.0;
        let mut bridgerectifier: f64 = 0.0;
        let mut power_sag: f64 = 0.003300223685324102874217;
        let mut ratio: f64 = ((s.p[1] * s.p[1]) as f64 * 15.0 + 1.0).sqrt() - 1.0;
        if ratio > 2.99999 {
            ratio = 2.99999;
        }
        if ratio < 0.0 {
            ratio = 0.0;
        }
        let mut ratioselector: i32 = ratio.floor() as i32;
        ratio -= ratioselector as f64;
        let mut inv_ratio: f64 = 1.0 - ratio;
        let mut out_sample_al: f64 = 0.0;
        let mut out_sample_bl: f64 = 0.0;
        let mut out_sample_cl: f64 = 0.0;
        let mut out_sample_ar: f64 = 0.0;
        let mut out_sample_br: f64 = 0.0;
        let mut out_sample_cr: f64 = 0.0;
        let mut outputgain: f64 = 10.0_f64.powf((s.p[3] as f64 * 40.0 - 20.0) / 20.0);
        let mut wet: f64 = s.p[4] as f64;
        for smp in 0..l.len() {
            input_sample_l = l[smp] as f64;
            input_sample_r = r[smp] as f64;
            if input_sample_l.abs() < 1.18e-23 {
                input_sample_l = s.fpd_l as f64 * 1.18e-17;
            }
            if input_sample_r.abs() < 1.18e-23 {
                input_sample_r = s.fpd_r as f64 * 1.18e-17;
            }
            dry_sample_l = input_sample_l as f32;
            dry_sample_r = input_sample_r as f32;
            s.gcount -= 1;
            if s.gcount < 0 || s.gcount > 499 {
                s.gcount = 499;
            }
            s.d_l[s.gcount as usize] = input_sample_l.abs() * (intensity - (s.control_apos_l + s.control_bpos_l + s.control_aneg_l + s.control_bneg_l) * power_sag);
            s.d_l[(s.gcount + 499) as usize] = s.d_l[s.gcount as usize];
            s.control_l += s.d_l[s.gcount as usize] / offset_a as f64;
            s.control_l -= s.d_l[(s.gcount + offset_a) as usize] / offset_a as f64;
            s.control_l -= 0.000001;
            clamp = 1.0;
            if s.control_l < 0.0 {
                s.control_l = 0.0;
            }
            if s.control_l > 1.0 {
                clamp -= s.control_l - 1.0;
                s.control_l = 1.0;
            }
            if clamp < 0.5 {
                clamp = 0.5;
            }
            thickness = (1.0 - s.control_l) * 2.0 - 1.0;
            out = thickness.abs();
            bridgerectifier = input_sample_l.abs();
            if bridgerectifier > 1.57079633 {
                bridgerectifier = 1.57079633;
            }
            if thickness > 0.0 {
                bridgerectifier = bridgerectifier.sin();
            } else {
                bridgerectifier = 1.0 - bridgerectifier.cos();
            }
            if input_sample_l > 0.0 {
                input_sample_l = input_sample_l * (1.0 - out) + bridgerectifier * out;
            } else {
                input_sample_l = input_sample_l * (1.0 - out) - bridgerectifier * out;
            }
            if clamp != 1.0 {
                input_sample_l *= clamp;
            }
            s.d_r[s.gcount as usize] = input_sample_r.abs() * (intensity - (s.control_apos_r + s.control_bpos_r + s.control_aneg_r + s.control_bneg_r) * power_sag);
            s.d_r[(s.gcount + 499) as usize] = s.d_r[s.gcount as usize];
            s.control_r += s.d_r[s.gcount as usize] / offset_a as f64;
            s.control_r -= s.d_r[(s.gcount + offset_a) as usize] / offset_a as f64;
            s.control_r -= 0.000001;
            clamp = 1.0;
            if s.control_r < 0.0 {
                s.control_r = 0.0;
            }
            if s.control_r > 1.0 {
                clamp -= s.control_r - 1.0;
                s.control_r = 1.0;
            }
            if clamp < 0.5 {
                clamp = 0.5;
            }
            thickness = (1.0 - s.control_r) * 2.0 - 1.0;
            out = thickness.abs();
            bridgerectifier = input_sample_r.abs();
            if bridgerectifier > 1.57079633 {
                bridgerectifier = 1.57079633;
            }
            if thickness > 0.0 {
                bridgerectifier = bridgerectifier.sin();
            } else {
                bridgerectifier = 1.0 - bridgerectifier.cos();
            }
            if input_sample_r > 0.0 {
                input_sample_r = input_sample_r * (1.0 - out) + bridgerectifier * out;
            } else {
                input_sample_r = input_sample_r * (1.0 - out) - bridgerectifier * out;
            }
            if clamp != 1.0 {
                input_sample_r *= clamp;
            }
            if inputgain != 1.0 {
                input_sample_l *= inputgain;
            }
            inputpos = input_sample_l * fp_old + s.avg_al * fp_new + 1.0;
            s.avg_al = input_sample_l;
            if inputpos < 0.001 {
                inputpos = 0.001;
            }
            outputpos = inputpos / 2.0;
            if outputpos > 1.0 {
                outputpos = 1.0;
            }
            inputpos *= inputpos;
            dynamic_remainder = remainder * (inputpos + 1.0);
            if dynamic_remainder > 1.0 {
                dynamic_remainder = 1.0;
            }
            dynamic_divisor = 1.0 - dynamic_remainder;
            s.targetpos_l *= dynamic_divisor;
            s.targetpos_l += inputpos * dynamic_remainder;
            calcpos = (1.0 / s.targetpos_l).powf(2.0);
            inputneg = -input_sample_l * fp_old + s.nvg_al * fp_new + 1.0;
            s.nvg_al = -input_sample_l;
            if inputneg < 0.001 {
                inputneg = 0.001;
            }
            outputneg = inputneg / 2.0;
            if outputneg > 1.0 {
                outputneg = 1.0;
            }
            inputneg *= inputneg;
            dynamic_remainder = remainder * (inputneg + 1.0);
            if dynamic_remainder > 1.0 {
                dynamic_remainder = 1.0;
            }
            dynamic_divisor = 1.0 - dynamic_remainder;
            s.targetneg_l *= dynamic_divisor;
            s.targetneg_l += inputneg * dynamic_remainder;
            calcneg = (1.0 / s.targetneg_l).powf(2.0);
            if input_sample_l > 0.0 {
                if true as i32 == s.fp_flip as i32 {
                    s.control_apos_l *= divisor;
                    s.control_apos_l += calcpos * remainder;
                    if s.control_apos_r > s.control_apos_l {
                        s.control_apos_r = (s.control_apos_r + s.control_apos_l) * 0.5;
                    }
                } else {
                    s.control_bpos_l *= divisor;
                    s.control_bpos_l += calcpos * remainder;
                    if s.control_bpos_r > s.control_bpos_l {
                        s.control_bpos_r = (s.control_bpos_r + s.control_bpos_l) * 0.5;
                    }
                }
            } else {
                if true as i32 == s.fp_flip as i32 {
                    s.control_aneg_l *= divisor;
                    s.control_aneg_l += calcneg * remainder;
                    if s.control_aneg_r > s.control_aneg_l {
                        s.control_aneg_r = (s.control_aneg_r + s.control_aneg_l) * 0.5;
                    }
                } else {
                    s.control_bneg_l *= divisor;
                    s.control_bneg_l += calcneg * remainder;
                    if s.control_bneg_r > s.control_bneg_l {
                        s.control_bneg_r = (s.control_bneg_r + s.control_bneg_l) * 0.5;
                    }
                }
            }
            if true as i32 == s.fp_flip as i32 {
                totalmultiplier = s.control_apos_l * outputpos + s.control_aneg_l * outputneg;
            } else {
                totalmultiplier = s.control_bpos_l * outputpos + s.control_bneg_l * outputneg;
            }
            if totalmultiplier != 1.0 {
                input_sample_l *= totalmultiplier;
            }
            if input_sample_l > 36.0 {
                input_sample_l = 36.0;
            }
            if input_sample_l < -36.0 {
                input_sample_l = -36.0;
            }
            out_sample_al = input_sample_l / compoutgain;
            if inputgain != 1.0 {
                input_sample_r *= inputgain;
            }
            inputpos = input_sample_r * fp_old + s.avg_ar * fp_new + 1.0;
            s.avg_ar = input_sample_r;
            if inputpos < 0.001 {
                inputpos = 0.001;
            }
            outputpos = inputpos / 2.0;
            if outputpos > 1.0 {
                outputpos = 1.0;
            }
            inputpos *= inputpos;
            dynamic_remainder = remainder * (inputpos + 1.0);
            if dynamic_remainder > 1.0 {
                dynamic_remainder = 1.0;
            }
            dynamic_divisor = 1.0 - dynamic_remainder;
            s.targetpos_r *= dynamic_divisor;
            s.targetpos_r += inputpos * dynamic_remainder;
            calcpos = (1.0 / s.targetpos_r).powf(2.0);
            inputneg = -input_sample_r * fp_old + s.nvg_ar * fp_new + 1.0;
            s.nvg_ar = -input_sample_r;
            if inputneg < 0.001 {
                inputneg = 0.001;
            }
            outputneg = inputneg / 2.0;
            if outputneg > 1.0 {
                outputneg = 1.0;
            }
            inputneg *= inputneg;
            dynamic_remainder = remainder * (inputneg + 1.0);
            if dynamic_remainder > 1.0 {
                dynamic_remainder = 1.0;
            }
            dynamic_divisor = 1.0 - dynamic_remainder;
            s.targetneg_r *= dynamic_divisor;
            s.targetneg_r += inputneg * dynamic_remainder;
            calcneg = (1.0 / s.targetneg_r).powf(2.0);
            if input_sample_r > 0.0 {
                if true as i32 == s.fp_flip as i32 {
                    s.control_apos_r *= divisor;
                    s.control_apos_r += calcpos * remainder;
                    if s.control_apos_l > s.control_apos_r {
                        s.control_apos_l = (s.control_apos_r + s.control_apos_l) * 0.5;
                    }
                } else {
                    s.control_bpos_r *= divisor;
                    s.control_bpos_r += calcpos * remainder;
                    if s.control_bpos_l > s.control_bpos_r {
                        s.control_bpos_l = (s.control_bpos_r + s.control_bpos_l) * 0.5;
                    }
                }
            } else {
                if true as i32 == s.fp_flip as i32 {
                    s.control_aneg_r *= divisor;
                    s.control_aneg_r += calcneg * remainder;
                    if s.control_aneg_l > s.control_aneg_r {
                        s.control_aneg_l = (s.control_aneg_r + s.control_aneg_l) * 0.5;
                    }
                } else {
                    s.control_bneg_r *= divisor;
                    s.control_bneg_r += calcneg * remainder;
                    if s.control_bneg_l > s.control_bneg_r {
                        s.control_bneg_l = (s.control_bneg_r + s.control_bneg_l) * 0.5;
                    }
                }
            }
            if true as i32 == s.fp_flip as i32 {
                totalmultiplier = s.control_apos_r * outputpos + s.control_aneg_r * outputneg;
            } else {
                totalmultiplier = s.control_bpos_r * outputpos + s.control_bneg_r * outputneg;
            }
            if totalmultiplier != 1.0 {
                input_sample_r *= totalmultiplier;
            }
            if input_sample_r > 36.0 {
                input_sample_r = 36.0;
            }
            if input_sample_r < -36.0 {
                input_sample_r = -36.0;
            }
            out_sample_ar = input_sample_r / compoutgain;
            if ratioselector > 0 {
                s.b_l[s.gcount as usize] = input_sample_l.abs() * (intensity - (s.control_apos_bl + s.control_bpos_bl + s.control_aneg_bl + s.control_bneg_bl) * power_sag);
                s.b_l[(s.gcount + 499) as usize] = s.b_l[s.gcount as usize];
                s.control_bl += s.b_l[s.gcount as usize] / offset_b as f64;
                s.control_bl -= s.b_l[(s.gcount + offset_b) as usize] / offset_b as f64;
                s.control_bl -= 0.000001;
                clamp = 1.0;
                if s.control_bl < 0.0 {
                    s.control_bl = 0.0;
                }
                if s.control_bl > 1.0 {
                    clamp -= s.control_bl - 1.0;
                    s.control_bl = 1.0;
                }
                if clamp < 0.5 {
                    clamp = 0.5;
                }
                thickness = (1.0 - s.control_bl) * 2.0 - 1.0;
                out = thickness.abs();
                bridgerectifier = input_sample_l.abs();
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                if thickness > 0.0 {
                    bridgerectifier = bridgerectifier.sin();
                } else {
                    bridgerectifier = 1.0 - bridgerectifier.cos();
                }
                if input_sample_l > 0.0 {
                    input_sample_l = input_sample_l * (1.0 - out) + bridgerectifier * out;
                } else {
                    input_sample_l = input_sample_l * (1.0 - out) - bridgerectifier * out;
                }
                if clamp != 1.0 {
                    input_sample_l *= clamp;
                }
                s.b_r[s.gcount as usize] = input_sample_r.abs() * (intensity - (s.control_apos_br + s.control_bpos_br + s.control_aneg_br + s.control_bneg_br) * power_sag);
                s.b_r[(s.gcount + 499) as usize] = s.b_r[s.gcount as usize];
                s.control_br += s.b_r[s.gcount as usize] / offset_b as f64;
                s.control_br -= s.b_r[(s.gcount + offset_b) as usize] / offset_b as f64;
                s.control_br -= 0.000001;
                clamp = 1.0;
                if s.control_br < 0.0 {
                    s.control_br = 0.0;
                }
                if s.control_br > 1.0 {
                    clamp -= s.control_br - 1.0;
                    s.control_br = 1.0;
                }
                if clamp < 0.5 {
                    clamp = 0.5;
                }
                thickness = (1.0 - s.control_br) * 2.0 - 1.0;
                out = thickness.abs();
                bridgerectifier = input_sample_r.abs();
                if bridgerectifier > 1.57079633 {
                    bridgerectifier = 1.57079633;
                }
                if thickness > 0.0 {
                    bridgerectifier = bridgerectifier.sin();
                } else {
                    bridgerectifier = 1.0 - bridgerectifier.cos();
                }
                if input_sample_r > 0.0 {
                    input_sample_r = input_sample_r * (1.0 - out) + bridgerectifier * out;
                } else {
                    input_sample_r = input_sample_r * (1.0 - out) - bridgerectifier * out;
                }
                if clamp != 1.0 {
                    input_sample_r *= clamp;
                }
                inputpos = input_sample_l * fp_old + s.avg_bl * fp_new + 1.0;
                s.avg_bl = input_sample_l;
                if inputpos < 0.001 {
                    inputpos = 0.001;
                }
                outputpos = inputpos / 2.0;
                if outputpos > 1.0 {
                    outputpos = 1.0;
                }
                inputpos *= inputpos;
                dynamic_remainder = remainder_b * (inputpos + 1.0);
                if dynamic_remainder > 1.0 {
                    dynamic_remainder = 1.0;
                }
                dynamic_divisor = 1.0 - dynamic_remainder;
                s.targetpos_bl *= dynamic_divisor;
                s.targetpos_bl += inputpos * dynamic_remainder;
                calcpos = (1.0 / s.targetpos_bl).powf(2.0);
                inputneg = -input_sample_l * fp_old + s.nvg_bl * fp_new + 1.0;
                s.nvg_bl = -input_sample_l;
                if inputneg < 0.001 {
                    inputneg = 0.001;
                }
                outputneg = inputneg / 2.0;
                if outputneg > 1.0 {
                    outputneg = 1.0;
                }
                inputneg *= inputneg;
                dynamic_remainder = remainder_b * (inputneg + 1.0);
                if dynamic_remainder > 1.0 {
                    dynamic_remainder = 1.0;
                }
                dynamic_divisor = 1.0 - dynamic_remainder;
                s.targetneg_bl *= dynamic_divisor;
                s.targetneg_bl += inputneg * dynamic_remainder;
                calcneg = (1.0 / s.targetneg_bl).powf(2.0);
                if input_sample_l > 0.0 {
                    if true as i32 == s.fp_flip as i32 {
                        s.control_apos_bl *= divisor_b;
                        s.control_apos_bl += calcpos * remainder_b;
                        if s.control_apos_br > s.control_apos_bl {
                            s.control_apos_br = (s.control_apos_br + s.control_apos_bl) * 0.5;
                        }
                    } else {
                        s.control_bpos_bl *= divisor_b;
                        s.control_bpos_bl += calcpos * remainder_b;
                        if s.control_bpos_br > s.control_bpos_bl {
                            s.control_bpos_br = (s.control_bpos_br + s.control_bpos_bl) * 0.5;
                        }
                    }
                } else {
                    if true as i32 == s.fp_flip as i32 {
                        s.control_aneg_bl *= divisor_b;
                        s.control_aneg_bl += calcneg * remainder_b;
                        if s.control_aneg_br > s.control_aneg_bl {
                            s.control_aneg_br = (s.control_aneg_br + s.control_aneg_bl) * 0.5;
                        }
                    } else {
                        s.control_bneg_bl *= divisor_b;
                        s.control_bneg_bl += calcneg * remainder_b;
                        if s.control_bneg_br > s.control_bneg_bl {
                            s.control_bneg_br = (s.control_bneg_br + s.control_bneg_bl) * 0.5;
                        }
                    }
                }
                if true as i32 == s.fp_flip as i32 {
                    totalmultiplier = s.control_apos_bl * outputpos + s.control_aneg_bl * outputneg;
                } else {
                    totalmultiplier = s.control_bpos_bl * outputpos + s.control_bneg_bl * outputneg;
                }
                if totalmultiplier != 1.0 {
                    input_sample_l *= totalmultiplier;
                }
                if input_sample_l > 36.0 {
                    input_sample_l = 36.0;
                }
                if input_sample_l < -36.0 {
                    input_sample_l = -36.0;
                }
                out_sample_bl = input_sample_l / compoutgain;
                inputpos = input_sample_r * fp_old + s.avg_br * fp_new + 1.0;
                s.avg_br = input_sample_r;
                if inputpos < 0.001 {
                    inputpos = 0.001;
                }
                outputpos = inputpos / 2.0;
                if outputpos > 1.0 {
                    outputpos = 1.0;
                }
                inputpos *= inputpos;
                dynamic_remainder = remainder_b * (inputpos + 1.0);
                if dynamic_remainder > 1.0 {
                    dynamic_remainder = 1.0;
                }
                dynamic_divisor = 1.0 - dynamic_remainder;
                s.targetpos_br *= dynamic_divisor;
                s.targetpos_br += inputpos * dynamic_remainder;
                calcpos = (1.0 / s.targetpos_br).powf(2.0);
                inputneg = -input_sample_r * fp_old + s.nvg_br * fp_new + 1.0;
                s.nvg_br = -input_sample_r;
                if inputneg < 0.001 {
                    inputneg = 0.001;
                }
                outputneg = inputneg / 2.0;
                if outputneg > 1.0 {
                    outputneg = 1.0;
                }
                inputneg *= inputneg;
                dynamic_remainder = remainder_b * (inputneg + 1.0);
                if dynamic_remainder > 1.0 {
                    dynamic_remainder = 1.0;
                }
                dynamic_divisor = 1.0 - dynamic_remainder;
                s.targetneg_br *= dynamic_divisor;
                s.targetneg_br += inputneg * dynamic_remainder;
                calcneg = (1.0 / s.targetneg_br).powf(2.0);
                if input_sample_r > 0.0 {
                    if true as i32 == s.fp_flip as i32 {
                        s.control_apos_br *= divisor_b;
                        s.control_apos_br += calcpos * remainder_b;
                        if s.control_apos_bl > s.control_apos_br {
                            s.control_apos_bl = (s.control_apos_br + s.control_apos_bl) * 0.5;
                        }
                    } else {
                        s.control_bpos_br *= divisor_b;
                        s.control_bpos_br += calcpos * remainder_b;
                        if s.control_bpos_bl > s.control_bpos_br {
                            s.control_bpos_bl = (s.control_bpos_br + s.control_bpos_bl) * 0.5;
                        }
                    }
                } else {
                    if true as i32 == s.fp_flip as i32 {
                        s.control_aneg_br *= divisor_b;
                        s.control_aneg_br += calcneg * remainder_b;
                        if s.control_aneg_bl > s.control_aneg_br {
                            s.control_aneg_bl = (s.control_aneg_br + s.control_aneg_bl) * 0.5;
                        }
                    } else {
                        s.control_bneg_br *= divisor_b;
                        s.control_bneg_br += calcneg * remainder_b;
                        if s.control_bneg_bl > s.control_bneg_br {
                            s.control_bneg_bl = (s.control_bneg_br + s.control_bneg_bl) * 0.5;
                        }
                    }
                }
                if true as i32 == s.fp_flip as i32 {
                    totalmultiplier = s.control_apos_br * outputpos + s.control_aneg_br * outputneg;
                } else {
                    totalmultiplier = s.control_bpos_br * outputpos + s.control_bneg_br * outputneg;
                }
                if totalmultiplier != 1.0 {
                    input_sample_r *= totalmultiplier;
                }
                if input_sample_r > 36.0 {
                    input_sample_r = 36.0;
                }
                if input_sample_r < -36.0 {
                    input_sample_r = -36.0;
                }
                out_sample_br = input_sample_r / compoutgain;
                if ratioselector > 1 {
                    s.c_l[s.gcount as usize] = input_sample_l.abs() * (intensity - (s.control_apos_cl + s.control_bpos_cl + s.control_aneg_cl + s.control_bneg_cl) * power_sag);
                    s.c_l[(s.gcount + 499) as usize] = s.c_l[s.gcount as usize];
                    s.control_cl += s.c_l[s.gcount as usize] / offset_c as f64;
                    s.control_cl -= s.c_l[(s.gcount + offset_b) as usize] / offset_c as f64;
                    s.control_cl -= 0.000001;
                    clamp = 1.0;
                    if s.control_cl < 0.0 {
                        s.control_cl = 0.0;
                    }
                    if s.control_cl > 1.0 {
                        clamp -= s.control_cl - 1.0;
                        s.control_cl = 1.0;
                    }
                    if clamp < 0.5 {
                        clamp = 0.5;
                    }
                    thickness = (1.0 - s.control_cl) * 2.0 - 1.0;
                    out = thickness.abs();
                    bridgerectifier = input_sample_l.abs();
                    if bridgerectifier > 1.57079633 {
                        bridgerectifier = 1.57079633;
                    }
                    if thickness > 0.0 {
                        bridgerectifier = bridgerectifier.sin();
                    } else {
                        bridgerectifier = 1.0 - bridgerectifier.cos();
                    }
                    if input_sample_l > 0.0 {
                        input_sample_l = input_sample_l * (1.0 - out) + bridgerectifier * out;
                    } else {
                        input_sample_l = input_sample_l * (1.0 - out) - bridgerectifier * out;
                    }
                    if clamp != 1.0 {
                        input_sample_l *= clamp;
                    }
                    s.c_r[s.gcount as usize] = input_sample_r.abs() * (intensity - (s.control_apos_cr + s.control_bpos_cr + s.control_aneg_cr + s.control_bneg_cr) * power_sag);
                    s.c_r[(s.gcount + 499) as usize] = s.c_r[s.gcount as usize];
                    s.control_cr += s.c_r[s.gcount as usize] / offset_c as f64;
                    s.control_cr -= s.c_r[(s.gcount + offset_b) as usize] / offset_c as f64;
                    s.control_cr -= 0.000001;
                    clamp = 1.0;
                    if s.control_cr < 0.0 {
                        s.control_cr = 0.0;
                    }
                    if s.control_cr > 1.0 {
                        clamp -= s.control_cr - 1.0;
                        s.control_cr = 1.0;
                    }
                    if clamp < 0.5 {
                        clamp = 0.5;
                    }
                    thickness = (1.0 - s.control_cr) * 2.0 - 1.0;
                    out = thickness.abs();
                    bridgerectifier = input_sample_r.abs();
                    if bridgerectifier > 1.57079633 {
                        bridgerectifier = 1.57079633;
                    }
                    if thickness > 0.0 {
                        bridgerectifier = bridgerectifier.sin();
                    } else {
                        bridgerectifier = 1.0 - bridgerectifier.cos();
                    }
                    if input_sample_r > 0.0 {
                        input_sample_r = input_sample_r * (1.0 - out) + bridgerectifier * out;
                    } else {
                        input_sample_r = input_sample_r * (1.0 - out) - bridgerectifier * out;
                    }
                    if clamp != 1.0 {
                        input_sample_r *= clamp;
                    }
                    inputpos = input_sample_l * fp_old + s.avg_cl * fp_new + 1.0;
                    s.avg_cl = input_sample_l;
                    if inputpos < 0.001 {
                        inputpos = 0.001;
                    }
                    outputpos = inputpos / 2.0;
                    if outputpos > 1.0 {
                        outputpos = 1.0;
                    }
                    inputpos *= inputpos;
                    dynamic_remainder = remainder_c * (inputpos + 1.0);
                    if dynamic_remainder > 1.0 {
                        dynamic_remainder = 1.0;
                    }
                    dynamic_divisor = 1.0 - dynamic_remainder;
                    s.targetpos_cl *= dynamic_divisor;
                    s.targetpos_cl += inputpos * dynamic_remainder;
                    calcpos = (1.0 / s.targetpos_cl).powf(2.0);
                    inputneg = -input_sample_l * fp_old + s.nvg_cl * fp_new + 1.0;
                    s.nvg_cl = -input_sample_l;
                    if inputneg < 0.001 {
                        inputneg = 0.001;
                    }
                    outputneg = inputneg / 2.0;
                    if outputneg > 1.0 {
                        outputneg = 1.0;
                    }
                    inputneg *= inputneg;
                    dynamic_remainder = remainder_c * (inputneg + 1.0);
                    if dynamic_remainder > 1.0 {
                        dynamic_remainder = 1.0;
                    }
                    dynamic_divisor = 1.0 - dynamic_remainder;
                    s.targetneg_cl *= dynamic_divisor;
                    s.targetneg_cl += inputneg * dynamic_remainder;
                    calcneg = (1.0 / s.targetneg_cl).powf(2.0);
                    if input_sample_l > 0.0 {
                        if true as i32 == s.fp_flip as i32 {
                            s.control_apos_cl *= divisor_c;
                            s.control_apos_cl += calcpos * remainder_c;
                            if s.control_apos_cr > s.control_apos_cl {
                                s.control_apos_cr = (s.control_apos_cr + s.control_apos_cl) * 0.5;
                            }
                        } else {
                            s.control_bpos_cl *= divisor_c;
                            s.control_bpos_cl += calcpos * remainder_c;
                            if s.control_bpos_cr > s.control_bpos_cl {
                                s.control_bpos_cr = (s.control_bpos_cr + s.control_bpos_cl) * 0.5;
                            }
                        }
                    } else {
                        if true as i32 == s.fp_flip as i32 {
                            s.control_aneg_cl *= divisor_c;
                            s.control_aneg_cl += calcneg * remainder_c;
                            if s.control_aneg_cr > s.control_aneg_cl {
                                s.control_aneg_cr = (s.control_aneg_cr + s.control_aneg_cl) * 0.5;
                            }
                        } else {
                            s.control_bneg_cl *= divisor_c;
                            s.control_bneg_cl += calcneg * remainder_c;
                            if s.control_bneg_cr > s.control_bneg_cl {
                                s.control_bneg_cr = (s.control_bneg_cr + s.control_bneg_cl) * 0.5;
                            }
                        }
                    }
                    if true as i32 == s.fp_flip as i32 {
                        totalmultiplier = s.control_apos_cl * outputpos + s.control_aneg_cl * outputneg;
                    } else {
                        totalmultiplier = s.control_bpos_cl * outputpos + s.control_bneg_cl * outputneg;
                    }
                    if totalmultiplier != 1.0 {
                        input_sample_l *= totalmultiplier;
                    }
                    if input_sample_l > 36.0 {
                        input_sample_l = 36.0;
                    }
                    if input_sample_l < -36.0 {
                        input_sample_l = -36.0;
                    }
                    out_sample_cl = input_sample_l / compoutgain;
                    inputpos = input_sample_r * fp_old + s.avg_cr * fp_new + 1.0;
                    s.avg_cr = input_sample_r;
                    if inputpos < 0.001 {
                        inputpos = 0.001;
                    }
                    outputpos = inputpos / 2.0;
                    if outputpos > 1.0 {
                        outputpos = 1.0;
                    }
                    inputpos *= inputpos;
                    dynamic_remainder = remainder_c * (inputpos + 1.0);
                    if dynamic_remainder > 1.0 {
                        dynamic_remainder = 1.0;
                    }
                    dynamic_divisor = 1.0 - dynamic_remainder;
                    s.targetpos_cl *= dynamic_divisor;
                    s.targetpos_cl += inputpos * dynamic_remainder;
                    calcpos = (1.0 / s.targetpos_cr).powf(2.0);
                    inputneg = -input_sample_r * fp_old + s.nvg_cr * fp_new + 1.0;
                    s.nvg_cr = -input_sample_r;
                    if inputneg < 0.001 {
                        inputneg = 0.001;
                    }
                    outputneg = inputneg / 2.0;
                    if outputneg > 1.0 {
                        outputneg = 1.0;
                    }
                    inputneg *= inputneg;
                    dynamic_remainder = remainder_c * (inputneg + 1.0);
                    if dynamic_remainder > 1.0 {
                        dynamic_remainder = 1.0;
                    }
                    dynamic_divisor = 1.0 - dynamic_remainder;
                    s.targetneg_cr *= dynamic_divisor;
                    s.targetneg_cr += inputneg * dynamic_remainder;
                    calcneg = (1.0 / s.targetneg_cr).powf(2.0);
                    if input_sample_r > 0.0 {
                        if true as i32 == s.fp_flip as i32 {
                            s.control_apos_cr *= divisor_c;
                            s.control_apos_cr += calcpos * remainder_c;
                            if s.control_apos_cl > s.control_apos_cr {
                                s.control_apos_cl = (s.control_apos_cr + s.control_apos_cl) * 0.5;
                            }
                        } else {
                            s.control_bpos_cr *= divisor_c;
                            s.control_bpos_cr += calcpos * remainder_c;
                            if s.control_bpos_cl > s.control_bpos_cr {
                                s.control_bpos_cl = (s.control_bpos_cr + s.control_bpos_cl) * 0.5;
                            }
                        }
                    } else {
                        if true as i32 == s.fp_flip as i32 {
                            s.control_aneg_cr *= divisor_c;
                            s.control_aneg_cr += calcneg * remainder_c;
                            if s.control_aneg_cl > s.control_aneg_cr {
                                s.control_aneg_cl = (s.control_aneg_cr + s.control_aneg_cl) * 0.5;
                            }
                        } else {
                            s.control_bneg_cr *= divisor_c;
                            s.control_bneg_cr += calcneg * remainder_c;
                            if s.control_bneg_cl > s.control_bneg_cr {
                                s.control_bneg_cl = (s.control_bneg_cr + s.control_bneg_cl) * 0.5;
                            }
                        }
                    }
                    if true as i32 == s.fp_flip as i32 {
                        totalmultiplier = s.control_apos_cr * outputpos + s.control_aneg_cr * outputneg;
                    } else {
                        totalmultiplier = s.control_bpos_cr * outputpos + s.control_bneg_cr * outputneg;
                    }
                    if totalmultiplier != 1.0 {
                        input_sample_r *= totalmultiplier;
                    }
                    if input_sample_r > 36.0 {
                        input_sample_r = 36.0;
                    }
                    if input_sample_r < -36.0 {
                        input_sample_r = -36.0;
                    }
                    out_sample_cr = input_sample_r / compoutgain;
                }
            }
            match ratioselector {
                0 => {
                    input_sample_l = dry_sample_l as f64 * inv_ratio + out_sample_al * ratio;
                    input_sample_r = dry_sample_r as f64 * inv_ratio + out_sample_ar * ratio;
                }
                1 => {
                    input_sample_l = out_sample_al * inv_ratio + out_sample_bl * ratio;
                    input_sample_r = out_sample_ar * inv_ratio + out_sample_br * ratio;
                }
                _ => {
                    input_sample_l = out_sample_bl * inv_ratio + out_sample_cl * ratio;
                    input_sample_r = out_sample_br * inv_ratio + out_sample_cr * ratio;
                }
            }
            if outputgain != 1.0 {
                input_sample_l *= outputgain;
                input_sample_r *= outputgain;
            }
            if wet != 1.0 {
                input_sample_l = input_sample_l * wet + dry_sample_l as f64 * (1.0 - wet);
                input_sample_r = input_sample_r * wet + dry_sample_r as f64 * (1.0 - wet);
            }
            s.fp_flip = !s.fp_flip;
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
