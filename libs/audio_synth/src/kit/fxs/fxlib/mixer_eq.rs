// Ported from MackEQ (MackEQProc.cpp, MackEQ.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! MackEQ (sub-effect id 56): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;
#[allow(unused_imports)]
use std::f64::consts::{PI};

pub const PARAMS: &[SlotDef] = &[
    pf("Trim", 0.1),
    pf("Hi", 0.5),
    pf("Lo", 0.5),
    pf("Gain", 1.0),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct MixerEq {
    p: [f32; 5],
    sr: f32,
    iir_sample_al: f64,
    iir_sample_bl: f64,
    iir_sample_cl: f64,
    iir_sample_dl: f64,
    iir_sample_el: f64,
    iir_sample_fl: f64,
    iir_sample_ar: f64,
    iir_sample_br: f64,
    iir_sample_cr: f64,
    iir_sample_dr: f64,
    iir_sample_er: f64,
    iir_sample_fr: f64,
    biquad_a: [f64; 15],
    biquad_b: [f64; 15],
    biquad_c: [f64; 15],
    biquad_d: [f64; 15],
    fpd_l: u32,
    fpd_r: u32,
}

impl MixerEq {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<MixerEq> {
        let mut s = Box::new(MixerEq {
            p: [0.1, 0.5, 0.5, 1.0, 1.0],
            sr: sr as f32,
            iir_sample_al: 0.0,
            iir_sample_bl: 0.0,
            iir_sample_cl: 0.0,
            iir_sample_dl: 0.0,
            iir_sample_el: 0.0,
            iir_sample_fl: 0.0,
            iir_sample_ar: 0.0,
            iir_sample_br: 0.0,
            iir_sample_cr: 0.0,
            iir_sample_dr: 0.0,
            iir_sample_er: 0.0,
            iir_sample_fr: 0.0,
            biquad_a: [0.0; 15],
            biquad_b: [0.0; 15],
            biquad_c: [0.0; 15],
            biquad_d: [0.0; 15],
            fpd_l: 0,
            fpd_r: 0,
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for MixerEq {
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
        let mut in_trim: f64 = s.p[0] as f64 * 10.0;
        in_trim *= in_trim;
        let mut gain_high: f64 = (s.p[1] as f64).powf(2.0) * 4.0;
        let mut out_high: f64 = s.p[1].sqrt() as f64;
        let mut gain_bass: f64 = (s.p[2] as f64).powf(2.0) * 4.0;
        let mut out_bass: f64 = s.p[2].sqrt() as f64;
        let mut out_pad: f64 = s.p[3] as f64;
        let mut wet: f64 = s.p[4] as f64;
        let mut iir_amount_a: f64 = 0.001860867 / overallscale;
        let mut iir_amount_b: f64 = 0.000287496 / overallscale;
        let mut iir_bass_mid: f64 = 0.159 / overallscale;
        let mut iir_mid_high: f64 = 0.236 / overallscale;
        s.biquad_a[0] = 19160.0 / s.sr as f64;
        s.biquad_b[0] = s.biquad_a[0];
        s.biquad_c[0] = s.biquad_b[0];
        s.biquad_d[0] = s.biquad_c[0];
        s.biquad_a[1] = 0.431684981684982;
        s.biquad_b[1] = 1.1582298;
        s.biquad_c[1] = 0.657027382751269;
        s.biquad_d[1] = 1.076210852946577;
        let mut k: f64 = (PI * s.biquad_a[0]).tan();
        let mut norm: f64 = 1.0 / (1.0 + k / s.biquad_a[1] + k * k);
        s.biquad_a[2] = k * k * norm;
        s.biquad_a[3] = 2.0 * s.biquad_a[2];
        s.biquad_a[4] = s.biquad_a[2];
        s.biquad_a[5] = 2.0 * (k * k - 1.0) * norm;
        s.biquad_a[6] = (1.0 - k / s.biquad_a[1] + k * k) * norm;
        k = (PI * s.biquad_b[0]).tan();
        norm = 1.0 / (1.0 + k / s.biquad_b[1] + k * k);
        s.biquad_b[2] = k * k * norm;
        s.biquad_b[3] = 2.0 * s.biquad_b[2];
        s.biquad_b[4] = s.biquad_b[2];
        s.biquad_b[5] = 2.0 * (k * k - 1.0) * norm;
        s.biquad_b[6] = (1.0 - k / s.biquad_b[1] + k * k) * norm;
        k = (PI * s.biquad_c[0]).tan();
        norm = 1.0 / (1.0 + k / s.biquad_c[1] + k * k);
        s.biquad_c[2] = k * k * norm;
        s.biquad_c[3] = 2.0 * s.biquad_c[2];
        s.biquad_c[4] = s.biquad_c[2];
        s.biquad_c[5] = 2.0 * (k * k - 1.0) * norm;
        s.biquad_c[6] = (1.0 - k / s.biquad_c[1] + k * k) * norm;
        k = (PI * s.biquad_d[0]).tan();
        norm = 1.0 / (1.0 + k / s.biquad_d[1] + k * k);
        s.biquad_d[2] = k * k * norm;
        s.biquad_d[3] = 2.0 * s.biquad_d[2];
        s.biquad_d[4] = s.biquad_d[2];
        s.biquad_d[5] = 2.0 * (k * k - 1.0) * norm;
        s.biquad_d[6] = (1.0 - k / s.biquad_d[1] + k * k) * norm;
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
            if in_trim != 1.0 {
                input_sample_l *= in_trim;
                input_sample_r *= in_trim;
            }
            let mut out_sample_l: f64 = s.biquad_a[2] * input_sample_l + s.biquad_a[3] * s.biquad_a[7] + s.biquad_a[4] * s.biquad_a[8] - s.biquad_a[5] * s.biquad_a[9] - s.biquad_a[6] * s.biquad_a[10];
            s.biquad_a[8] = s.biquad_a[7];
            s.biquad_a[7] = input_sample_l;
            input_sample_l = out_sample_l;
            s.biquad_a[10] = s.biquad_a[9];
            s.biquad_a[9] = input_sample_l;
            let mut out_sample_r: f64 = s.biquad_a[2] * input_sample_r + s.biquad_a[3] * s.biquad_a[11] + s.biquad_a[4] * s.biquad_a[12] - s.biquad_a[5] * s.biquad_a[13] - s.biquad_a[6] * s.biquad_a[14];
            s.biquad_a[12] = s.biquad_a[11];
            s.biquad_a[11] = input_sample_r;
            input_sample_r = out_sample_r;
            s.biquad_a[14] = s.biquad_a[13];
            s.biquad_a[13] = input_sample_r;
            if input_sample_l > 1.0 {
                input_sample_l = 1.0;
            }
            if input_sample_l < -1.0 {
                input_sample_l = -1.0;
            }
            input_sample_l -= input_sample_l.powf(5.0) * 0.1768;
            if input_sample_r > 1.0 {
                input_sample_r = 1.0;
            }
            if input_sample_r < -1.0 {
                input_sample_r = -1.0;
            }
            input_sample_r -= input_sample_r.powf(5.0) * 0.1768;
            out_sample_l = s.biquad_b[2] * input_sample_l + s.biquad_b[3] * s.biquad_b[7] + s.biquad_b[4] * s.biquad_b[8] - s.biquad_b[5] * s.biquad_b[9] - s.biquad_b[6] * s.biquad_b[10];
            s.biquad_b[8] = s.biquad_b[7];
            s.biquad_b[7] = input_sample_l;
            input_sample_l = out_sample_l;
            s.biquad_b[10] = s.biquad_b[9];
            s.biquad_b[9] = input_sample_l;
            out_sample_r = s.biquad_b[2] * input_sample_r + s.biquad_b[3] * s.biquad_b[11] + s.biquad_b[4] * s.biquad_b[12] - s.biquad_b[5] * s.biquad_b[13] - s.biquad_b[6] * s.biquad_b[14];
            s.biquad_b[12] = s.biquad_b[11];
            s.biquad_b[11] = input_sample_r;
            input_sample_r = out_sample_r;
            s.biquad_b[14] = s.biquad_b[13];
            s.biquad_b[13] = input_sample_r;
            if s.iir_sample_bl.abs() < 1.18e-37 {
                s.iir_sample_bl = 0.0;
            }
            s.iir_sample_bl = s.iir_sample_bl * (1.0 - iir_amount_b) + input_sample_l * iir_amount_b;
            input_sample_l -= s.iir_sample_bl;
            if s.iir_sample_br.abs() < 1.18e-37 {
                s.iir_sample_br = 0.0;
            }
            s.iir_sample_br = s.iir_sample_br * (1.0 - iir_amount_b) + input_sample_r * iir_amount_b;
            input_sample_r -= s.iir_sample_br;
            if s.iir_sample_cl.abs() < 1.18e-37 {
                s.iir_sample_cl = 0.0;
            }
            s.iir_sample_cl = s.iir_sample_cl * (1.0 - iir_bass_mid) + input_sample_l * iir_bass_mid;
            if s.iir_sample_cr.abs() < 1.18e-37 {
                s.iir_sample_cr = 0.0;
            }
            s.iir_sample_cr = s.iir_sample_cr * (1.0 - iir_bass_mid) + input_sample_r * iir_bass_mid;
            let mut bass_sample_l: f64 = s.iir_sample_cl;
            let mut mid_sample_l: f64 = input_sample_l - bass_sample_l;
            let mut bass_sample_r: f64 = s.iir_sample_cr;
            let mut mid_sample_r: f64 = input_sample_r - bass_sample_r;
            if gain_bass != 1.0 {
                bass_sample_l *= gain_bass;
                bass_sample_r *= gain_bass;
            }
            if bass_sample_l > 1.0 {
                bass_sample_l = 1.0;
            }
            if bass_sample_l < -1.0 {
                bass_sample_l = -1.0;
            }
            bass_sample_l -= bass_sample_l.powf(5.0) * 0.1768;
            if bass_sample_r > 1.0 {
                bass_sample_r = 1.0;
            }
            if bass_sample_r < -1.0 {
                bass_sample_r = -1.0;
            }
            bass_sample_r -= bass_sample_r.powf(5.0) * 0.1768;
            if s.iir_sample_dl.abs() < 1.18e-37 {
                s.iir_sample_dl = 0.0;
            }
            s.iir_sample_dl = s.iir_sample_dl * (1.0 - iir_bass_mid) + bass_sample_l * iir_bass_mid;
            bass_sample_l = s.iir_sample_dl;
            if s.iir_sample_dr.abs() < 1.18e-37 {
                s.iir_sample_dr = 0.0;
            }
            s.iir_sample_dr = s.iir_sample_dr * (1.0 - iir_bass_mid) + bass_sample_r * iir_bass_mid;
            bass_sample_r = s.iir_sample_dr;
            if s.iir_sample_el.abs() < 1.18e-37 {
                s.iir_sample_el = 0.0;
            }
            s.iir_sample_el = s.iir_sample_el * (1.0 - iir_mid_high) + mid_sample_l * iir_mid_high;
            let mut high_sample_l: f64 = mid_sample_l - s.iir_sample_el;
            mid_sample_l = s.iir_sample_el;
            if s.iir_sample_er.abs() < 1.18e-37 {
                s.iir_sample_er = 0.0;
            }
            s.iir_sample_er = s.iir_sample_er * (1.0 - iir_mid_high) + mid_sample_r * iir_mid_high;
            let mut high_sample_r: f64 = mid_sample_r - s.iir_sample_er;
            mid_sample_r = s.iir_sample_er;
            if s.iir_sample_fl.abs() < 1.18e-37 {
                s.iir_sample_fl = 0.0;
            }
            s.iir_sample_fl = s.iir_sample_fl * (1.0 - iir_mid_high) + high_sample_l * iir_mid_high;
            high_sample_l -= s.iir_sample_fl;
            if s.iir_sample_fr.abs() < 1.18e-37 {
                s.iir_sample_fr = 0.0;
            }
            s.iir_sample_fr = s.iir_sample_fr * (1.0 - iir_mid_high) + high_sample_r * iir_mid_high;
            high_sample_r -= s.iir_sample_fr;
            if gain_high != 1.0 {
                high_sample_l *= gain_high;
                high_sample_r *= gain_high;
            }
            if high_sample_l > 1.0 {
                high_sample_l = 1.0;
            }
            if high_sample_l < -1.0 {
                high_sample_l = -1.0;
            }
            high_sample_l -= high_sample_l.powf(5.0) * 0.1768;
            if high_sample_r > 1.0 {
                high_sample_r = 1.0;
            }
            if high_sample_r < -1.0 {
                high_sample_r = -1.0;
            }
            high_sample_r -= high_sample_r.powf(5.0) * 0.1768;
            input_sample_l = (bass_sample_l * out_bass + mid_sample_l + high_sample_l * out_high) * 4.0;
            input_sample_r = (bass_sample_r * out_bass + mid_sample_r + high_sample_r * out_high) * 4.0;
            out_sample_l = s.biquad_c[2] * input_sample_l + s.biquad_c[3] * s.biquad_c[7] + s.biquad_c[4] * s.biquad_c[8] - s.biquad_c[5] * s.biquad_c[9] - s.biquad_c[6] * s.biquad_c[10];
            s.biquad_c[8] = s.biquad_c[7];
            s.biquad_c[7] = input_sample_l;
            input_sample_l = out_sample_l;
            s.biquad_c[10] = s.biquad_c[9];
            s.biquad_c[9] = input_sample_l;
            out_sample_r = s.biquad_c[2] * input_sample_r + s.biquad_c[3] * s.biquad_c[11] + s.biquad_c[4] * s.biquad_c[12] - s.biquad_c[5] * s.biquad_c[13] - s.biquad_c[6] * s.biquad_c[14];
            s.biquad_c[12] = s.biquad_c[11];
            s.biquad_c[11] = input_sample_r;
            input_sample_r = out_sample_r;
            s.biquad_c[14] = s.biquad_c[13];
            s.biquad_c[13] = input_sample_r;
            if input_sample_l > 1.0 {
                input_sample_l = 1.0;
            }
            if input_sample_l < -1.0 {
                input_sample_l = -1.0;
            }
            input_sample_l -= input_sample_l.powf(5.0) * 0.1768;
            if input_sample_r > 1.0 {
                input_sample_r = 1.0;
            }
            if input_sample_r < -1.0 {
                input_sample_r = -1.0;
            }
            input_sample_r -= input_sample_r.powf(5.0) * 0.1768;
            out_sample_l = s.biquad_d[2] * input_sample_l + s.biquad_d[3] * s.biquad_d[7] + s.biquad_d[4] * s.biquad_d[8] - s.biquad_d[5] * s.biquad_d[9] - s.biquad_d[6] * s.biquad_d[10];
            s.biquad_d[8] = s.biquad_d[7];
            s.biquad_d[7] = input_sample_l;
            input_sample_l = out_sample_l;
            s.biquad_d[10] = s.biquad_d[9];
            s.biquad_d[9] = input_sample_l;
            out_sample_r = s.biquad_d[2] * input_sample_r + s.biquad_d[3] * s.biquad_d[11] + s.biquad_d[4] * s.biquad_d[12] - s.biquad_d[5] * s.biquad_d[13] - s.biquad_d[6] * s.biquad_d[14];
            s.biquad_d[12] = s.biquad_d[11];
            s.biquad_d[11] = input_sample_r;
            input_sample_r = out_sample_r;
            s.biquad_d[14] = s.biquad_d[13];
            s.biquad_d[13] = input_sample_r;
            if out_pad != 1.0 {
                input_sample_l *= out_pad;
                input_sample_r *= out_pad;
            }
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
            l[smp] = input_sample_l as f32;
            r[smp] = input_sample_r as f32;
        }
    }
}
