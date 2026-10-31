// Ported from ZNotch2 (ZNotch2Proc.cpp, ZNotch2.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! ZNotch (sub-effect id 76): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;
#[allow(unused_imports)]
use std::f64::consts::{PI};

const FIX_FREQ: usize = 0;
const BIQ_FREQ: usize = 0;
const FIX_RESO: usize = 1;
const BIQ_RESO: usize = 1;
const FIX_A0: usize = 2;
const BIQ_A0: usize = 2;
const FIX_A1: usize = 3;
const BIQ_A1: usize = 3;
const BIQ_A2: usize = 4;
const FIX_A2: usize = 4;
const BIQ_B1: usize = 5;
const FIX_B1: usize = 5;
const FIX_B2: usize = 6;
const BIQ_B2: usize = 6;
const BIQ_A_A0: usize = 7;
const FIX_S_L1: usize = 7;
const BIQ_A_A1: usize = 8;
const FIX_S_L2: usize = 8;
const FIX_S_R1: usize = 9;
const BIQ_A_A2: usize = 9;
const BIQ_B_A1: usize = 10;
const FIX_S_R2: usize = 10;
const BIQ_B_A2: usize = 11;
const BIQ_A_B0: usize = 12;
const BIQ_A_B1: usize = 13;
const BIQ_A_B2: usize = 14;
const BIQ_B_B1: usize = 15;
const BIQ_B_B2: usize = 16;
const BIQ_S_L1: usize = 17;
const BIQ_S_L2: usize = 18;
const BIQ_S_R1: usize = 19;
const BIQ_S_R2: usize = 20;

pub const PARAMS: &[SlotDef] = &[
    pf("Input", 0.1),
    pf("Freq", 0.5),
    pf("Output", 1.0),
    pf("Poles", 0.5),
];

#[allow(dead_code)]
pub struct ZNotch2 {
    p: [f32; 4],
    sr: f32,
    iir_sample_al: f64,
    iir_sample_ar: f64,
    biquad_a: [f64; 21],
    biquad_b: [f64; 21],
    biquad_c: [f64; 21],
    biquad_d: [f64; 21],
    in_trim_a: f64,
    in_trim_b: f64,
    out_trim_a: f64,
    out_trim_b: f64,
    wet_a: f64,
    wet_b: f64,
    fix_a: [f64; 11],
    fix_b: [f64; 11],
    fpd_l: u32,
    fpd_r: u32,
}

impl ZNotch2 {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<ZNotch2> {
        let mut s = Box::new(ZNotch2 {
            p: [0.1, 0.5, 1.0, 0.5],
            sr: sr as f32,
            iir_sample_al: 0.0,
            iir_sample_ar: 0.0,
            biquad_a: [0.0; 21],
            biquad_b: [0.0; 21],
            biquad_c: [0.0; 21],
            biquad_d: [0.0; 21],
            in_trim_a: 0.0,
            in_trim_b: 0.0,
            out_trim_a: 0.0,
            out_trim_b: 0.0,
            wet_a: 0.0,
            wet_b: 0.0,
            fix_a: [0.0; 11],
            fix_b: [0.0; 11],
            fpd_l: 0,
            fpd_r: 0,
        });
        s.in_trim_a = 0.1;
        s.in_trim_b = 0.1;
        s.out_trim_a = 1.0;
        s.out_trim_b = 1.0;
        s.wet_a = 0.5;
        s.wet_b = 0.5;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for ZNotch2 {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut in_frames_to_process: i32 = l.len() as i32;
        let mut overallscale: f64 = 1.0;
        overallscale /= 44100.0;
        overallscale *= s.sr as f64;
        s.biquad_a[BIQ_FREQ] = (s.p[1] as f64).powf(3.0) * 4700.0 / s.sr as f64 + 0.0009963;
        let mut clip_factor: f64 = 0.91 - (1.0 - s.p[1] as f64) * 0.15;
        s.biquad_a[BIQ_RESO] = 0.618033988749894848204586;
        s.biquad_a[BIQ_A_A0] = s.biquad_a[BIQ_A_B0];
        s.biquad_a[BIQ_A_A1] = s.biquad_a[BIQ_A_B1];
        s.biquad_a[BIQ_A_A2] = s.biquad_a[BIQ_A_B2];
        s.biquad_a[BIQ_B_A1] = s.biquad_a[BIQ_B_B1];
        s.biquad_a[BIQ_B_A2] = s.biquad_a[BIQ_B_B2];
        let mut k: f64 = (PI * s.biquad_a[BIQ_FREQ]).tan();
        let mut norm: f64 = 1.0 / (1.0 + k / s.biquad_a[BIQ_RESO] + k * k);
        s.biquad_a[BIQ_A_B0] = (1.0 + k * k) * norm;
        s.biquad_a[BIQ_A_B1] = 2.0 * (k * k - 1.0) * norm;
        s.biquad_a[BIQ_A_B2] = s.biquad_a[BIQ_A_B0];
        s.biquad_a[BIQ_B_B1] = s.biquad_a[BIQ_A_B1];
        s.biquad_a[BIQ_B_B2] = (1.0 - k / s.biquad_a[BIQ_RESO] + k * k) * norm;
        s.in_trim_a = s.in_trim_b;
        s.in_trim_b = s.p[0] as f64 * 10.0;
        s.in_trim_b *= s.in_trim_b;
        s.in_trim_b *= s.in_trim_b;
        s.out_trim_a = s.out_trim_b;
        s.out_trim_b = s.p[2] as f64 * 10.0;
        s.wet_a = s.wet_b;
        s.wet_b = (s.p[3] as f64).powf(2.0);
        let mut iir_amount_a: f64 = 0.00069 / overallscale;
        s.fix_b[FIX_FREQ] = 15500.0 / s.sr as f64;
        s.fix_a[FIX_FREQ] = s.fix_b[FIX_FREQ];
        s.fix_b[FIX_RESO] = 0.935;
        s.fix_a[FIX_RESO] = s.fix_b[FIX_RESO];
        k = (PI * s.fix_b[FIX_FREQ]).tan();
        norm = 1.0 / (1.0 + k / s.fix_b[FIX_RESO] + k * k);
        s.fix_b[FIX_A0] = k * k * norm;
        s.fix_a[FIX_A0] = s.fix_b[FIX_A0];
        s.fix_b[FIX_A1] = 2.0 * s.fix_b[FIX_A0];
        s.fix_a[FIX_A1] = s.fix_b[FIX_A1];
        s.fix_b[FIX_A2] = s.fix_b[FIX_A0];
        s.fix_a[FIX_A2] = s.fix_b[FIX_A2];
        s.fix_b[FIX_B1] = 2.0 * (k * k - 1.0) * norm;
        s.fix_a[FIX_B1] = s.fix_b[FIX_B1];
        s.fix_b[FIX_B2] = (1.0 - k / s.fix_b[FIX_RESO] + k * k) * norm;
        s.fix_a[FIX_B2] = s.fix_b[FIX_B2];
        let mut trim: f64 = 0.1 + 3.712 * s.biquad_a[BIQ_FREQ];
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
            let mut overall_dry_sample_l: f64 = input_sample_l;
            let mut overall_dry_sample_r: f64 = input_sample_r;
            let mut out_sample: f64 = (l.len() - 1 - smp) as i32 as f64 / in_frames_to_process as f64;
            s.biquad_a[BIQ_A0] = s.biquad_a[BIQ_A_A0] * out_sample + s.biquad_a[BIQ_A_B0] * (1.0 - out_sample);
            s.biquad_a[BIQ_A1] = s.biquad_a[BIQ_A_A1] * out_sample + s.biquad_a[BIQ_A_B1] * (1.0 - out_sample);
            s.biquad_a[BIQ_A2] = s.biquad_a[BIQ_A_A2] * out_sample + s.biquad_a[BIQ_A_B2] * (1.0 - out_sample);
            s.biquad_a[BIQ_B1] = s.biquad_a[BIQ_B_A1] * out_sample + s.biquad_a[BIQ_B_B1] * (1.0 - out_sample);
            s.biquad_a[BIQ_B2] = s.biquad_a[BIQ_B_A2] * out_sample + s.biquad_a[BIQ_B_B2] * (1.0 - out_sample);
            {
                let mut x: i32 = 0;
                while x < 7 {
                    s.biquad_b[x as usize] = s.biquad_a[x as usize];
                    s.biquad_c[x as usize] = s.biquad_b[x as usize];
                    s.biquad_d[x as usize] = s.biquad_c[x as usize];
                    x += 1;
                }
            }
            let mut in_trim: f64 = s.in_trim_a * out_sample + s.in_trim_b * (1.0 - out_sample);
            let mut out_trim: f64 = s.out_trim_a * out_sample + s.out_trim_b * (1.0 - out_sample);
            let mut wet: f64 = s.wet_a * out_sample + s.wet_b * (1.0 - out_sample);
            let mut a_wet: f64 = 1.0;
            let mut b_wet: f64 = 1.0;
            let mut c_wet: f64 = 1.0;
            let mut d_wet: f64 = wet * 4.0;
            if d_wet < 1.0 {
                a_wet = d_wet;
                b_wet = 0.0;
                c_wet = 0.0;
                d_wet = 0.0;
            } else if d_wet < 2.0 {
                b_wet = d_wet - 1.0;
                c_wet = 0.0;
                d_wet = 0.0;
            } else if d_wet < 3.0 {
                c_wet = d_wet - 2.0;
                d_wet = 0.0;
            } else {
                d_wet -= 3.0;
            }
            if in_trim != 1.0 {
                input_sample_l *= in_trim;
                input_sample_r *= in_trim;
            }
            if input_sample_l > 1.0 {
                input_sample_l = 1.0;
            }
            if input_sample_l < -1.0 {
                input_sample_l = -1.0;
            }
            if input_sample_r > 1.0 {
                input_sample_r = 1.0;
            }
            if input_sample_r < -1.0 {
                input_sample_r = -1.0;
            }
            input_sample_l *= trim;
            input_sample_r *= trim;
            input_sample_l /= clip_factor;
            input_sample_r /= clip_factor;
            out_sample = input_sample_l * s.biquad_a[BIQ_A0] + s.biquad_a[BIQ_S_L1];
            if out_sample > 1.0 {
                out_sample = 1.0;
            }
            if out_sample < -1.0 {
                out_sample = -1.0;
            }
            s.biquad_a[BIQ_S_L1] = input_sample_l * s.biquad_a[BIQ_A1] - out_sample * s.biquad_a[BIQ_B1] + s.biquad_a[BIQ_S_L2];
            s.biquad_a[BIQ_S_L2] = input_sample_l * s.biquad_a[BIQ_A2] - out_sample * s.biquad_a[BIQ_B2];
            input_sample_l = out_sample;
            dry_sample_l = input_sample_l;
            out_sample = input_sample_r * s.biquad_a[BIQ_A0] + s.biquad_a[BIQ_S_R1];
            if out_sample > 1.0 {
                out_sample = 1.0;
            }
            if out_sample < -1.0 {
                out_sample = -1.0;
            }
            s.biquad_a[BIQ_S_R1] = input_sample_r * s.biquad_a[BIQ_A1] - out_sample * s.biquad_a[BIQ_B1] + s.biquad_a[BIQ_S_R2];
            s.biquad_a[BIQ_S_R2] = input_sample_r * s.biquad_a[BIQ_A2] - out_sample * s.biquad_a[BIQ_B2];
            input_sample_r = out_sample;
            dry_sample_r = input_sample_r;
            if b_wet > 0.0 {
                input_sample_l /= clip_factor;
                out_sample = input_sample_l * s.biquad_b[BIQ_A0] + s.biquad_b[BIQ_S_L1];
                if out_sample > 1.0 {
                    out_sample = 1.0;
                }
                if out_sample < -1.0 {
                    out_sample = -1.0;
                }
                s.biquad_b[BIQ_S_L1] = input_sample_l * s.biquad_b[BIQ_A1] - out_sample * s.biquad_b[BIQ_B1] + s.biquad_b[BIQ_S_L2];
                s.biquad_b[BIQ_S_L2] = input_sample_l * s.biquad_b[BIQ_A2] - out_sample * s.biquad_b[BIQ_B2];
                input_sample_l = out_sample * b_wet + dry_sample_l * (1.0 - b_wet);
                dry_sample_l = input_sample_l;
                input_sample_r /= clip_factor;
                out_sample = input_sample_r * s.biquad_b[BIQ_A0] + s.biquad_b[BIQ_S_R1];
                if out_sample > 1.0 {
                    out_sample = 1.0;
                }
                if out_sample < -1.0 {
                    out_sample = -1.0;
                }
                s.biquad_b[BIQ_S_R1] = input_sample_r * s.biquad_b[BIQ_A1] - out_sample * s.biquad_b[BIQ_B1] + s.biquad_b[BIQ_S_R2];
                s.biquad_b[BIQ_S_R2] = input_sample_r * s.biquad_b[BIQ_A2] - out_sample * s.biquad_b[BIQ_B2];
                input_sample_r = out_sample * b_wet + dry_sample_r * (1.0 - b_wet);
                dry_sample_r = input_sample_r;
            }
            if c_wet > 0.0 {
                input_sample_l /= clip_factor;
                out_sample = input_sample_l * s.biquad_c[BIQ_A0] + s.biquad_c[BIQ_S_L1];
                if out_sample > 1.0 {
                    out_sample = 1.0;
                }
                if out_sample < -1.0 {
                    out_sample = -1.0;
                }
                s.biquad_c[BIQ_S_L1] = input_sample_l * s.biquad_c[BIQ_A1] - out_sample * s.biquad_c[BIQ_B1] + s.biquad_c[BIQ_S_L2];
                s.biquad_c[BIQ_S_L2] = input_sample_l * s.biquad_c[BIQ_A2] - out_sample * s.biquad_c[BIQ_B2];
                input_sample_l = out_sample * c_wet + dry_sample_l * (1.0 - c_wet);
                dry_sample_l = input_sample_l;
                input_sample_r /= clip_factor;
                out_sample = input_sample_r * s.biquad_c[BIQ_A0] + s.biquad_c[BIQ_S_R1];
                if out_sample > 1.0 {
                    out_sample = 1.0;
                }
                if out_sample < -1.0 {
                    out_sample = -1.0;
                }
                s.biquad_c[BIQ_S_R1] = input_sample_r * s.biquad_c[BIQ_A1] - out_sample * s.biquad_c[BIQ_B1] + s.biquad_c[BIQ_S_R2];
                s.biquad_c[BIQ_S_R2] = input_sample_r * s.biquad_c[BIQ_A2] - out_sample * s.biquad_c[BIQ_B2];
                input_sample_r = out_sample * c_wet + dry_sample_r * (1.0 - c_wet);
                dry_sample_r = input_sample_r;
            }
            if d_wet > 0.0 {
                input_sample_l /= clip_factor;
                out_sample = input_sample_l * s.biquad_d[BIQ_A0] + s.biquad_d[BIQ_S_L1];
                if out_sample > 1.0 {
                    out_sample = 1.0;
                }
                if out_sample < -1.0 {
                    out_sample = -1.0;
                }
                s.biquad_d[BIQ_S_L1] = input_sample_l * s.biquad_d[BIQ_A1] - out_sample * s.biquad_d[BIQ_B1] + s.biquad_d[BIQ_S_L2];
                s.biquad_d[BIQ_S_L2] = input_sample_l * s.biquad_d[BIQ_A2] - out_sample * s.biquad_d[BIQ_B2];
                input_sample_l = out_sample * d_wet + dry_sample_l * (1.0 - d_wet);
                dry_sample_l = input_sample_l;
                input_sample_r /= clip_factor;
                out_sample = input_sample_r * s.biquad_d[BIQ_A0] + s.biquad_d[BIQ_S_R1];
                if out_sample > 1.0 {
                    out_sample = 1.0;
                }
                if out_sample < -1.0 {
                    out_sample = -1.0;
                }
                s.biquad_d[BIQ_S_R1] = input_sample_r * s.biquad_d[BIQ_A1] - out_sample * s.biquad_d[BIQ_B1] + s.biquad_d[BIQ_S_R2];
                s.biquad_d[BIQ_S_R2] = input_sample_r * s.biquad_d[BIQ_A2] - out_sample * s.biquad_d[BIQ_B2];
                input_sample_r = out_sample * d_wet + dry_sample_r * (1.0 - d_wet);
                dry_sample_r = input_sample_r;
            }
            input_sample_l /= clip_factor;
            input_sample_r /= clip_factor;
            if s.iir_sample_al.abs() < 1.18e-37 {
                s.iir_sample_al = 0.0;
            }
            s.iir_sample_al = s.iir_sample_al * (1.0 - iir_amount_a) + input_sample_l * iir_amount_a;
            input_sample_l -= s.iir_sample_al;
            if s.iir_sample_ar.abs() < 1.18e-37 {
                s.iir_sample_ar = 0.0;
            }
            s.iir_sample_ar = s.iir_sample_ar * (1.0 - iir_amount_a) + input_sample_r * iir_amount_a;
            input_sample_r -= s.iir_sample_ar;
            out_sample = input_sample_l * s.fix_a[FIX_A0] + s.fix_a[FIX_S_L1];
            s.fix_a[FIX_S_L1] = input_sample_l * s.fix_a[FIX_A1] - out_sample * s.fix_a[FIX_B1] + s.fix_a[FIX_S_L2];
            s.fix_a[FIX_S_L2] = input_sample_l * s.fix_a[FIX_A2] - out_sample * s.fix_a[FIX_B2];
            input_sample_l = out_sample;
            out_sample = input_sample_r * s.fix_a[FIX_A0] + s.fix_a[FIX_S_R1];
            s.fix_a[FIX_S_R1] = input_sample_r * s.fix_a[FIX_A1] - out_sample * s.fix_a[FIX_B1] + s.fix_a[FIX_S_R2];
            s.fix_a[FIX_S_R2] = input_sample_r * s.fix_a[FIX_A2] - out_sample * s.fix_a[FIX_B2];
            input_sample_r = out_sample;
            if input_sample_l > 1.0 {
                input_sample_l = 1.0;
            }
            if input_sample_l < -1.0 {
                input_sample_l = -1.0;
            }
            input_sample_l -= input_sample_l * input_sample_l * input_sample_l * input_sample_l * input_sample_l * 0.1768;
            if input_sample_r > 1.0 {
                input_sample_r = 1.0;
            }
            if input_sample_r < -1.0 {
                input_sample_r = -1.0;
            }
            input_sample_r -= input_sample_r * input_sample_r * input_sample_r * input_sample_r * input_sample_r * 0.1768;
            out_sample = input_sample_l * s.fix_b[FIX_A0] + s.fix_b[FIX_S_L1];
            s.fix_b[FIX_S_L1] = input_sample_l * s.fix_b[FIX_A1] - out_sample * s.fix_b[FIX_B1] + s.fix_b[FIX_S_L2];
            s.fix_b[FIX_S_L2] = input_sample_l * s.fix_b[FIX_A2] - out_sample * s.fix_b[FIX_B2];
            input_sample_l = out_sample;
            out_sample = input_sample_r * s.fix_b[FIX_A0] + s.fix_b[FIX_S_R1];
            s.fix_b[FIX_S_R1] = input_sample_r * s.fix_b[FIX_A1] - out_sample * s.fix_b[FIX_B1] + s.fix_b[FIX_S_R2];
            s.fix_b[FIX_S_R2] = input_sample_r * s.fix_b[FIX_A2] - out_sample * s.fix_b[FIX_B2];
            input_sample_r = out_sample;
            if out_trim != 1.0 {
                input_sample_l *= out_trim;
                input_sample_r *= out_trim;
            }
            if a_wet != 1.0 {
                input_sample_l = input_sample_l * a_wet + overall_dry_sample_l * (1.0 - a_wet);
                input_sample_r = input_sample_r * a_wet + overall_dry_sample_r * (1.0 - a_wet);
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
