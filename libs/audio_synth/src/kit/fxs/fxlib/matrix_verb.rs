// Ported from MatrixVerb (MatrixVerbProc.cpp, MatrixVerb.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! MatrixVerb (sub-effect id 51): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;
#[allow(unused_imports)]
use std::f64::consts::{PI};

pub const PARAMS: &[SlotDef] = &[
    pf("Filter", 1.0),
    pf("Damping", 0.0),
    pf("Speed", 0.0),
    pf("Vibrato", 0.0),
    pf("RmSize", 0.5),
    pf("Flavor", 0.5),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct MatrixVerb {
    p: [f32; 7],
    sr: f32,
    biquad_a: [f64; 11],
    biquad_b: [f64; 11],
    biquad_c: [f64; 11],
    a_al: Vec<f64>,
    a_bl: Vec<f64>,
    a_cl: Vec<f64>,
    a_dl: Vec<f64>,
    a_el: Vec<f64>,
    a_fl: Vec<f64>,
    a_gl: Vec<f64>,
    a_hl: Vec<f64>,
    a_il: Vec<f64>,
    a_jl: Vec<f64>,
    a_kl: Vec<f64>,
    a_ll: Vec<f64>,
    a_ml: Vec<f64>,
    a_ar: Vec<f64>,
    a_br: Vec<f64>,
    a_cr: Vec<f64>,
    a_dr: Vec<f64>,
    a_er: Vec<f64>,
    a_fr: Vec<f64>,
    a_gr: Vec<f64>,
    a_hr: Vec<f64>,
    a_ir: Vec<f64>,
    a_jr: Vec<f64>,
    a_kr: Vec<f64>,
    a_lr: Vec<f64>,
    a_mr: Vec<f64>,
    count_a: i32,
    delay_a: i32,
    count_b: i32,
    delay_b: i32,
    count_c: i32,
    delay_c: i32,
    count_d: i32,
    delay_d: i32,
    count_e: i32,
    delay_e: i32,
    count_f: i32,
    delay_f: i32,
    count_g: i32,
    delay_g: i32,
    count_h: i32,
    delay_h: i32,
    count_i: i32,
    delay_i: i32,
    count_j: i32,
    delay_j: i32,
    count_k: i32,
    delay_k: i32,
    count_l: i32,
    delay_l: i32,
    count_m: i32,
    delay_m: i32,
    feedback_al: f64,
    vib_al: f64,
    depth_a: f64,
    feedback_bl: f64,
    vib_bl: f64,
    depth_b: f64,
    feedback_cl: f64,
    vib_cl: f64,
    depth_c: f64,
    feedback_dl: f64,
    vib_dl: f64,
    depth_d: f64,
    feedback_el: f64,
    vib_el: f64,
    depth_e: f64,
    feedback_fl: f64,
    vib_fl: f64,
    depth_f: f64,
    feedback_gl: f64,
    vib_gl: f64,
    depth_g: f64,
    feedback_hl: f64,
    vib_hl: f64,
    depth_h: f64,
    feedback_ar: f64,
    vib_ar: f64,
    feedback_br: f64,
    vib_br: f64,
    feedback_cr: f64,
    vib_cr: f64,
    feedback_dr: f64,
    vib_dr: f64,
    feedback_er: f64,
    vib_er: f64,
    feedback_fr: f64,
    vib_fr: f64,
    feedback_gr: f64,
    vib_gr: f64,
    feedback_hr: f64,
    vib_hr: f64,
    fpd_l: u32,
    fpd_r: u32,
}

impl MatrixVerb {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<MatrixVerb> {
        let mut s = Box::new(MatrixVerb {
            p: [1.0, 0.0, 0.0, 0.0, 0.5, 0.5, 1.0],
            sr: sr as f32,
            biquad_a: [0.0; 11],
            biquad_b: [0.0; 11],
            biquad_c: [0.0; 11],
            a_al: vec![0.0; 8111],
            a_bl: vec![0.0; 7511],
            a_cl: vec![0.0; 7311],
            a_dl: vec![0.0; 6911],
            a_el: vec![0.0; 6311],
            a_fl: vec![0.0; 6111],
            a_gl: vec![0.0; 5511],
            a_hl: vec![0.0; 4911],
            a_il: vec![0.0; 4511],
            a_jl: vec![0.0; 4311],
            a_kl: vec![0.0; 3911],
            a_ll: vec![0.0; 3311],
            a_ml: vec![0.0; 3111],
            a_ar: vec![0.0; 8111],
            a_br: vec![0.0; 7511],
            a_cr: vec![0.0; 7311],
            a_dr: vec![0.0; 6911],
            a_er: vec![0.0; 6311],
            a_fr: vec![0.0; 6111],
            a_gr: vec![0.0; 5511],
            a_hr: vec![0.0; 4911],
            a_ir: vec![0.0; 4511],
            a_jr: vec![0.0; 4311],
            a_kr: vec![0.0; 3911],
            a_lr: vec![0.0; 3311],
            a_mr: vec![0.0; 3111],
            count_a: 0,
            delay_a: 0,
            count_b: 0,
            delay_b: 0,
            count_c: 0,
            delay_c: 0,
            count_d: 0,
            delay_d: 0,
            count_e: 0,
            delay_e: 0,
            count_f: 0,
            delay_f: 0,
            count_g: 0,
            delay_g: 0,
            count_h: 0,
            delay_h: 0,
            count_i: 0,
            delay_i: 0,
            count_j: 0,
            delay_j: 0,
            count_k: 0,
            delay_k: 0,
            count_l: 0,
            delay_l: 0,
            count_m: 0,
            delay_m: 0,
            feedback_al: 0.0,
            vib_al: 0.0,
            depth_a: 0.0,
            feedback_bl: 0.0,
            vib_bl: 0.0,
            depth_b: 0.0,
            feedback_cl: 0.0,
            vib_cl: 0.0,
            depth_c: 0.0,
            feedback_dl: 0.0,
            vib_dl: 0.0,
            depth_d: 0.0,
            feedback_el: 0.0,
            vib_el: 0.0,
            depth_e: 0.0,
            feedback_fl: 0.0,
            vib_fl: 0.0,
            depth_f: 0.0,
            feedback_gl: 0.0,
            vib_gl: 0.0,
            depth_g: 0.0,
            feedback_hl: 0.0,
            vib_hl: 0.0,
            depth_h: 0.0,
            feedback_ar: 0.0,
            vib_ar: 0.0,
            feedback_br: 0.0,
            vib_br: 0.0,
            feedback_cr: 0.0,
            vib_cr: 0.0,
            feedback_dr: 0.0,
            vib_dr: 0.0,
            feedback_er: 0.0,
            vib_er: 0.0,
            feedback_fr: 0.0,
            vib_fr: 0.0,
            feedback_gr: 0.0,
            vib_gr: 0.0,
            feedback_hr: 0.0,
            vib_hr: 0.0,
            fpd_l: 0,
            fpd_r: 0,
        });
        let mut count: i32 = 0;
        s.count_a = 1;
        s.delay_a = 79;
        s.count_b = 1;
        s.delay_b = 73;
        s.count_c = 1;
        s.delay_c = 71;
        s.count_d = 1;
        s.delay_d = 67;
        s.count_e = 1;
        s.delay_e = 61;
        s.count_f = 1;
        s.delay_f = 59;
        s.count_g = 1;
        s.delay_g = 53;
        s.count_h = 1;
        s.delay_h = 47;
        s.count_i = 1;
        s.delay_i = 43;
        s.count_j = 1;
        s.delay_j = 41;
        s.count_k = 1;
        s.delay_k = 37;
        s.count_l = 1;
        s.delay_l = 31;
        s.count_m = 1;
        s.delay_m = 29;
        s.depth_a = 0.003251;
        s.depth_b = 0.002999;
        s.depth_c = 0.002917;
        s.depth_d = 0.002749;
        s.depth_e = 0.002503;
        s.depth_f = 0.002423;
        s.depth_g = 0.002146;
        s.depth_h = 0.002088;
        s.vib_al = (rng.next() as i32 * -2147483647) as f64;
        s.vib_bl = (rng.next() as i32 * -2147483647) as f64;
        s.vib_cl = (rng.next() as i32 * -2147483647) as f64;
        s.vib_dl = (rng.next() as i32 * -2147483647) as f64;
        s.vib_el = (rng.next() as i32 * -2147483647) as f64;
        s.vib_fl = (rng.next() as i32 * -2147483647) as f64;
        s.vib_gl = (rng.next() as i32 * -2147483647) as f64;
        s.vib_hl = (rng.next() as i32 * -2147483647) as f64;
        s.vib_ar = (rng.next() as i32 * -2147483647) as f64;
        s.vib_br = (rng.next() as i32 * -2147483647) as f64;
        s.vib_cr = (rng.next() as i32 * -2147483647) as f64;
        s.vib_dr = (rng.next() as i32 * -2147483647) as f64;
        s.vib_er = (rng.next() as i32 * -2147483647) as f64;
        s.vib_fr = (rng.next() as i32 * -2147483647) as f64;
        s.vib_gr = (rng.next() as i32 * -2147483647) as f64;
        s.vib_hr = (rng.next() as i32 * -2147483647) as f64;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for MatrixVerb {
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
        s.biquad_a[0] = (s.p[0] as f64 * 9000.0 + 1000.0) / s.sr as f64;
        s.biquad_b[0] = s.biquad_a[0];
        s.biquad_c[0] = s.biquad_b[0];
        s.biquad_a[1] = 1.618033988749894848204586;
        s.biquad_b[1] = 0.618033988749894848204586;
        s.biquad_c[1] = 0.5;
        let mut k: f64 = (PI * s.biquad_a[0]).tan();
        let mut norm: f64 = 1.0 / (1.0 + k / s.biquad_a[1] + k * k);
        s.biquad_a[2] = k * k * norm;
        s.biquad_a[3] = 2.0 * s.biquad_a[2];
        s.biquad_a[4] = s.biquad_a[2];
        s.biquad_a[5] = 2.0 * (k * k - 1.0) * norm;
        s.biquad_a[6] = (1.0 - k / s.biquad_a[1] + k * k) * norm;
        k = (PI * s.biquad_a[0]).tan();
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
        let mut vib_speed: f64 = 0.06 + s.p[2] as f64;
        let mut vib_depth: f64 = (0.027 + (s.p[3] as f64).powf(3.0)) * 100.0;
        let mut size: f64 = (s.p[4] as f64).powf(2.0) * 90.0 + 10.0;
        let mut depth_factor: f64 = 1.0 - (1.0 - (0.82 - (s.p[1] as f64 * 0.5 + size * 0.002))).powf(4.0);
        let mut blend: f64 = 0.955 - size * 0.007;
        let mut crossmod: f64 = (s.p[5] as f64 - 0.5) * 2.0;
        crossmod = crossmod.powf(3.0) * 0.5;
        let mut regen: f64 = depth_factor * (0.5 - crossmod.abs() * 0.031);
        let mut wet: f64 = s.p[6] as f64;
        s.delay_a = (79.0 * size) as i32;
        s.delay_b = (73.0 * size) as i32;
        s.delay_c = (71.0 * size) as i32;
        s.delay_d = (67.0 * size) as i32;
        s.delay_e = (61.0 * size) as i32;
        s.delay_f = (59.0 * size) as i32;
        s.delay_g = (53.0 * size) as i32;
        s.delay_h = (47.0 * size) as i32;
        s.delay_i = (43.0 * size) as i32;
        s.delay_j = (41.0 * size) as i32;
        s.delay_k = (37.0 * size) as i32;
        s.delay_l = (31.0 * size) as i32;
        s.delay_m = (29.0 * size - 56.0 * size * crossmod.abs()) as i32;
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
            s.a_ml[s.count_m as usize] = input_sample_l;
            s.a_mr[s.count_m as usize] = input_sample_r;
            s.count_m += 1;
            if s.count_m < 0 || s.count_m > s.delay_m {
                s.count_m = 0;
            }
            input_sample_l = s.a_ml[s.count_m as usize];
            input_sample_r = s.a_mr[s.count_m as usize];
            let mut temp_sample_l: f64 = input_sample_l * s.biquad_a[2] + s.biquad_a[7];
            s.biquad_a[7] = input_sample_l * s.biquad_a[3] - temp_sample_l * s.biquad_a[5] + s.biquad_a[8];
            s.biquad_a[8] = input_sample_l * s.biquad_a[4] - temp_sample_l * s.biquad_a[6];
            input_sample_l = temp_sample_l;
            let mut temp_sample_r: f64 = input_sample_r * s.biquad_a[2] + s.biquad_a[9];
            s.biquad_a[9] = input_sample_r * s.biquad_a[3] - temp_sample_r * s.biquad_a[5] + s.biquad_a[10];
            s.biquad_a[10] = input_sample_r * s.biquad_a[4] - temp_sample_r * s.biquad_a[6];
            input_sample_r = temp_sample_r;
            input_sample_l *= wet;
            input_sample_r *= wet;
            input_sample_l = input_sample_l.sin();
            input_sample_r = input_sample_r.sin();
            let mut allpass_il: f64 = input_sample_l;
            let mut allpass_jl: f64 = input_sample_l;
            let mut allpass_kl: f64 = input_sample_l;
            let mut allpass_ll: f64 = input_sample_l;
            let mut allpass_ir: f64 = input_sample_r;
            let mut allpass_jr: f64 = input_sample_r;
            let mut allpass_kr: f64 = input_sample_r;
            let mut allpass_lr: f64 = input_sample_r;
            let mut allpasstemp: i32 = s.count_i + 1;
            if allpasstemp < 0 || allpasstemp > s.delay_i {
                allpasstemp = 0;
            }
            allpass_il -= s.a_il[allpasstemp as usize] * 0.5;
            s.a_il[s.count_i as usize] = allpass_il;
            allpass_il *= 0.5;
            allpass_ir -= s.a_ir[allpasstemp as usize] * 0.5;
            s.a_ir[s.count_i as usize] = allpass_ir;
            allpass_ir *= 0.5;
            s.count_i += 1;
            if s.count_i < 0 || s.count_i > s.delay_i {
                s.count_i = 0;
            }
            allpass_il += s.a_il[s.count_i as usize];
            allpass_ir += s.a_ir[s.count_i as usize];
            allpasstemp = s.count_j + 1;
            if allpasstemp < 0 || allpasstemp > s.delay_j {
                allpasstemp = 0;
            }
            allpass_jl -= s.a_jl[allpasstemp as usize] * 0.5;
            s.a_jl[s.count_j as usize] = allpass_jl;
            allpass_jl *= 0.5;
            allpass_jr -= s.a_jr[allpasstemp as usize] * 0.5;
            s.a_jr[s.count_j as usize] = allpass_jr;
            allpass_jr *= 0.5;
            s.count_j += 1;
            if s.count_j < 0 || s.count_j > s.delay_j {
                s.count_j = 0;
            }
            allpass_jl += s.a_jl[s.count_j as usize];
            allpass_jr += s.a_jr[s.count_j as usize];
            allpasstemp = s.count_k + 1;
            if allpasstemp < 0 || allpasstemp > s.delay_k {
                allpasstemp = 0;
            }
            allpass_kl -= s.a_kl[allpasstemp as usize] * 0.5;
            s.a_kl[s.count_k as usize] = allpass_kl;
            allpass_kl *= 0.5;
            allpass_kr -= s.a_kr[allpasstemp as usize] * 0.5;
            s.a_kr[s.count_k as usize] = allpass_kr;
            allpass_kr *= 0.5;
            s.count_k += 1;
            if s.count_k < 0 || s.count_k > s.delay_k {
                s.count_k = 0;
            }
            allpass_kl += s.a_kl[s.count_k as usize];
            allpass_kr += s.a_kr[s.count_k as usize];
            allpasstemp = s.count_l + 1;
            if allpasstemp < 0 || allpasstemp > s.delay_l {
                allpasstemp = 0;
            }
            allpass_ll -= s.a_ll[allpasstemp as usize] * 0.5;
            s.a_ll[s.count_l as usize] = allpass_ll;
            allpass_ll *= 0.5;
            allpass_lr -= s.a_lr[allpasstemp as usize] * 0.5;
            s.a_lr[s.count_l as usize] = allpass_lr;
            allpass_lr *= 0.5;
            s.count_l += 1;
            if s.count_l < 0 || s.count_l > s.delay_l {
                s.count_l = 0;
            }
            allpass_ll += s.a_ll[s.count_l as usize];
            allpass_lr += s.a_lr[s.count_l as usize];
            s.a_al[s.count_a as usize] = allpass_ll + s.feedback_al;
            s.a_bl[s.count_b as usize] = allpass_kl + s.feedback_bl;
            s.a_cl[s.count_c as usize] = allpass_jl + s.feedback_cl;
            s.a_dl[s.count_d as usize] = allpass_il + s.feedback_dl;
            s.a_el[s.count_e as usize] = allpass_il + s.feedback_el;
            s.a_fl[s.count_f as usize] = allpass_jl + s.feedback_fl;
            s.a_gl[s.count_g as usize] = allpass_kl + s.feedback_gl;
            s.a_hl[s.count_h as usize] = allpass_ll + s.feedback_hl;
            s.a_ar[s.count_a as usize] = allpass_lr + s.feedback_ar;
            s.a_br[s.count_b as usize] = allpass_kr + s.feedback_br;
            s.a_cr[s.count_c as usize] = allpass_jr + s.feedback_cr;
            s.a_dr[s.count_d as usize] = allpass_ir + s.feedback_dr;
            s.a_er[s.count_e as usize] = allpass_ir + s.feedback_er;
            s.a_fr[s.count_f as usize] = allpass_jr + s.feedback_fr;
            s.a_gr[s.count_g as usize] = allpass_kr + s.feedback_gr;
            s.a_hr[s.count_h as usize] = allpass_lr + s.feedback_hr;
            s.count_a += 1;
            if s.count_a < 0 || s.count_a > s.delay_a {
                s.count_a = 0;
            }
            s.count_b += 1;
            if s.count_b < 0 || s.count_b > s.delay_b {
                s.count_b = 0;
            }
            s.count_c += 1;
            if s.count_c < 0 || s.count_c > s.delay_c {
                s.count_c = 0;
            }
            s.count_d += 1;
            if s.count_d < 0 || s.count_d > s.delay_d {
                s.count_d = 0;
            }
            s.count_e += 1;
            if s.count_e < 0 || s.count_e > s.delay_e {
                s.count_e = 0;
            }
            s.count_f += 1;
            if s.count_f < 0 || s.count_f > s.delay_f {
                s.count_f = 0;
            }
            s.count_g += 1;
            if s.count_g < 0 || s.count_g > s.delay_g {
                s.count_g = 0;
            }
            s.count_h += 1;
            if s.count_h < 0 || s.count_h > s.delay_h {
                s.count_h = 0;
            }
            s.vib_al += s.depth_a * vib_speed;
            s.vib_bl += s.depth_b * vib_speed;
            s.vib_cl += s.depth_c * vib_speed;
            s.vib_dl += s.depth_d * vib_speed;
            s.vib_el += s.depth_e * vib_speed;
            s.vib_fl += s.depth_f * vib_speed;
            s.vib_gl += s.depth_g * vib_speed;
            s.vib_hl += s.depth_h * vib_speed;
            s.vib_ar += s.depth_a * vib_speed;
            s.vib_br += s.depth_b * vib_speed;
            s.vib_cr += s.depth_c * vib_speed;
            s.vib_dr += s.depth_d * vib_speed;
            s.vib_er += s.depth_e * vib_speed;
            s.vib_fr += s.depth_f * vib_speed;
            s.vib_gr += s.depth_g * vib_speed;
            s.vib_hr += s.depth_h * vib_speed;
            let mut offset_al: f64 = (s.vib_al.sin() + 1.0) * vib_depth;
            let mut offset_bl: f64 = (s.vib_bl.sin() + 1.0) * vib_depth;
            let mut offset_cl: f64 = (s.vib_cl.sin() + 1.0) * vib_depth;
            let mut offset_dl: f64 = (s.vib_dl.sin() + 1.0) * vib_depth;
            let mut offset_el: f64 = (s.vib_el.sin() + 1.0) * vib_depth;
            let mut offset_fl: f64 = (s.vib_fl.sin() + 1.0) * vib_depth;
            let mut offset_gl: f64 = (s.vib_gl.sin() + 1.0) * vib_depth;
            let mut offset_hl: f64 = (s.vib_hl.sin() + 1.0) * vib_depth;
            let mut offset_ar: f64 = (s.vib_ar.sin() + 1.0) * vib_depth;
            let mut offset_br: f64 = (s.vib_br.sin() + 1.0) * vib_depth;
            let mut offset_cr: f64 = (s.vib_cr.sin() + 1.0) * vib_depth;
            let mut offset_dr: f64 = (s.vib_dr.sin() + 1.0) * vib_depth;
            let mut offset_er: f64 = (s.vib_er.sin() + 1.0) * vib_depth;
            let mut offset_fr: f64 = (s.vib_fr.sin() + 1.0) * vib_depth;
            let mut offset_gr: f64 = (s.vib_gr.sin() + 1.0) * vib_depth;
            let mut offset_hr: f64 = (s.vib_hr.sin() + 1.0) * vib_depth;
            let mut working_al: i32 = (s.count_a as f64 + offset_al) as i32;
            let mut working_bl: i32 = (s.count_b as f64 + offset_bl) as i32;
            let mut working_cl: i32 = (s.count_c as f64 + offset_cl) as i32;
            let mut working_dl: i32 = (s.count_d as f64 + offset_dl) as i32;
            let mut working_el: i32 = (s.count_e as f64 + offset_el) as i32;
            let mut working_fl: i32 = (s.count_f as f64 + offset_fl) as i32;
            let mut working_gl: i32 = (s.count_g as f64 + offset_gl) as i32;
            let mut working_hl: i32 = (s.count_h as f64 + offset_hl) as i32;
            let mut working_ar: i32 = (s.count_a as f64 + offset_ar) as i32;
            let mut working_br: i32 = (s.count_b as f64 + offset_br) as i32;
            let mut working_cr: i32 = (s.count_c as f64 + offset_cr) as i32;
            let mut working_dr: i32 = (s.count_d as f64 + offset_dr) as i32;
            let mut working_er: i32 = (s.count_e as f64 + offset_er) as i32;
            let mut working_fr: i32 = (s.count_f as f64 + offset_fr) as i32;
            let mut working_gr: i32 = (s.count_g as f64 + offset_gr) as i32;
            let mut working_hr: i32 = (s.count_h as f64 + offset_hr) as i32;
            let mut interpol_al: f64 = s.a_al[(working_al - (if working_al > s.delay_a { s.delay_a + 1 } else { 0 })) as usize] * (1.0 - (offset_al - offset_al.floor()));
            interpol_al += s.a_al[(working_al + 1 - (if working_al + 1 > s.delay_a { s.delay_a + 1 } else { 0 })) as usize] * (offset_al - offset_al.floor());
            let mut interpol_bl: f64 = s.a_bl[(working_bl - (if working_bl > s.delay_b { s.delay_b + 1 } else { 0 })) as usize] * (1.0 - (offset_bl - offset_bl.floor()));
            interpol_bl += s.a_bl[(working_bl + 1 - (if working_bl + 1 > s.delay_b { s.delay_b + 1 } else { 0 })) as usize] * (offset_bl - offset_bl.floor());
            let mut interpol_cl: f64 = s.a_cl[(working_cl - (if working_cl > s.delay_c { s.delay_c + 1 } else { 0 })) as usize] * (1.0 - (offset_cl - offset_cl.floor()));
            interpol_cl += s.a_cl[(working_cl + 1 - (if working_cl + 1 > s.delay_c { s.delay_c + 1 } else { 0 })) as usize] * (offset_cl - offset_cl.floor());
            let mut interpol_dl: f64 = s.a_dl[(working_dl - (if working_dl > s.delay_d { s.delay_d + 1 } else { 0 })) as usize] * (1.0 - (offset_dl - offset_dl.floor()));
            interpol_dl += s.a_dl[(working_dl + 1 - (if working_dl + 1 > s.delay_d { s.delay_d + 1 } else { 0 })) as usize] * (offset_dl - offset_dl.floor());
            let mut interpol_el: f64 = s.a_el[(working_el - (if working_el > s.delay_e { s.delay_e + 1 } else { 0 })) as usize] * (1.0 - (offset_el - offset_el.floor()));
            interpol_el += s.a_el[(working_el + 1 - (if working_el + 1 > s.delay_e { s.delay_e + 1 } else { 0 })) as usize] * (offset_el - offset_el.floor());
            let mut interpol_fl: f64 = s.a_fl[(working_fl - (if working_fl > s.delay_f { s.delay_f + 1 } else { 0 })) as usize] * (1.0 - (offset_fl - offset_fl.floor()));
            interpol_fl += s.a_fl[(working_fl + 1 - (if working_fl + 1 > s.delay_f { s.delay_f + 1 } else { 0 })) as usize] * (offset_fl - offset_fl.floor());
            let mut interpol_gl: f64 = s.a_gl[(working_gl - (if working_gl > s.delay_g { s.delay_g + 1 } else { 0 })) as usize] * (1.0 - (offset_gl - offset_gl.floor()));
            interpol_gl += s.a_gl[(working_gl + 1 - (if working_gl + 1 > s.delay_g { s.delay_g + 1 } else { 0 })) as usize] * (offset_gl - offset_gl.floor());
            let mut interpol_hl: f64 = s.a_hl[(working_hl - (if working_hl > s.delay_h { s.delay_h + 1 } else { 0 })) as usize] * (1.0 - (offset_hl - offset_hl.floor()));
            interpol_hl += s.a_hl[(working_hl + 1 - (if working_hl + 1 > s.delay_h { s.delay_h + 1 } else { 0 })) as usize] * (offset_hl - offset_hl.floor());
            let mut interpol_ar: f64 = s.a_ar[(working_ar - (if working_ar > s.delay_a { s.delay_a + 1 } else { 0 })) as usize] * (1.0 - (offset_ar - offset_ar.floor()));
            interpol_ar += s.a_ar[(working_ar + 1 - (if working_ar + 1 > s.delay_a { s.delay_a + 1 } else { 0 })) as usize] * (offset_ar - offset_ar.floor());
            let mut interpol_br: f64 = s.a_br[(working_br - (if working_br > s.delay_b { s.delay_b + 1 } else { 0 })) as usize] * (1.0 - (offset_br - offset_br.floor()));
            interpol_br += s.a_br[(working_br + 1 - (if working_br + 1 > s.delay_b { s.delay_b + 1 } else { 0 })) as usize] * (offset_br - offset_br.floor());
            let mut interpol_cr: f64 = s.a_cr[(working_cr - (if working_cr > s.delay_c { s.delay_c + 1 } else { 0 })) as usize] * (1.0 - (offset_cr - offset_cr.floor()));
            interpol_cr += s.a_cr[(working_cr + 1 - (if working_cr + 1 > s.delay_c { s.delay_c + 1 } else { 0 })) as usize] * (offset_cr - offset_cr.floor());
            let mut interpol_dr: f64 = s.a_dr[(working_dr - (if working_dr > s.delay_d { s.delay_d + 1 } else { 0 })) as usize] * (1.0 - (offset_dr - offset_dr.floor()));
            interpol_dr += s.a_dr[(working_dr + 1 - (if working_dr + 1 > s.delay_d { s.delay_d + 1 } else { 0 })) as usize] * (offset_dr - offset_dr.floor());
            let mut interpol_er: f64 = s.a_er[(working_er - (if working_er > s.delay_e { s.delay_e + 1 } else { 0 })) as usize] * (1.0 - (offset_er - offset_er.floor()));
            interpol_er += s.a_er[(working_er + 1 - (if working_er + 1 > s.delay_e { s.delay_e + 1 } else { 0 })) as usize] * (offset_er - offset_er.floor());
            let mut interpol_fr: f64 = s.a_fr[(working_fr - (if working_fr > s.delay_f { s.delay_f + 1 } else { 0 })) as usize] * (1.0 - (offset_fr - offset_fr.floor()));
            interpol_fr += s.a_fr[(working_fr + 1 - (if working_fr + 1 > s.delay_f { s.delay_f + 1 } else { 0 })) as usize] * (offset_fr - offset_fr.floor());
            let mut interpol_gr: f64 = s.a_gr[(working_gr - (if working_gr > s.delay_g { s.delay_g + 1 } else { 0 })) as usize] * (1.0 - (offset_gr - offset_gr.floor()));
            interpol_gr += s.a_gr[(working_gr + 1 - (if working_gr + 1 > s.delay_g { s.delay_g + 1 } else { 0 })) as usize] * (offset_gr - offset_gr.floor());
            let mut interpol_hr: f64 = s.a_hr[(working_hr - (if working_hr > s.delay_h { s.delay_h + 1 } else { 0 })) as usize] * (1.0 - (offset_hr - offset_hr.floor()));
            interpol_hr += s.a_hr[(working_hr + 1 - (if working_hr + 1 > s.delay_h { s.delay_h + 1 } else { 0 })) as usize] * (offset_hr - offset_hr.floor());
            interpol_al = (1.0 - blend) * interpol_al + s.a_al[(working_al - (if working_al > s.delay_a { s.delay_a + 1 } else { 0 })) as usize] * blend;
            interpol_bl = (1.0 - blend) * interpol_bl + s.a_bl[(working_bl - (if working_bl > s.delay_b { s.delay_b + 1 } else { 0 })) as usize] * blend;
            interpol_cl = (1.0 - blend) * interpol_cl + s.a_cl[(working_cl - (if working_cl > s.delay_c { s.delay_c + 1 } else { 0 })) as usize] * blend;
            interpol_dl = (1.0 - blend) * interpol_dl + s.a_dl[(working_dl - (if working_dl > s.delay_d { s.delay_d + 1 } else { 0 })) as usize] * blend;
            interpol_el = (1.0 - blend) * interpol_el + s.a_el[(working_el - (if working_el > s.delay_e { s.delay_e + 1 } else { 0 })) as usize] * blend;
            interpol_fl = (1.0 - blend) * interpol_fl + s.a_fl[(working_fl - (if working_fl > s.delay_f { s.delay_f + 1 } else { 0 })) as usize] * blend;
            interpol_gl = (1.0 - blend) * interpol_gl + s.a_gl[(working_gl - (if working_gl > s.delay_g { s.delay_g + 1 } else { 0 })) as usize] * blend;
            interpol_hl = (1.0 - blend) * interpol_hl + s.a_hl[(working_hl - (if working_hl > s.delay_h { s.delay_h + 1 } else { 0 })) as usize] * blend;
            interpol_ar = (1.0 - blend) * interpol_ar + s.a_ar[(working_ar - (if working_ar > s.delay_a { s.delay_a + 1 } else { 0 })) as usize] * blend;
            interpol_br = (1.0 - blend) * interpol_br + s.a_br[(working_br - (if working_br > s.delay_b { s.delay_b + 1 } else { 0 })) as usize] * blend;
            interpol_cr = (1.0 - blend) * interpol_cr + s.a_cr[(working_cr - (if working_cr > s.delay_c { s.delay_c + 1 } else { 0 })) as usize] * blend;
            interpol_dr = (1.0 - blend) * interpol_dr + s.a_dr[(working_dr - (if working_dr > s.delay_d { s.delay_d + 1 } else { 0 })) as usize] * blend;
            interpol_er = (1.0 - blend) * interpol_er + s.a_er[(working_er - (if working_er > s.delay_e { s.delay_e + 1 } else { 0 })) as usize] * blend;
            interpol_fr = (1.0 - blend) * interpol_fr + s.a_fr[(working_fr - (if working_fr > s.delay_f { s.delay_f + 1 } else { 0 })) as usize] * blend;
            interpol_gr = (1.0 - blend) * interpol_gr + s.a_gr[(working_gr - (if working_gr > s.delay_g { s.delay_g + 1 } else { 0 })) as usize] * blend;
            interpol_hr = (1.0 - blend) * interpol_hr + s.a_hr[(working_hr - (if working_hr > s.delay_h { s.delay_h + 1 } else { 0 })) as usize] * blend;
            interpol_al = interpol_al * (1.0 - crossmod.abs()) + interpol_el * crossmod;
            interpol_el = interpol_el * (1.0 - crossmod.abs()) + interpol_al * crossmod;
            interpol_ar = interpol_ar * (1.0 - crossmod.abs()) + interpol_er * crossmod;
            interpol_er = interpol_er * (1.0 - crossmod.abs()) + interpol_ar * crossmod;
            s.feedback_al = (interpol_al - (interpol_bl + interpol_cl + interpol_dl)) * regen;
            s.feedback_bl = (interpol_bl - (interpol_al + interpol_cl + interpol_dl)) * regen;
            s.feedback_cl = (interpol_cl - (interpol_al + interpol_bl + interpol_dl)) * regen;
            s.feedback_dl = (interpol_dl - (interpol_al + interpol_bl + interpol_cl)) * regen;
            s.feedback_el = (interpol_el - (interpol_fl + interpol_gl + interpol_hl)) * regen;
            s.feedback_fl = (interpol_fl - (interpol_el + interpol_gl + interpol_hl)) * regen;
            s.feedback_gl = (interpol_gl - (interpol_el + interpol_fl + interpol_hl)) * regen;
            s.feedback_hl = (interpol_hl - (interpol_el + interpol_fl + interpol_gl)) * regen;
            s.feedback_ar = (interpol_ar - (interpol_br + interpol_cr + interpol_dr)) * regen;
            s.feedback_br = (interpol_br - (interpol_ar + interpol_cr + interpol_dr)) * regen;
            s.feedback_cr = (interpol_cr - (interpol_ar + interpol_br + interpol_dr)) * regen;
            s.feedback_dr = (interpol_dr - (interpol_ar + interpol_br + interpol_cr)) * regen;
            s.feedback_er = (interpol_er - (interpol_fr + interpol_gr + interpol_hr)) * regen;
            s.feedback_fr = (interpol_fr - (interpol_er + interpol_gr + interpol_hr)) * regen;
            s.feedback_gr = (interpol_gr - (interpol_er + interpol_fr + interpol_hr)) * regen;
            s.feedback_hr = (interpol_hr - (interpol_er + interpol_fr + interpol_gr)) * regen;
            input_sample_l = (interpol_al + interpol_bl + interpol_cl + interpol_dl + interpol_el + interpol_fl + interpol_gl + interpol_hl) / 8.0;
            input_sample_r = (interpol_ar + interpol_br + interpol_cr + interpol_dr + interpol_er + interpol_fr + interpol_gr + interpol_hr) / 8.0;
            temp_sample_l = input_sample_l * s.biquad_b[2] + s.biquad_b[7];
            s.biquad_b[7] = input_sample_l * s.biquad_b[3] - temp_sample_l * s.biquad_b[5] + s.biquad_b[8];
            s.biquad_b[8] = input_sample_l * s.biquad_b[4] - temp_sample_l * s.biquad_b[6];
            input_sample_l = temp_sample_l;
            temp_sample_r = input_sample_r * s.biquad_b[2] + s.biquad_b[9];
            s.biquad_b[9] = input_sample_r * s.biquad_b[3] - temp_sample_r * s.biquad_b[5] + s.biquad_b[10];
            s.biquad_b[10] = input_sample_r * s.biquad_b[4] - temp_sample_r * s.biquad_b[6];
            input_sample_r = temp_sample_r;
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
            input_sample_l = input_sample_l.asin();
            input_sample_r = input_sample_r.asin();
            temp_sample_l = input_sample_l * s.biquad_c[2] + s.biquad_c[7];
            s.biquad_c[7] = input_sample_l * s.biquad_c[3] - temp_sample_l * s.biquad_c[5] + s.biquad_c[8];
            s.biquad_c[8] = input_sample_l * s.biquad_c[4] - temp_sample_l * s.biquad_c[6];
            input_sample_l = temp_sample_l;
            temp_sample_r = input_sample_r * s.biquad_c[2] + s.biquad_c[9];
            s.biquad_c[9] = input_sample_r * s.biquad_c[3] - temp_sample_r * s.biquad_c[5] + s.biquad_c[10];
            s.biquad_c[10] = input_sample_r * s.biquad_c[4] - temp_sample_r * s.biquad_c[6];
            input_sample_r = temp_sample_r;
            if wet != 1.0 {
                input_sample_l += dry_sample_l * (1.0 - wet);
                input_sample_r += dry_sample_r * (1.0 - wet);
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
