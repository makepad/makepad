// Ported from Verbity (VerbityProc.cpp, Verbity.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Verbity (sub-effect id 52): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Bigness", 0.25),
    pf("Longness", 0.0),
    pf("Darkness", 0.25),
    pf("Wetness", 0.25),
];

#[allow(dead_code)]
pub struct Verbity {
    p: [f32; 4],
    sr: f32,
    iir_al: f64,
    iir_bl: f64,
    a_il: Vec<f64>,
    a_jl: Vec<f64>,
    a_kl: Vec<f64>,
    a_ll: Vec<f64>,
    a_al: Vec<f64>,
    a_bl: Vec<f64>,
    a_cl: Vec<f64>,
    a_dl: Vec<f64>,
    a_el: Vec<f64>,
    a_fl: Vec<f64>,
    a_gl: Vec<f64>,
    a_hl: Vec<f64>,
    feedback_al: f64,
    feedback_bl: f64,
    feedback_cl: f64,
    feedback_dl: f64,
    previous_al: f64,
    previous_bl: f64,
    previous_cl: f64,
    previous_dl: f64,
    last_ref_l: [f64; 7],
    thunder_l: f64,
    iir_ar: f64,
    iir_br: f64,
    a_ir: Vec<f64>,
    a_jr: Vec<f64>,
    a_kr: Vec<f64>,
    a_lr: Vec<f64>,
    a_ar: Vec<f64>,
    a_br: Vec<f64>,
    a_cr: Vec<f64>,
    a_dr: Vec<f64>,
    a_er: Vec<f64>,
    a_fr: Vec<f64>,
    a_gr: Vec<f64>,
    a_hr: Vec<f64>,
    feedback_ar: f64,
    feedback_br: f64,
    feedback_cr: f64,
    feedback_dr: f64,
    previous_ar: f64,
    previous_br: f64,
    previous_cr: f64,
    previous_dr: f64,
    last_ref_r: [f64; 7],
    thunder_r: f64,
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
    cycle: i32,
    fpd_l: u32,
    fpd_r: u32,
}

impl Verbity {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Verbity> {
        let mut s = Box::new(Verbity {
            p: [0.25, 0.0, 0.25, 0.25],
            sr: sr as f32,
            iir_al: 0.0,
            iir_bl: 0.0,
            a_il: vec![0.0; 6480],
            a_jl: vec![0.0; 3660],
            a_kl: vec![0.0; 1720],
            a_ll: vec![0.0; 680],
            a_al: vec![0.0; 9700],
            a_bl: vec![0.0; 6000],
            a_cl: vec![0.0; 2320],
            a_dl: vec![0.0; 940],
            a_el: vec![0.0; 15220],
            a_fl: vec![0.0; 8460],
            a_gl: vec![0.0; 4540],
            a_hl: vec![0.0; 3200],
            feedback_al: 0.0,
            feedback_bl: 0.0,
            feedback_cl: 0.0,
            feedback_dl: 0.0,
            previous_al: 0.0,
            previous_bl: 0.0,
            previous_cl: 0.0,
            previous_dl: 0.0,
            last_ref_l: [0.0; 7],
            thunder_l: 0.0,
            iir_ar: 0.0,
            iir_br: 0.0,
            a_ir: vec![0.0; 6480],
            a_jr: vec![0.0; 3660],
            a_kr: vec![0.0; 1720],
            a_lr: vec![0.0; 680],
            a_ar: vec![0.0; 9700],
            a_br: vec![0.0; 6000],
            a_cr: vec![0.0; 2320],
            a_dr: vec![0.0; 940],
            a_er: vec![0.0; 15220],
            a_fr: vec![0.0; 8460],
            a_gr: vec![0.0; 4540],
            a_hr: vec![0.0; 3200],
            feedback_ar: 0.0,
            feedback_br: 0.0,
            feedback_cr: 0.0,
            feedback_dr: 0.0,
            previous_ar: 0.0,
            previous_br: 0.0,
            previous_cr: 0.0,
            previous_dr: 0.0,
            last_ref_r: [0.0; 7],
            thunder_r: 0.0,
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
            cycle: 0,
            fpd_l: 0,
            fpd_r: 0,
        });
        s.count_i = 1;
        s.count_j = 1;
        s.count_k = 1;
        s.count_l = 1;
        s.count_a = 1;
        s.count_b = 1;
        s.count_c = 1;
        s.count_d = 1;
        s.count_e = 1;
        s.count_f = 1;
        s.count_g = 1;
        s.count_h = 1;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for Verbity {
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
        let mut cycle_end: i32 = overallscale.floor() as i32;
        if cycle_end < 1 {
            cycle_end = 1;
        }
        if cycle_end > 4 {
            cycle_end = 4;
        }
        if s.cycle > cycle_end - 1 {
            s.cycle = cycle_end - 1;
        }
        let mut size: f64 = s.p[0] as f64 * 1.77 + 0.1;
        let mut regen: f64 = 0.0625 + s.p[1] as f64 * 0.03125;
        let mut lowpass: f64 = (1.0 - (s.p[2] as f64).powf(2.0)) / overallscale.sqrt();
        let mut interpolate: f64 = (s.p[2] as f64).powf(2.0) * 0.618033988749894848204586;
        let mut thunder_amount: f64 = (0.3 - s.p[1] as f64 * 0.22) * s.p[2] as f64 * 0.1;
        let mut wet: f64 = s.p[3] as f64 * 2.0;
        let mut dry: f64 = 2.0 - wet;
        if wet > 1.0 {
            wet = 1.0;
        }
        if wet < 0.0 {
            wet = 0.0;
        }
        if dry > 1.0 {
            dry = 1.0;
        }
        if dry < 0.0 {
            dry = 0.0;
        }
        s.delay_i = (3407.0 * size) as i32;
        s.delay_j = (1823.0 * size) as i32;
        s.delay_k = (859.0 * size) as i32;
        s.delay_l = (331.0 * size) as i32;
        s.delay_a = (4801.0 * size) as i32;
        s.delay_b = (2909.0 * size) as i32;
        s.delay_c = (1153.0 * size) as i32;
        s.delay_d = (461.0 * size) as i32;
        s.delay_e = (7607.0 * size) as i32;
        s.delay_f = (4217.0 * size) as i32;
        s.delay_g = (2269.0 * size) as i32;
        s.delay_h = (1597.0 * size) as i32;
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
            if s.iir_al.abs() < 1.18e-37 {
                s.iir_al = 0.0;
            }
            s.iir_al = s.iir_al * (1.0 - lowpass) + input_sample_l * lowpass;
            input_sample_l = s.iir_al;
            if s.iir_ar.abs() < 1.18e-37 {
                s.iir_ar = 0.0;
            }
            s.iir_ar = s.iir_ar * (1.0 - lowpass) + input_sample_r * lowpass;
            input_sample_r = s.iir_ar;
            s.cycle += 1;
            if s.cycle == cycle_end {
                s.feedback_al = s.feedback_al * (1.0 - interpolate) + s.previous_al * interpolate;
                s.previous_al = s.feedback_al;
                s.feedback_bl = s.feedback_bl * (1.0 - interpolate) + s.previous_bl * interpolate;
                s.previous_bl = s.feedback_bl;
                s.feedback_cl = s.feedback_cl * (1.0 - interpolate) + s.previous_cl * interpolate;
                s.previous_cl = s.feedback_cl;
                s.feedback_dl = s.feedback_dl * (1.0 - interpolate) + s.previous_dl * interpolate;
                s.previous_dl = s.feedback_dl;
                s.feedback_ar = s.feedback_ar * (1.0 - interpolate) + s.previous_ar * interpolate;
                s.previous_ar = s.feedback_ar;
                s.feedback_br = s.feedback_br * (1.0 - interpolate) + s.previous_br * interpolate;
                s.previous_br = s.feedback_br;
                s.feedback_cr = s.feedback_cr * (1.0 - interpolate) + s.previous_cr * interpolate;
                s.previous_cr = s.feedback_cr;
                s.feedback_dr = s.feedback_dr * (1.0 - interpolate) + s.previous_dr * interpolate;
                s.previous_dr = s.feedback_dr;
                s.thunder_l = s.thunder_l * 0.99 - s.feedback_al * thunder_amount;
                s.thunder_r = s.thunder_r * 0.99 - s.feedback_ar * thunder_amount;
                s.a_il[s.count_i as usize] = input_sample_l + (s.feedback_al + s.thunder_l) * regen;
                s.a_jl[s.count_j as usize] = input_sample_l + s.feedback_bl * regen;
                s.a_kl[s.count_k as usize] = input_sample_l + s.feedback_cl * regen;
                s.a_ll[s.count_l as usize] = input_sample_l + s.feedback_dl * regen;
                s.a_ir[s.count_i as usize] = input_sample_r + (s.feedback_ar + s.thunder_r) * regen;
                s.a_jr[s.count_j as usize] = input_sample_r + s.feedback_br * regen;
                s.a_kr[s.count_k as usize] = input_sample_r + s.feedback_cr * regen;
                s.a_lr[s.count_l as usize] = input_sample_r + s.feedback_dr * regen;
                s.count_i += 1;
                if s.count_i < 0 || s.count_i > s.delay_i {
                    s.count_i = 0;
                }
                s.count_j += 1;
                if s.count_j < 0 || s.count_j > s.delay_j {
                    s.count_j = 0;
                }
                s.count_k += 1;
                if s.count_k < 0 || s.count_k > s.delay_k {
                    s.count_k = 0;
                }
                s.count_l += 1;
                if s.count_l < 0 || s.count_l > s.delay_l {
                    s.count_l = 0;
                }
                let mut out_il: f64 = s.a_il[(s.count_i - (if s.count_i > s.delay_i { s.delay_i + 1 } else { 0 })) as usize];
                let mut out_jl: f64 = s.a_jl[(s.count_j - (if s.count_j > s.delay_j { s.delay_j + 1 } else { 0 })) as usize];
                let mut out_kl: f64 = s.a_kl[(s.count_k - (if s.count_k > s.delay_k { s.delay_k + 1 } else { 0 })) as usize];
                let mut out_ll: f64 = s.a_ll[(s.count_l - (if s.count_l > s.delay_l { s.delay_l + 1 } else { 0 })) as usize];
                let mut out_ir: f64 = s.a_ir[(s.count_i - (if s.count_i > s.delay_i { s.delay_i + 1 } else { 0 })) as usize];
                let mut out_jr: f64 = s.a_jr[(s.count_j - (if s.count_j > s.delay_j { s.delay_j + 1 } else { 0 })) as usize];
                let mut out_kr: f64 = s.a_kr[(s.count_k - (if s.count_k > s.delay_k { s.delay_k + 1 } else { 0 })) as usize];
                let mut out_lr: f64 = s.a_lr[(s.count_l - (if s.count_l > s.delay_l { s.delay_l + 1 } else { 0 })) as usize];
                s.a_al[s.count_a as usize] = out_il - (out_jl + out_kl + out_ll);
                s.a_bl[s.count_b as usize] = out_jl - (out_il + out_kl + out_ll);
                s.a_cl[s.count_c as usize] = out_kl - (out_il + out_jl + out_ll);
                s.a_dl[s.count_d as usize] = out_ll - (out_il + out_jl + out_kl);
                s.a_ar[s.count_a as usize] = out_ir - (out_jr + out_kr + out_lr);
                s.a_br[s.count_b as usize] = out_jr - (out_ir + out_kr + out_lr);
                s.a_cr[s.count_c as usize] = out_kr - (out_ir + out_jr + out_lr);
                s.a_dr[s.count_d as usize] = out_lr - (out_ir + out_jr + out_kr);
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
                let mut out_al: f64 = s.a_al[(s.count_a - (if s.count_a > s.delay_a { s.delay_a + 1 } else { 0 })) as usize];
                let mut out_bl: f64 = s.a_bl[(s.count_b - (if s.count_b > s.delay_b { s.delay_b + 1 } else { 0 })) as usize];
                let mut out_cl: f64 = s.a_cl[(s.count_c - (if s.count_c > s.delay_c { s.delay_c + 1 } else { 0 })) as usize];
                let mut out_dl: f64 = s.a_dl[(s.count_d - (if s.count_d > s.delay_d { s.delay_d + 1 } else { 0 })) as usize];
                let mut out_ar: f64 = s.a_ar[(s.count_a - (if s.count_a > s.delay_a { s.delay_a + 1 } else { 0 })) as usize];
                let mut out_br: f64 = s.a_br[(s.count_b - (if s.count_b > s.delay_b { s.delay_b + 1 } else { 0 })) as usize];
                let mut out_cr: f64 = s.a_cr[(s.count_c - (if s.count_c > s.delay_c { s.delay_c + 1 } else { 0 })) as usize];
                let mut out_dr: f64 = s.a_dr[(s.count_d - (if s.count_d > s.delay_d { s.delay_d + 1 } else { 0 })) as usize];
                s.a_el[s.count_e as usize] = out_al - (out_bl + out_cl + out_dl);
                s.a_fl[s.count_f as usize] = out_bl - (out_al + out_cl + out_dl);
                s.a_gl[s.count_g as usize] = out_cl - (out_al + out_bl + out_dl);
                s.a_hl[s.count_h as usize] = out_dl - (out_al + out_bl + out_cl);
                s.a_er[s.count_e as usize] = out_ar - (out_br + out_cr + out_dr);
                s.a_fr[s.count_f as usize] = out_br - (out_ar + out_cr + out_dr);
                s.a_gr[s.count_g as usize] = out_cr - (out_ar + out_br + out_dr);
                s.a_hr[s.count_h as usize] = out_dr - (out_ar + out_br + out_cr);
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
                let mut out_el: f64 = s.a_el[(s.count_e - (if s.count_e > s.delay_e { s.delay_e + 1 } else { 0 })) as usize];
                let mut out_fl: f64 = s.a_fl[(s.count_f - (if s.count_f > s.delay_f { s.delay_f + 1 } else { 0 })) as usize];
                let mut out_gl: f64 = s.a_gl[(s.count_g - (if s.count_g > s.delay_g { s.delay_g + 1 } else { 0 })) as usize];
                let mut out_hl: f64 = s.a_hl[(s.count_h - (if s.count_h > s.delay_h { s.delay_h + 1 } else { 0 })) as usize];
                let mut out_er: f64 = s.a_er[(s.count_e - (if s.count_e > s.delay_e { s.delay_e + 1 } else { 0 })) as usize];
                let mut out_fr: f64 = s.a_fr[(s.count_f - (if s.count_f > s.delay_f { s.delay_f + 1 } else { 0 })) as usize];
                let mut out_gr: f64 = s.a_gr[(s.count_g - (if s.count_g > s.delay_g { s.delay_g + 1 } else { 0 })) as usize];
                let mut out_hr: f64 = s.a_hr[(s.count_h - (if s.count_h > s.delay_h { s.delay_h + 1 } else { 0 })) as usize];
                s.feedback_al = out_el - (out_fl + out_gl + out_hl);
                s.feedback_bl = out_fl - (out_el + out_gl + out_hl);
                s.feedback_cl = out_gl - (out_el + out_fl + out_hl);
                s.feedback_dl = out_hl - (out_el + out_fl + out_gl);
                s.feedback_ar = out_er - (out_fr + out_gr + out_hr);
                s.feedback_br = out_fr - (out_er + out_gr + out_hr);
                s.feedback_cr = out_gr - (out_er + out_fr + out_hr);
                s.feedback_dr = out_hr - (out_er + out_fr + out_gr);
                input_sample_l = (out_el + out_fl + out_gl + out_hl) / 8.0;
                input_sample_r = (out_er + out_fr + out_gr + out_hr) / 8.0;
                if cycle_end == 4 {
                    s.last_ref_l[0] = s.last_ref_l[4];
                    s.last_ref_l[2] = (s.last_ref_l[0] + input_sample_l) / 2.0;
                    s.last_ref_l[1] = (s.last_ref_l[0] + s.last_ref_l[2]) / 2.0;
                    s.last_ref_l[3] = (s.last_ref_l[2] + input_sample_l) / 2.0;
                    s.last_ref_l[4] = input_sample_l;
                    s.last_ref_r[0] = s.last_ref_r[4];
                    s.last_ref_r[2] = (s.last_ref_r[0] + input_sample_r) / 2.0;
                    s.last_ref_r[1] = (s.last_ref_r[0] + s.last_ref_r[2]) / 2.0;
                    s.last_ref_r[3] = (s.last_ref_r[2] + input_sample_r) / 2.0;
                    s.last_ref_r[4] = input_sample_r;
                }
                if cycle_end == 3 {
                    s.last_ref_l[0] = s.last_ref_l[3];
                    s.last_ref_l[2] = (s.last_ref_l[0] + s.last_ref_l[0] + input_sample_l) / 3.0;
                    s.last_ref_l[1] = (s.last_ref_l[0] + input_sample_l + input_sample_l) / 3.0;
                    s.last_ref_l[3] = input_sample_l;
                    s.last_ref_r[0] = s.last_ref_r[3];
                    s.last_ref_r[2] = (s.last_ref_r[0] + s.last_ref_r[0] + input_sample_r) / 3.0;
                    s.last_ref_r[1] = (s.last_ref_r[0] + input_sample_r + input_sample_r) / 3.0;
                    s.last_ref_r[3] = input_sample_r;
                }
                if cycle_end == 2 {
                    s.last_ref_l[0] = s.last_ref_l[2];
                    s.last_ref_l[1] = (s.last_ref_l[0] + input_sample_l) / 2.0;
                    s.last_ref_l[2] = input_sample_l;
                    s.last_ref_r[0] = s.last_ref_r[2];
                    s.last_ref_r[1] = (s.last_ref_r[0] + input_sample_r) / 2.0;
                    s.last_ref_r[2] = input_sample_r;
                }
                if cycle_end == 1 {
                    s.last_ref_l[0] = input_sample_l;
                    s.last_ref_r[0] = input_sample_r;
                }
                s.cycle = 0;
                input_sample_l = s.last_ref_l[s.cycle as usize];
                input_sample_r = s.last_ref_r[s.cycle as usize];
            } else {
                input_sample_l = s.last_ref_l[s.cycle as usize];
                input_sample_r = s.last_ref_r[s.cycle as usize];
            }
            if s.iir_bl.abs() < 1.18e-37 {
                s.iir_bl = 0.0;
            }
            s.iir_bl = s.iir_bl * (1.0 - lowpass) + input_sample_l * lowpass;
            input_sample_l = s.iir_bl;
            if s.iir_br.abs() < 1.18e-37 {
                s.iir_br = 0.0;
            }
            s.iir_br = s.iir_br * (1.0 - lowpass) + input_sample_r * lowpass;
            input_sample_r = s.iir_br;
            if wet < 1.0 {
                input_sample_l *= wet;
                input_sample_r *= wet;
            }
            if dry < 1.0 {
                dry_sample_l *= dry;
                dry_sample_r *= dry;
            }
            input_sample_l += dry_sample_l;
            input_sample_r += dry_sample_r;
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
