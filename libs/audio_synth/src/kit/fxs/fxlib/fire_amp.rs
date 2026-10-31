// Ported from FireAmp (FireAmpProc.cpp, FireAmp.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Fire Amp (sub-effect id 62): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;
#[allow(unused_imports)]
use std::f64::consts::{PI};

const FIX_FREQ: usize = 0;
const FIX_RESO: usize = 1;
const FIX_A0: usize = 2;
const FIX_A1: usize = 3;
const FIX_A2: usize = 4;
const FIX_B1: usize = 5;
const FIX_B2: usize = 6;
const FIX_S_L1: usize = 7;
const FIX_S_L2: usize = 8;
const FIX_S_R1: usize = 9;
const FIX_S_R2: usize = 10;

pub const PARAMS: &[SlotDef] = &[
    pf("Gain", 0.5),
    pf("Tone", 0.5),
    pf("Output", 0.8),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct FireAmp {
    p: [f32; 4],
    sr: f32,
    last_sample_l: f64,
    store_sample_l: f64,
    smooth_al: f64,
    smooth_bl: f64,
    smooth_cl: f64,
    smooth_dl: f64,
    smooth_el: f64,
    smooth_fl: f64,
    smooth_gl: f64,
    smooth_hl: f64,
    smooth_il: f64,
    smooth_jl: f64,
    smooth_kl: f64,
    smooth_ll: f64,
    iir_sample_al: f64,
    iir_sample_bl: f64,
    iir_sample_cl: f64,
    iir_sample_dl: f64,
    iir_sample_el: f64,
    iir_sample_fl: f64,
    iir_sample_gl: f64,
    iir_sample_hl: f64,
    iir_sample_il: f64,
    iir_sample_jl: f64,
    iir_sample_kl: f64,
    iir_sample_ll: f64,
    iir_lowpass_l: f64,
    iir_spk_al: f64,
    iir_spk_bl: f64,
    iir_sub_l: f64,
    odd_l: Vec<f64>,
    even_l: Vec<f64>,
    last_sample_r: f64,
    store_sample_r: f64,
    smooth_ar: f64,
    smooth_br: f64,
    smooth_cr: f64,
    smooth_dr: f64,
    smooth_er: f64,
    smooth_fr: f64,
    smooth_gr: f64,
    smooth_hr: f64,
    smooth_ir: f64,
    smooth_jr: f64,
    smooth_kr: f64,
    smooth_lr: f64,
    iir_sample_ar: f64,
    iir_sample_br: f64,
    iir_sample_cr: f64,
    iir_sample_dr: f64,
    iir_sample_er: f64,
    iir_sample_fr: f64,
    iir_sample_gr: f64,
    iir_sample_hr: f64,
    iir_sample_ir: f64,
    iir_sample_jr: f64,
    iir_sample_kr: f64,
    iir_sample_lr: f64,
    iir_lowpass_r: f64,
    iir_spk_ar: f64,
    iir_spk_br: f64,
    iir_sub_r: f64,
    odd_r: Vec<f64>,
    even_r: Vec<f64>,
    flip: bool,
    count: i32,
    b_l: Vec<f64>,
    last_cab_sample_l: f64,
    smooth_cab_al: f64,
    smooth_cab_bl: f64,
    b_r: Vec<f64>,
    last_cab_sample_r: f64,
    smooth_cab_ar: f64,
    smooth_cab_br: f64,
    last_ref_l: [f64; 10],
    last_ref_r: [f64; 10],
    cycle: i32,
    fix_a: [f64; 11],
    fix_b: [f64; 11],
    fix_c: [f64; 11],
    fix_d: [f64; 11],
    fix_e: [f64; 11],
    fix_f: [f64; 11],
    fpd_l: u32,
    fpd_r: u32,
}

impl FireAmp {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<FireAmp> {
        let mut s = Box::new(FireAmp {
            p: [0.5, 0.5, 0.8, 1.0],
            sr: sr as f32,
            last_sample_l: 0.0,
            store_sample_l: 0.0,
            smooth_al: 0.0,
            smooth_bl: 0.0,
            smooth_cl: 0.0,
            smooth_dl: 0.0,
            smooth_el: 0.0,
            smooth_fl: 0.0,
            smooth_gl: 0.0,
            smooth_hl: 0.0,
            smooth_il: 0.0,
            smooth_jl: 0.0,
            smooth_kl: 0.0,
            smooth_ll: 0.0,
            iir_sample_al: 0.0,
            iir_sample_bl: 0.0,
            iir_sample_cl: 0.0,
            iir_sample_dl: 0.0,
            iir_sample_el: 0.0,
            iir_sample_fl: 0.0,
            iir_sample_gl: 0.0,
            iir_sample_hl: 0.0,
            iir_sample_il: 0.0,
            iir_sample_jl: 0.0,
            iir_sample_kl: 0.0,
            iir_sample_ll: 0.0,
            iir_lowpass_l: 0.0,
            iir_spk_al: 0.0,
            iir_spk_bl: 0.0,
            iir_sub_l: 0.0,
            odd_l: vec![0.0; 257],
            even_l: vec![0.0; 257],
            last_sample_r: 0.0,
            store_sample_r: 0.0,
            smooth_ar: 0.0,
            smooth_br: 0.0,
            smooth_cr: 0.0,
            smooth_dr: 0.0,
            smooth_er: 0.0,
            smooth_fr: 0.0,
            smooth_gr: 0.0,
            smooth_hr: 0.0,
            smooth_ir: 0.0,
            smooth_jr: 0.0,
            smooth_kr: 0.0,
            smooth_lr: 0.0,
            iir_sample_ar: 0.0,
            iir_sample_br: 0.0,
            iir_sample_cr: 0.0,
            iir_sample_dr: 0.0,
            iir_sample_er: 0.0,
            iir_sample_fr: 0.0,
            iir_sample_gr: 0.0,
            iir_sample_hr: 0.0,
            iir_sample_ir: 0.0,
            iir_sample_jr: 0.0,
            iir_sample_kr: 0.0,
            iir_sample_lr: 0.0,
            iir_lowpass_r: 0.0,
            iir_spk_ar: 0.0,
            iir_spk_br: 0.0,
            iir_sub_r: 0.0,
            odd_r: vec![0.0; 257],
            even_r: vec![0.0; 257],
            flip: false,
            count: 0,
            b_l: vec![0.0; 90],
            last_cab_sample_l: 0.0,
            smooth_cab_al: 0.0,
            smooth_cab_bl: 0.0,
            b_r: vec![0.0; 90],
            last_cab_sample_r: 0.0,
            smooth_cab_ar: 0.0,
            smooth_cab_br: 0.0,
            last_ref_l: [0.0; 10],
            last_ref_r: [0.0; 10],
            cycle: 0,
            fix_a: [0.0; 11],
            fix_b: [0.0; 11],
            fix_c: [0.0; 11],
            fix_d: [0.0; 11],
            fix_e: [0.0; 11],
            fix_f: [0.0; 11],
            fpd_l: 0,
            fpd_r: 0,
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for FireAmp {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut bassfill: f64 = s.p[0] as f64;
        let mut outputlevel: f64 = s.p[2] as f64;
        let mut wet: f64 = s.p[3] as f64;
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
        let mut startlevel: f64 = bassfill;
        let mut samplerate: f64 = s.sr as f64;
        let mut basstrim: f64 = bassfill / 16.0;
        let mut tone_eq: f64 = s.p[1] as f64 / samplerate * 22050.0;
        let mut eq: f64 = basstrim / samplerate * 22050.0;
        let mut bleed: f64 = outputlevel / 16.0;
        let mut bassfactor: f64 = 1.0 - basstrim * basstrim;
        let mut beq: f64 = bleed / samplerate * 22050.0;
        let mut diagonal: i32 = (0.000861678 * samplerate) as i32;
        if diagonal > 127 {
            diagonal = 127;
        }
        let mut side: i32 = (diagonal as f64 / 1.4142135623730951) as i32;
        let mut down: i32 = (side + diagonal) / 2;
        let mut cutoff: f64 = (15000.0 + s.p[1] as f64 * 10000.0) / s.sr as f64;
        if cutoff > 0.49 {
            cutoff = 0.49;
        }
        if cutoff < 0.001 {
            cutoff = 0.001;
        }
        s.fix_a[FIX_FREQ] = cutoff;
        s.fix_b[FIX_FREQ] = s.fix_a[FIX_FREQ];
        s.fix_c[FIX_FREQ] = s.fix_b[FIX_FREQ];
        s.fix_d[FIX_FREQ] = s.fix_c[FIX_FREQ];
        s.fix_e[FIX_FREQ] = s.fix_d[FIX_FREQ];
        s.fix_f[FIX_FREQ] = s.fix_e[FIX_FREQ];
        s.fix_a[FIX_RESO] = 4.46570214;
        s.fix_b[FIX_RESO] = 1.51387132;
        s.fix_c[FIX_RESO] = 0.93979296;
        s.fix_d[FIX_RESO] = 0.70710678;
        s.fix_e[FIX_RESO] = 0.52972649;
        s.fix_f[FIX_RESO] = 0.50316379;
        let mut k: f64 = (PI * s.fix_a[FIX_FREQ]).tan();
        let mut norm: f64 = 1.0 / (1.0 + k / s.fix_a[FIX_RESO] + k * k);
        s.fix_a[FIX_A0] = k * k * norm;
        s.fix_a[FIX_A1] = 2.0 * s.fix_a[FIX_A0];
        s.fix_a[FIX_A2] = s.fix_a[FIX_A0];
        s.fix_a[FIX_B1] = 2.0 * (k * k - 1.0) * norm;
        s.fix_a[FIX_B2] = (1.0 - k / s.fix_a[FIX_RESO] + k * k) * norm;
        k = (PI * s.fix_b[FIX_FREQ]).tan();
        norm = 1.0 / (1.0 + k / s.fix_b[FIX_RESO] + k * k);
        s.fix_b[FIX_A0] = k * k * norm;
        s.fix_b[FIX_A1] = 2.0 * s.fix_b[FIX_A0];
        s.fix_b[FIX_A2] = s.fix_b[FIX_A0];
        s.fix_b[FIX_B1] = 2.0 * (k * k - 1.0) * norm;
        s.fix_b[FIX_B2] = (1.0 - k / s.fix_b[FIX_RESO] + k * k) * norm;
        k = (PI * s.fix_c[FIX_FREQ]).tan();
        norm = 1.0 / (1.0 + k / s.fix_c[FIX_RESO] + k * k);
        s.fix_c[FIX_A0] = k * k * norm;
        s.fix_c[FIX_A1] = 2.0 * s.fix_c[FIX_A0];
        s.fix_c[FIX_A2] = s.fix_c[FIX_A0];
        s.fix_c[FIX_B1] = 2.0 * (k * k - 1.0) * norm;
        s.fix_c[FIX_B2] = (1.0 - k / s.fix_c[FIX_RESO] + k * k) * norm;
        k = (PI * s.fix_d[FIX_FREQ]).tan();
        norm = 1.0 / (1.0 + k / s.fix_d[FIX_RESO] + k * k);
        s.fix_d[FIX_A0] = k * k * norm;
        s.fix_d[FIX_A1] = 2.0 * s.fix_d[FIX_A0];
        s.fix_d[FIX_A2] = s.fix_d[FIX_A0];
        s.fix_d[FIX_B1] = 2.0 * (k * k - 1.0) * norm;
        s.fix_d[FIX_B2] = (1.0 - k / s.fix_d[FIX_RESO] + k * k) * norm;
        k = (PI * s.fix_e[FIX_FREQ]).tan();
        norm = 1.0 / (1.0 + k / s.fix_e[FIX_RESO] + k * k);
        s.fix_e[FIX_A0] = k * k * norm;
        s.fix_e[FIX_A1] = 2.0 * s.fix_e[FIX_A0];
        s.fix_e[FIX_A2] = s.fix_e[FIX_A0];
        s.fix_e[FIX_B1] = 2.0 * (k * k - 1.0) * norm;
        s.fix_e[FIX_B2] = (1.0 - k / s.fix_e[FIX_RESO] + k * k) * norm;
        k = (PI * s.fix_f[FIX_FREQ]).tan();
        norm = 1.0 / (1.0 + k / s.fix_f[FIX_RESO] + k * k);
        s.fix_f[FIX_A0] = k * k * norm;
        s.fix_f[FIX_A1] = 2.0 * s.fix_f[FIX_A0];
        s.fix_f[FIX_A2] = s.fix_f[FIX_A0];
        s.fix_f[FIX_B1] = 2.0 * (k * k - 1.0) * norm;
        s.fix_f[FIX_B2] = (1.0 - k / s.fix_f[FIX_RESO] + k * k) * norm;
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
            let mut out_sample: f64 = input_sample_l * s.fix_a[FIX_A0] + s.fix_a[FIX_S_L1];
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
            let mut basscut_l: f64 = 0.98;
            let mut inputlevel_l: f64 = startlevel;
            input_sample_l *= inputlevel_l;
            inputlevel_l = (inputlevel_l * 7.0 + 1.0) / 8.0;
            s.iir_sample_al = s.iir_sample_al * (1.0 - eq) + input_sample_l * eq;
            basscut_l *= bassfactor;
            input_sample_l = input_sample_l - s.iir_sample_al * basscut_l;
            input_sample_l -= input_sample_l * (input_sample_l.abs() * 0.654) * (input_sample_l.abs() * 0.654);
            let mut bridgerectifier: f64 = s.smooth_al + input_sample_l;
            s.smooth_al = input_sample_l;
            input_sample_l = bridgerectifier;
            input_sample_l *= inputlevel_l;
            inputlevel_l = (inputlevel_l * 7.0 + 1.0) / 8.0;
            s.iir_sample_bl = s.iir_sample_bl * (1.0 - eq) + input_sample_l * eq;
            basscut_l *= bassfactor;
            input_sample_l = input_sample_l - s.iir_sample_bl * basscut_l;
            input_sample_l -= input_sample_l * (input_sample_l.abs() * 0.654) * (input_sample_l.abs() * 0.654);
            bridgerectifier = s.smooth_bl + input_sample_l;
            s.smooth_bl = input_sample_l;
            input_sample_l = bridgerectifier;
            if input_sample_r > 1.0 {
                input_sample_r = 1.0;
            }
            if input_sample_r < -1.0 {
                input_sample_r = -1.0;
            }
            let mut basscut_r: f64 = 0.98;
            let mut inputlevel_r: f64 = startlevel;
            input_sample_r *= inputlevel_r;
            inputlevel_r = (inputlevel_r * 7.0 + 1.0) / 8.0;
            s.iir_sample_ar = s.iir_sample_ar * (1.0 - eq) + input_sample_r * eq;
            basscut_r *= bassfactor;
            input_sample_r = input_sample_r - s.iir_sample_ar * basscut_r;
            input_sample_r -= input_sample_r * (input_sample_r.abs() * 0.654) * (input_sample_r.abs() * 0.654);
            bridgerectifier = s.smooth_ar + input_sample_r;
            s.smooth_ar = input_sample_r;
            input_sample_r = bridgerectifier;
            input_sample_r *= inputlevel_r;
            inputlevel_r = (inputlevel_r * 7.0 + 1.0) / 8.0;
            s.iir_sample_br = s.iir_sample_br * (1.0 - eq) + input_sample_r * eq;
            basscut_r *= bassfactor;
            input_sample_r = input_sample_r - s.iir_sample_br * basscut_r;
            input_sample_r -= input_sample_r * (input_sample_r.abs() * 0.654) * (input_sample_r.abs() * 0.654);
            bridgerectifier = s.smooth_br + input_sample_r;
            s.smooth_br = input_sample_r;
            input_sample_r = bridgerectifier;
            out_sample = input_sample_l * s.fix_b[FIX_A0] + s.fix_b[FIX_S_L1];
            s.fix_b[FIX_S_L1] = input_sample_l * s.fix_b[FIX_A1] - out_sample * s.fix_b[FIX_B1] + s.fix_b[FIX_S_L2];
            s.fix_b[FIX_S_L2] = input_sample_l * s.fix_b[FIX_A2] - out_sample * s.fix_b[FIX_B2];
            input_sample_l = out_sample;
            out_sample = input_sample_r * s.fix_b[FIX_A0] + s.fix_b[FIX_S_R1];
            s.fix_b[FIX_S_R1] = input_sample_r * s.fix_b[FIX_A1] - out_sample * s.fix_b[FIX_B1] + s.fix_b[FIX_S_R2];
            s.fix_b[FIX_S_R2] = input_sample_r * s.fix_b[FIX_A2] - out_sample * s.fix_b[FIX_B2];
            input_sample_r = out_sample;
            input_sample_l *= inputlevel_l;
            inputlevel_l = (inputlevel_l * 7.0 + 1.0) / 8.0;
            s.iir_sample_cl = s.iir_sample_cl * (1.0 - eq) + input_sample_l * eq;
            basscut_l *= bassfactor;
            input_sample_l = input_sample_l - s.iir_sample_cl * basscut_l;
            input_sample_l -= input_sample_l * (input_sample_l.abs() * 0.654) * (input_sample_l.abs() * 0.654);
            bridgerectifier = s.smooth_cl + input_sample_l;
            s.smooth_cl = input_sample_l;
            input_sample_l = bridgerectifier;
            input_sample_l *= inputlevel_l;
            inputlevel_l = (inputlevel_l * 7.0 + 1.0) / 8.0;
            s.iir_sample_dl = s.iir_sample_dl * (1.0 - eq) + input_sample_l * eq;
            basscut_l *= bassfactor;
            input_sample_l = input_sample_l - s.iir_sample_dl * basscut_l;
            input_sample_l -= input_sample_l * (input_sample_l.abs() * 0.654) * (input_sample_l.abs() * 0.654);
            bridgerectifier = s.smooth_dl + input_sample_l;
            s.smooth_dl = input_sample_l;
            input_sample_l = bridgerectifier;
            input_sample_r *= inputlevel_r;
            inputlevel_r = (inputlevel_r * 7.0 + 1.0) / 8.0;
            s.iir_sample_cr = s.iir_sample_cr * (1.0 - eq) + input_sample_r * eq;
            basscut_r *= bassfactor;
            input_sample_r = input_sample_r - s.iir_sample_cr * basscut_r;
            input_sample_r -= input_sample_r * (input_sample_r.abs() * 0.654) * (input_sample_r.abs() * 0.654);
            bridgerectifier = s.smooth_cr + input_sample_r;
            s.smooth_cr = input_sample_r;
            input_sample_r = bridgerectifier;
            input_sample_r *= inputlevel_r;
            inputlevel_r = (inputlevel_r * 7.0 + 1.0) / 8.0;
            s.iir_sample_dr = s.iir_sample_dr * (1.0 - eq) + input_sample_r * eq;
            basscut_r *= bassfactor;
            input_sample_r = input_sample_r - s.iir_sample_dr * basscut_r;
            input_sample_r -= input_sample_r * (input_sample_r.abs() * 0.654) * (input_sample_r.abs() * 0.654);
            bridgerectifier = s.smooth_dr + input_sample_r;
            s.smooth_dr = input_sample_r;
            input_sample_r = bridgerectifier;
            out_sample = input_sample_l * s.fix_c[FIX_A0] + s.fix_c[FIX_S_L1];
            s.fix_c[FIX_S_L1] = input_sample_l * s.fix_c[FIX_A1] - out_sample * s.fix_c[FIX_B1] + s.fix_c[FIX_S_L2];
            s.fix_c[FIX_S_L2] = input_sample_l * s.fix_c[FIX_A2] - out_sample * s.fix_c[FIX_B2];
            input_sample_l = out_sample;
            out_sample = input_sample_r * s.fix_c[FIX_A0] + s.fix_c[FIX_S_R1];
            s.fix_c[FIX_S_R1] = input_sample_r * s.fix_c[FIX_A1] - out_sample * s.fix_c[FIX_B1] + s.fix_c[FIX_S_R2];
            s.fix_c[FIX_S_R2] = input_sample_r * s.fix_c[FIX_A2] - out_sample * s.fix_c[FIX_B2];
            input_sample_r = out_sample;
            input_sample_l *= inputlevel_l;
            inputlevel_l = (inputlevel_l * 7.0 + 1.0) / 8.0;
            s.iir_sample_el = s.iir_sample_el * (1.0 - eq) + input_sample_l * eq;
            basscut_l *= bassfactor;
            input_sample_l = input_sample_l - s.iir_sample_el * basscut_l;
            input_sample_l -= input_sample_l * (input_sample_l.abs() * 0.654) * (input_sample_l.abs() * 0.654);
            bridgerectifier = s.smooth_el + input_sample_l;
            s.smooth_el = input_sample_l;
            input_sample_l = bridgerectifier;
            input_sample_l *= inputlevel_l;
            inputlevel_l = (inputlevel_l * 7.0 + 1.0) / 8.0;
            s.iir_sample_fl = s.iir_sample_fl * (1.0 - eq) + input_sample_l * eq;
            basscut_l *= bassfactor;
            input_sample_l = input_sample_l - s.iir_sample_fl * basscut_l;
            input_sample_l -= input_sample_l * (input_sample_l.abs() * 0.654) * (input_sample_l.abs() * 0.654);
            bridgerectifier = s.smooth_fl + input_sample_l;
            s.smooth_fl = input_sample_l;
            input_sample_l = bridgerectifier;
            input_sample_r *= inputlevel_r;
            inputlevel_r = (inputlevel_r * 7.0 + 1.0) / 8.0;
            s.iir_sample_er = s.iir_sample_er * (1.0 - eq) + input_sample_r * eq;
            basscut_r *= bassfactor;
            input_sample_r = input_sample_r - s.iir_sample_er * basscut_r;
            input_sample_r -= input_sample_r * (input_sample_r.abs() * 0.654) * (input_sample_r.abs() * 0.654);
            bridgerectifier = s.smooth_er + input_sample_r;
            s.smooth_er = input_sample_r;
            input_sample_r = bridgerectifier;
            input_sample_r *= inputlevel_r;
            inputlevel_r = (inputlevel_r * 7.0 + 1.0) / 8.0;
            s.iir_sample_fr = s.iir_sample_fr * (1.0 - eq) + input_sample_r * eq;
            basscut_r *= bassfactor;
            input_sample_r = input_sample_r - s.iir_sample_fr * basscut_r;
            input_sample_r -= input_sample_r * (input_sample_r.abs() * 0.654) * (input_sample_r.abs() * 0.654);
            bridgerectifier = s.smooth_fr + input_sample_r;
            s.smooth_fr = input_sample_r;
            input_sample_r = bridgerectifier;
            out_sample = input_sample_l * s.fix_d[FIX_A0] + s.fix_d[FIX_S_L1];
            s.fix_d[FIX_S_L1] = input_sample_l * s.fix_d[FIX_A1] - out_sample * s.fix_d[FIX_B1] + s.fix_d[FIX_S_L2];
            s.fix_d[FIX_S_L2] = input_sample_l * s.fix_d[FIX_A2] - out_sample * s.fix_d[FIX_B2];
            input_sample_l = out_sample;
            out_sample = input_sample_r * s.fix_d[FIX_A0] + s.fix_d[FIX_S_R1];
            s.fix_d[FIX_S_R1] = input_sample_r * s.fix_d[FIX_A1] - out_sample * s.fix_d[FIX_B1] + s.fix_d[FIX_S_R2];
            s.fix_d[FIX_S_R2] = input_sample_r * s.fix_d[FIX_A2] - out_sample * s.fix_d[FIX_B2];
            input_sample_r = out_sample;
            input_sample_l *= inputlevel_l;
            inputlevel_l = (inputlevel_l * 7.0 + 1.0) / 8.0;
            s.iir_sample_gl = s.iir_sample_gl * (1.0 - eq) + input_sample_l * eq;
            basscut_l *= bassfactor;
            input_sample_l = input_sample_l - s.iir_sample_gl * basscut_l;
            input_sample_l -= input_sample_l * (input_sample_l.abs() * 0.654) * (input_sample_l.abs() * 0.654);
            bridgerectifier = s.smooth_gl + input_sample_l;
            s.smooth_gl = input_sample_l;
            input_sample_l = bridgerectifier;
            input_sample_l *= inputlevel_l;
            inputlevel_l = (inputlevel_l * 7.0 + 1.0) / 8.0;
            s.iir_sample_hl = s.iir_sample_hl * (1.0 - eq) + input_sample_l * eq;
            basscut_l *= bassfactor;
            input_sample_l = input_sample_l - s.iir_sample_hl * basscut_l;
            input_sample_l -= input_sample_l * (input_sample_l.abs() * 0.654) * (input_sample_l.abs() * 0.654);
            bridgerectifier = s.smooth_hl + input_sample_l;
            s.smooth_hl = input_sample_l;
            input_sample_l = bridgerectifier;
            input_sample_r *= inputlevel_r;
            inputlevel_r = (inputlevel_r * 7.0 + 1.0) / 8.0;
            s.iir_sample_gr = s.iir_sample_gr * (1.0 - eq) + input_sample_r * eq;
            basscut_r *= bassfactor;
            input_sample_r = input_sample_r - s.iir_sample_gr * basscut_r;
            input_sample_r -= input_sample_r * (input_sample_r.abs() * 0.654) * (input_sample_r.abs() * 0.654);
            bridgerectifier = s.smooth_gr + input_sample_r;
            s.smooth_gr = input_sample_r;
            input_sample_r = bridgerectifier;
            input_sample_r *= inputlevel_r;
            inputlevel_r = (inputlevel_r * 7.0 + 1.0) / 8.0;
            s.iir_sample_hr = s.iir_sample_hr * (1.0 - eq) + input_sample_r * eq;
            basscut_r *= bassfactor;
            input_sample_r = input_sample_r - s.iir_sample_hr * basscut_r;
            input_sample_r -= input_sample_r * (input_sample_r.abs() * 0.654) * (input_sample_r.abs() * 0.654);
            bridgerectifier = s.smooth_hr + input_sample_r;
            s.smooth_hr = input_sample_r;
            input_sample_r = bridgerectifier;
            out_sample = input_sample_l * s.fix_e[FIX_A0] + s.fix_e[FIX_S_L1];
            s.fix_e[FIX_S_L1] = input_sample_l * s.fix_e[FIX_A1] - out_sample * s.fix_e[FIX_B1] + s.fix_e[FIX_S_L2];
            s.fix_e[FIX_S_L2] = input_sample_l * s.fix_e[FIX_A2] - out_sample * s.fix_e[FIX_B2];
            input_sample_l = out_sample;
            out_sample = input_sample_r * s.fix_e[FIX_A0] + s.fix_e[FIX_S_R1];
            s.fix_e[FIX_S_R1] = input_sample_r * s.fix_e[FIX_A1] - out_sample * s.fix_e[FIX_B1] + s.fix_e[FIX_S_R2];
            s.fix_e[FIX_S_R2] = input_sample_r * s.fix_e[FIX_A2] - out_sample * s.fix_e[FIX_B2];
            input_sample_r = out_sample;
            input_sample_l *= inputlevel_l;
            inputlevel_l = (inputlevel_l * 7.0 + 1.0) / 8.0;
            s.iir_sample_il = s.iir_sample_il * (1.0 - eq) + input_sample_l * eq;
            basscut_l *= bassfactor;
            input_sample_l = input_sample_l - s.iir_sample_il * basscut_l;
            input_sample_l -= input_sample_l * (input_sample_l.abs() * 0.654) * (input_sample_l.abs() * 0.654);
            bridgerectifier = s.smooth_il + input_sample_l;
            s.smooth_il = input_sample_l;
            input_sample_l = bridgerectifier;
            input_sample_l *= inputlevel_l;
            inputlevel_l = (inputlevel_l * 7.0 + 1.0) / 8.0;
            s.iir_sample_jl = s.iir_sample_jl * (1.0 - eq) + input_sample_l * eq;
            basscut_l *= bassfactor;
            input_sample_l = input_sample_l - s.iir_sample_jl * basscut_l;
            input_sample_l -= input_sample_l * (input_sample_l.abs() * 0.654) * (input_sample_l.abs() * 0.654);
            bridgerectifier = s.smooth_jl + input_sample_l;
            s.smooth_jl = input_sample_l;
            input_sample_l = bridgerectifier;
            input_sample_r *= inputlevel_r;
            inputlevel_r = (inputlevel_r * 7.0 + 1.0) / 8.0;
            s.iir_sample_ir = s.iir_sample_ir * (1.0 - eq) + input_sample_r * eq;
            basscut_r *= bassfactor;
            input_sample_r = input_sample_r - s.iir_sample_ir * basscut_r;
            input_sample_r -= input_sample_r * (input_sample_r.abs() * 0.654) * (input_sample_r.abs() * 0.654);
            bridgerectifier = s.smooth_ir + input_sample_r;
            s.smooth_ir = input_sample_r;
            input_sample_r = bridgerectifier;
            input_sample_r *= inputlevel_r;
            inputlevel_r = (inputlevel_r * 7.0 + 1.0) / 8.0;
            s.iir_sample_jr = s.iir_sample_jr * (1.0 - eq) + input_sample_r * eq;
            basscut_r *= bassfactor;
            input_sample_r = input_sample_r - s.iir_sample_jr * basscut_r;
            input_sample_r -= input_sample_r * (input_sample_r.abs() * 0.654) * (input_sample_r.abs() * 0.654);
            bridgerectifier = s.smooth_jr + input_sample_r;
            s.smooth_jr = input_sample_r;
            input_sample_r = bridgerectifier;
            out_sample = input_sample_l * s.fix_f[FIX_A0] + s.fix_f[FIX_S_L1];
            s.fix_f[FIX_S_L1] = input_sample_l * s.fix_f[FIX_A1] - out_sample * s.fix_f[FIX_B1] + s.fix_f[FIX_S_L2];
            s.fix_f[FIX_S_L2] = input_sample_l * s.fix_f[FIX_A2] - out_sample * s.fix_f[FIX_B2];
            input_sample_l = out_sample;
            out_sample = input_sample_r * s.fix_f[FIX_A0] + s.fix_f[FIX_S_R1];
            s.fix_f[FIX_S_R1] = input_sample_r * s.fix_f[FIX_A1] - out_sample * s.fix_f[FIX_B1] + s.fix_f[FIX_S_R2];
            s.fix_f[FIX_S_R2] = input_sample_r * s.fix_f[FIX_A2] - out_sample * s.fix_f[FIX_B2];
            input_sample_r = out_sample;
            input_sample_l *= inputlevel_l;
            inputlevel_l = (inputlevel_l * 7.0 + 1.0) / 8.0;
            s.iir_sample_kl = s.iir_sample_kl * (1.0 - eq) + input_sample_l * eq;
            basscut_l *= bassfactor;
            input_sample_l = input_sample_l - s.iir_sample_kl * basscut_l;
            input_sample_l -= input_sample_l * (input_sample_l.abs() * 0.654) * (input_sample_l.abs() * 0.654);
            bridgerectifier = s.smooth_kl + input_sample_l;
            s.smooth_kl = input_sample_l;
            input_sample_l = bridgerectifier;
            input_sample_l *= inputlevel_l;
            inputlevel_l = (inputlevel_l * 7.0 + 1.0) / 8.0;
            s.iir_sample_ll = s.iir_sample_ll * (1.0 - eq) + input_sample_l * eq;
            basscut_l *= bassfactor;
            input_sample_l = input_sample_l - s.iir_sample_ll * basscut_l;
            input_sample_l -= input_sample_l * (input_sample_l.abs() * 0.654) * (input_sample_l.abs() * 0.654);
            bridgerectifier = s.smooth_ll + input_sample_l;
            s.smooth_ll = input_sample_l;
            input_sample_l = bridgerectifier;
            input_sample_r *= inputlevel_r;
            inputlevel_r = (inputlevel_r * 7.0 + 1.0) / 8.0;
            s.iir_sample_kr = s.iir_sample_kr * (1.0 - eq) + input_sample_r * eq;
            basscut_r *= bassfactor;
            input_sample_r = input_sample_r - s.iir_sample_kr * basscut_r;
            input_sample_r -= input_sample_r * (input_sample_r.abs() * 0.654) * (input_sample_r.abs() * 0.654);
            bridgerectifier = s.smooth_kr + input_sample_r;
            s.smooth_kr = input_sample_r;
            input_sample_r = bridgerectifier;
            input_sample_r *= inputlevel_r;
            inputlevel_r = (inputlevel_r * 7.0 + 1.0) / 8.0;
            s.iir_sample_lr = s.iir_sample_lr * (1.0 - eq) + input_sample_r * eq;
            basscut_r *= bassfactor;
            input_sample_r = input_sample_r - s.iir_sample_lr * basscut_r;
            input_sample_r -= input_sample_r * (input_sample_r.abs() * 0.654) * (input_sample_r.abs() * 0.654);
            bridgerectifier = s.smooth_lr + input_sample_r;
            s.smooth_lr = input_sample_r;
            input_sample_r = bridgerectifier;
            s.iir_lowpass_l = s.iir_lowpass_l * (1.0 - tone_eq) + input_sample_l * tone_eq;
            input_sample_l = s.iir_lowpass_l;
            s.iir_lowpass_r = s.iir_lowpass_r * (1.0 - tone_eq) + input_sample_r * tone_eq;
            input_sample_r = s.iir_lowpass_r;
            s.iir_spk_al = s.iir_spk_al * (1.0 - beq) + input_sample_l * beq;
            s.iir_spk_ar = s.iir_spk_ar * (1.0 - beq) + input_sample_r * beq;
            if s.count < 0 || s.count > 128 {
                s.count = 128;
            }
            let mut result_bl: f64 = 0.0;
            let mut result_br: f64 = 0.0;
            if s.flip {
                s.odd_l[s.count as usize] = s.iir_spk_al;
                s.odd_l[(s.count + 128) as usize] = s.odd_l[s.count as usize];
                result_bl = s.odd_l[(s.count + down) as usize] + s.odd_l[(s.count + side) as usize] + s.odd_l[(s.count + diagonal) as usize];
                s.odd_r[s.count as usize] = s.iir_spk_ar;
                s.odd_r[(s.count + 128) as usize] = s.odd_r[s.count as usize];
                result_br = s.odd_r[(s.count + down) as usize] + s.odd_r[(s.count + side) as usize] + s.odd_r[(s.count + diagonal) as usize];
            } else {
                s.even_l[s.count as usize] = s.iir_spk_al;
                s.even_l[(s.count + 128) as usize] = s.even_l[s.count as usize];
                result_bl = s.even_l[(s.count + down) as usize] + s.even_l[(s.count + side) as usize] + s.even_l[(s.count + diagonal) as usize];
                s.even_r[s.count as usize] = s.iir_spk_ar;
                s.even_r[(s.count + 128) as usize] = s.even_r[s.count as usize];
                result_br = s.even_r[(s.count + down) as usize] + s.even_r[(s.count + side) as usize] + s.even_r[(s.count + diagonal) as usize];
            }
            s.count -= 1;
            s.iir_spk_bl = s.iir_spk_bl * (1.0 - beq) + result_bl * beq;
            input_sample_l += s.iir_spk_bl * bleed;
            s.iir_spk_br = s.iir_spk_br * (1.0 - beq) + result_br * beq;
            input_sample_r += s.iir_spk_br * bleed;
            bridgerectifier = (input_sample_l * outputlevel).abs();
            if bridgerectifier > 1.57079633 {
                bridgerectifier = 1.57079633;
            }
            bridgerectifier = bridgerectifier.sin();
            if input_sample_l > 0.0 {
                input_sample_l = bridgerectifier;
            } else {
                input_sample_l = -bridgerectifier;
            }
            bridgerectifier = (input_sample_r * outputlevel).abs();
            if bridgerectifier > 1.57079633 {
                bridgerectifier = 1.57079633;
            }
            bridgerectifier = bridgerectifier.sin();
            if input_sample_r > 0.0 {
                input_sample_r = bridgerectifier;
            } else {
                input_sample_r = -bridgerectifier;
            }
            s.iir_sub_l = s.iir_sub_l * (1.0 - beq) + input_sample_l * beq;
            input_sample_l += s.iir_sub_l * bassfill * outputlevel;
            s.iir_sub_r = s.iir_sub_r * (1.0 - beq) + input_sample_r * beq;
            input_sample_r += s.iir_sub_r * bassfill * outputlevel;
            let mut randy: f64 = s.fpd_l as f64 / u32::MAX as f64 * 0.053;
            input_sample_l = (input_sample_l * (1.0 - randy) + s.store_sample_l * randy) * outputlevel;
            s.store_sample_l = input_sample_l;
            randy = s.fpd_r as f64 / u32::MAX as f64 * 0.053;
            input_sample_r = (input_sample_r * (1.0 - randy) + s.store_sample_r * randy) * outputlevel;
            s.store_sample_r = input_sample_r;
            s.flip = !s.flip;
            if wet != 1.0 {
                input_sample_l = input_sample_l * wet + dry_sample_l * (1.0 - wet);
                input_sample_r = input_sample_r * wet + dry_sample_r * (1.0 - wet);
            }
            s.cycle += 1;
            if s.cycle == cycle_end {
                let mut temp: f64 = (input_sample_l + s.smooth_cab_al) / 3.0;
                s.smooth_cab_al = input_sample_l;
                input_sample_l = temp;
                s.b_l[84] = s.b_l[83];
                s.b_l[83] = s.b_l[82];
                s.b_l[82] = s.b_l[81];
                s.b_l[81] = s.b_l[80];
                s.b_l[80] = s.b_l[79];
                s.b_l[79] = s.b_l[78];
                s.b_l[78] = s.b_l[77];
                s.b_l[77] = s.b_l[76];
                s.b_l[76] = s.b_l[75];
                s.b_l[75] = s.b_l[74];
                s.b_l[74] = s.b_l[73];
                s.b_l[73] = s.b_l[72];
                s.b_l[72] = s.b_l[71];
                s.b_l[71] = s.b_l[70];
                s.b_l[70] = s.b_l[69];
                s.b_l[69] = s.b_l[68];
                s.b_l[68] = s.b_l[67];
                s.b_l[67] = s.b_l[66];
                s.b_l[66] = s.b_l[65];
                s.b_l[65] = s.b_l[64];
                s.b_l[64] = s.b_l[63];
                s.b_l[63] = s.b_l[62];
                s.b_l[62] = s.b_l[61];
                s.b_l[61] = s.b_l[60];
                s.b_l[60] = s.b_l[59];
                s.b_l[59] = s.b_l[58];
                s.b_l[58] = s.b_l[57];
                s.b_l[57] = s.b_l[56];
                s.b_l[56] = s.b_l[55];
                s.b_l[55] = s.b_l[54];
                s.b_l[54] = s.b_l[53];
                s.b_l[53] = s.b_l[52];
                s.b_l[52] = s.b_l[51];
                s.b_l[51] = s.b_l[50];
                s.b_l[50] = s.b_l[49];
                s.b_l[49] = s.b_l[48];
                s.b_l[48] = s.b_l[47];
                s.b_l[47] = s.b_l[46];
                s.b_l[46] = s.b_l[45];
                s.b_l[45] = s.b_l[44];
                s.b_l[44] = s.b_l[43];
                s.b_l[43] = s.b_l[42];
                s.b_l[42] = s.b_l[41];
                s.b_l[41] = s.b_l[40];
                s.b_l[40] = s.b_l[39];
                s.b_l[39] = s.b_l[38];
                s.b_l[38] = s.b_l[37];
                s.b_l[37] = s.b_l[36];
                s.b_l[36] = s.b_l[35];
                s.b_l[35] = s.b_l[34];
                s.b_l[34] = s.b_l[33];
                s.b_l[33] = s.b_l[32];
                s.b_l[32] = s.b_l[31];
                s.b_l[31] = s.b_l[30];
                s.b_l[30] = s.b_l[29];
                s.b_l[29] = s.b_l[28];
                s.b_l[28] = s.b_l[27];
                s.b_l[27] = s.b_l[26];
                s.b_l[26] = s.b_l[25];
                s.b_l[25] = s.b_l[24];
                s.b_l[24] = s.b_l[23];
                s.b_l[23] = s.b_l[22];
                s.b_l[22] = s.b_l[21];
                s.b_l[21] = s.b_l[20];
                s.b_l[20] = s.b_l[19];
                s.b_l[19] = s.b_l[18];
                s.b_l[18] = s.b_l[17];
                s.b_l[17] = s.b_l[16];
                s.b_l[16] = s.b_l[15];
                s.b_l[15] = s.b_l[14];
                s.b_l[14] = s.b_l[13];
                s.b_l[13] = s.b_l[12];
                s.b_l[12] = s.b_l[11];
                s.b_l[11] = s.b_l[10];
                s.b_l[10] = s.b_l[9];
                s.b_l[9] = s.b_l[8];
                s.b_l[8] = s.b_l[7];
                s.b_l[7] = s.b_l[6];
                s.b_l[6] = s.b_l[5];
                s.b_l[5] = s.b_l[4];
                s.b_l[4] = s.b_l[3];
                s.b_l[3] = s.b_l[2];
                s.b_l[2] = s.b_l[1];
                s.b_l[1] = s.b_l[0];
                s.b_l[0] = input_sample_l;
                input_sample_l += s.b_l[1] * (1.31698250313308396 - 0.08140616497621633 * s.b_l[1].abs());
                input_sample_l += s.b_l[2] * (1.47229016949915326 - 0.27680278993637253 * s.b_l[2].abs());
                input_sample_l += s.b_l[3] * (1.30410109086044956 - 0.35629113432046489 * s.b_l[3].abs());
                input_sample_l += s.b_l[4] * (0.81766210474551260 - 0.26808782337659753 * s.b_l[4].abs());
                input_sample_l += s.b_l[5] * (0.19868872545506663 - 0.11105517193919669 * s.b_l[5].abs());
                input_sample_l -= s.b_l[6] * (0.39115909132567039 - 0.12630622002682679 * s.b_l[6].abs());
                input_sample_l -= s.b_l[7] * (0.76881891559343574 - 0.40879849500403143 * s.b_l[7].abs());
                input_sample_l -= s.b_l[8] * (0.87146861782680340 - 0.59529560488000599 * s.b_l[8].abs());
                input_sample_l -= s.b_l[9] * (0.79504575932563670 - 0.60877047551611796 * s.b_l[9].abs());
                input_sample_l -= s.b_l[10] * (0.61653017622406314 - 0.47662851438557335 * s.b_l[10].abs());
                input_sample_l -= s.b_l[11] * (0.40718195794382067 - 0.24955839378539713 * s.b_l[11].abs());
                input_sample_l -= s.b_l[12] * (0.31794900040616203 - 0.04169792259600613 * s.b_l[12].abs());
                input_sample_l -= s.b_l[13] * (0.41075032540217843 + 0.00368483996076280 * s.b_l[13].abs());
                input_sample_l -= s.b_l[14] * (0.56901352922170667 - 0.11027360805893105 * s.b_l[14].abs());
                input_sample_l -= s.b_l[15] * (0.62443222391889264 - 0.22198075154245228 * s.b_l[15].abs());
                input_sample_l -= s.b_l[16] * (0.53462856723129204 - 0.22933544545324852 * s.b_l[16].abs());
                input_sample_l -= s.b_l[17] * (0.34441703361995046 - 0.12956809502269492 * s.b_l[17].abs());
                input_sample_l -= s.b_l[18] * (0.13947052337867882 + 0.00339775055962799 * s.b_l[18].abs());
                input_sample_l += s.b_l[19] * (0.03771252648928484 - 0.10863931549251718 * s.b_l[19].abs());
                input_sample_l += s.b_l[20] * (0.18280210770271693 - 0.17413646599296417 * s.b_l[20].abs());
                input_sample_l += s.b_l[21] * (0.24621986701761467 - 0.14547053270435095 * s.b_l[21].abs());
                input_sample_l += s.b_l[22] * (0.22347075142737360 - 0.02493869490104031 * s.b_l[22].abs());
                input_sample_l += s.b_l[23] * (0.14346348482123716 + 0.11284054747963246 * s.b_l[23].abs());
                input_sample_l += s.b_l[24] * (0.00834364862916028 + 0.24284684053733926 * s.b_l[24].abs());
                input_sample_l -= s.b_l[25] * (0.11559740296078347 - 0.32623054435304538 * s.b_l[25].abs());
                input_sample_l -= s.b_l[26] * (0.18067604561283060 - 0.32311481551122478 * s.b_l[26].abs());
                input_sample_l -= s.b_l[27] * (0.22927997789035612 - 0.26991539052832925 * s.b_l[27].abs());
                input_sample_l -= s.b_l[28] * (0.28487666578669446 - 0.22437227250279349 * s.b_l[28].abs());
                input_sample_l -= s.b_l[29] * (0.31992973037153838 - 0.15289876100963865 * s.b_l[29].abs());
                input_sample_l -= s.b_l[30] * (0.35174606303520733 - 0.05656293023086628 * s.b_l[30].abs());
                input_sample_l -= s.b_l[31] * (0.36894898011375254 + 0.04333925421463558 * s.b_l[31].abs());
                input_sample_l -= s.b_l[32] * (0.32567576055307507 + 0.14594589410921388 * s.b_l[32].abs());
                input_sample_l -= s.b_l[33] * (0.27440135050585784 + 0.15529667398122521 * s.b_l[33].abs());
                input_sample_l -= s.b_l[34] * (0.21998973785078091 + 0.05083553737157104 * s.b_l[34].abs());
                input_sample_l -= s.b_l[35] * (0.10323624876862457 - 0.04651829594199963 * s.b_l[35].abs());
                input_sample_l += s.b_l[36] * (0.02091603687851074 + 0.12000046818439322 * s.b_l[36].abs());
                input_sample_l += s.b_l[37] * (0.11344930914138468 + 0.17697142512225839 * s.b_l[37].abs());
                input_sample_l += s.b_l[38] * (0.22766779627643968 + 0.13645102964003858 * s.b_l[38].abs());
                input_sample_l += s.b_l[39] * (0.38378309953638229 - 0.01997653307333791 * s.b_l[39].abs());
                input_sample_l += s.b_l[40] * (0.52789400804568076 - 0.21409137428422448 * s.b_l[40].abs());
                input_sample_l += s.b_l[41] * (0.55444630296938280 - 0.32331980931576626 * s.b_l[41].abs());
                input_sample_l += s.b_l[42] * (0.42333237669264601 - 0.26855847463044280 * s.b_l[42].abs());
                input_sample_l += s.b_l[43] * (0.21942831522035078 - 0.12051365248820624 * s.b_l[43].abs());
                input_sample_l -= s.b_l[44] * (0.00584169427830633 - 0.03706970171280329 * s.b_l[44].abs());
                input_sample_l -= s.b_l[45] * (0.24279799124660351 - 0.17296440491477982 * s.b_l[45].abs());
                input_sample_l -= s.b_l[46] * (0.40173760787507085 - 0.21717989835163351 * s.b_l[46].abs());
                input_sample_l -= s.b_l[47] * (0.43930035724188155 - 0.16425928481378199 * s.b_l[47].abs());
                input_sample_l -= s.b_l[48] * (0.41067765934041811 - 0.10390115786636855 * s.b_l[48].abs());
                input_sample_l -= s.b_l[49] * (0.34409235547165967 - 0.07268159377411920 * s.b_l[49].abs());
                input_sample_l -= s.b_l[50] * (0.26542883122568151 - 0.05483457497365785 * s.b_l[50].abs());
                input_sample_l -= s.b_l[51] * (0.22024754776138800 - 0.06484897950087598 * s.b_l[51].abs());
                input_sample_l -= s.b_l[52] * (0.20394367993632415 - 0.08746309731952180 * s.b_l[52].abs());
                input_sample_l -= s.b_l[53] * (0.17565242431124092 - 0.07611309538078760 * s.b_l[53].abs());
                input_sample_l -= s.b_l[54] * (0.10116623231246825 - 0.00642818706295112 * s.b_l[54].abs());
                input_sample_l -= s.b_l[55] * (0.00782648272053632 + 0.08004141267685004 * s.b_l[55].abs());
                input_sample_l += s.b_l[56] * (0.05059046006747323 - 0.12436676387548490 * s.b_l[56].abs());
                input_sample_l += s.b_l[57] * (0.06241531553254467 - 0.11530779547021434 * s.b_l[57].abs());
                input_sample_l += s.b_l[58] * (0.04952694587101836 - 0.08340945324333944 * s.b_l[58].abs());
                input_sample_l += s.b_l[59] * (0.00843873294401687 - 0.03279659052562903 * s.b_l[59].abs());
                input_sample_l -= s.b_l[60] * (0.05161338949440241 - 0.03428181149163798 * s.b_l[60].abs());
                input_sample_l -= s.b_l[61] * (0.08165520146902012 - 0.08196746092283110 * s.b_l[61].abs());
                input_sample_l -= s.b_l[62] * (0.06639532849935320 - 0.09797462781896329 * s.b_l[62].abs());
                input_sample_l -= s.b_l[63] * (0.02953430910661621 - 0.09175612938515763 * s.b_l[63].abs());
                input_sample_l += s.b_l[64] * (0.00741058547442938 + 0.05442091048731967 * s.b_l[64].abs());
                input_sample_l += s.b_l[65] * (0.01832866125391727 + 0.00306243693643687 * s.b_l[65].abs());
                input_sample_l += s.b_l[66] * (0.00526964230373573 - 0.04364102661136410 * s.b_l[66].abs());
                input_sample_l -= s.b_l[67] * (0.00300984373848200 + 0.09742737841278880 * s.b_l[67].abs());
                input_sample_l -= s.b_l[68] * (0.00413616769576694 + 0.14380661694523073 * s.b_l[68].abs());
                input_sample_l -= s.b_l[69] * (0.00588769034931419 + 0.16012843578892538 * s.b_l[69].abs());
                input_sample_l -= s.b_l[70] * (0.00688588239450581 + 0.14074464279305798 * s.b_l[70].abs());
                input_sample_l -= s.b_l[71] * (0.02277307992926315 + 0.07914752191801366 * s.b_l[71].abs());
                input_sample_l -= s.b_l[72] * (0.04627166091180877 - 0.00192787268067208 * s.b_l[72].abs());
                input_sample_l -= s.b_l[73] * (0.05562045897455786 - 0.05932868727665747 * s.b_l[73].abs());
                input_sample_l -= s.b_l[74] * (0.05134243784922165 - 0.08245334798868090 * s.b_l[74].abs());
                input_sample_l -= s.b_l[75] * (0.04719409472239919 - 0.07498680629253825 * s.b_l[75].abs());
                input_sample_l -= s.b_l[76] * (0.05889738914266415 - 0.06116127018043697 * s.b_l[76].abs());
                input_sample_l -= s.b_l[77] * (0.09428363535111127 - 0.06535868867863834 * s.b_l[77].abs());
                input_sample_l -= s.b_l[78] * (0.15181756953225126 - 0.08982979655234427 * s.b_l[78].abs());
                input_sample_l -= s.b_l[79] * (0.20878969456036670 - 0.10761070891499538 * s.b_l[79].abs());
                input_sample_l -= s.b_l[80] * (0.22647885581813790 - 0.08462542510349125 * s.b_l[80].abs());
                input_sample_l -= s.b_l[81] * (0.19723482443646323 - 0.02665160920736287 * s.b_l[81].abs());
                input_sample_l -= s.b_l[82] * (0.16441643451155163 + 0.02314691954338197 * s.b_l[82].abs());
                input_sample_l -= s.b_l[83] * (0.15201914054931515 + 0.04424903493886839 * s.b_l[83].abs());
                input_sample_l -= s.b_l[84] * (0.15454370641307855 + 0.04223203797913008 * s.b_l[84].abs());
                temp = (input_sample_l + s.smooth_cab_bl) / 3.0;
                s.smooth_cab_bl = input_sample_l;
                input_sample_l = temp / 4.0;
                randy = s.fpd_l as f64 / u32::MAX as f64 * 0.057;
                dry_sample_l = ((input_sample_l * (1.0 - randy) + s.last_cab_sample_l * randy) * wet + dry_sample_l * (1.0 - wet)) * outputlevel;
                s.last_cab_sample_l = input_sample_l;
                input_sample_l = dry_sample_l;
                temp = (input_sample_r + s.smooth_cab_ar) / 3.0;
                s.smooth_cab_ar = input_sample_r;
                input_sample_r = temp;
                s.b_r[84] = s.b_r[83];
                s.b_r[83] = s.b_r[82];
                s.b_r[82] = s.b_r[81];
                s.b_r[81] = s.b_r[80];
                s.b_r[80] = s.b_r[79];
                s.b_r[79] = s.b_r[78];
                s.b_r[78] = s.b_r[77];
                s.b_r[77] = s.b_r[76];
                s.b_r[76] = s.b_r[75];
                s.b_r[75] = s.b_r[74];
                s.b_r[74] = s.b_r[73];
                s.b_r[73] = s.b_r[72];
                s.b_r[72] = s.b_r[71];
                s.b_r[71] = s.b_r[70];
                s.b_r[70] = s.b_r[69];
                s.b_r[69] = s.b_r[68];
                s.b_r[68] = s.b_r[67];
                s.b_r[67] = s.b_r[66];
                s.b_r[66] = s.b_r[65];
                s.b_r[65] = s.b_r[64];
                s.b_r[64] = s.b_r[63];
                s.b_r[63] = s.b_r[62];
                s.b_r[62] = s.b_r[61];
                s.b_r[61] = s.b_r[60];
                s.b_r[60] = s.b_r[59];
                s.b_r[59] = s.b_r[58];
                s.b_r[58] = s.b_r[57];
                s.b_r[57] = s.b_r[56];
                s.b_r[56] = s.b_r[55];
                s.b_r[55] = s.b_r[54];
                s.b_r[54] = s.b_r[53];
                s.b_r[53] = s.b_r[52];
                s.b_r[52] = s.b_r[51];
                s.b_r[51] = s.b_r[50];
                s.b_r[50] = s.b_r[49];
                s.b_r[49] = s.b_r[48];
                s.b_r[48] = s.b_r[47];
                s.b_r[47] = s.b_r[46];
                s.b_r[46] = s.b_r[45];
                s.b_r[45] = s.b_r[44];
                s.b_r[44] = s.b_r[43];
                s.b_r[43] = s.b_r[42];
                s.b_r[42] = s.b_r[41];
                s.b_r[41] = s.b_r[40];
                s.b_r[40] = s.b_r[39];
                s.b_r[39] = s.b_r[38];
                s.b_r[38] = s.b_r[37];
                s.b_r[37] = s.b_r[36];
                s.b_r[36] = s.b_r[35];
                s.b_r[35] = s.b_r[34];
                s.b_r[34] = s.b_r[33];
                s.b_r[33] = s.b_r[32];
                s.b_r[32] = s.b_r[31];
                s.b_r[31] = s.b_r[30];
                s.b_r[30] = s.b_r[29];
                s.b_r[29] = s.b_r[28];
                s.b_r[28] = s.b_r[27];
                s.b_r[27] = s.b_r[26];
                s.b_r[26] = s.b_r[25];
                s.b_r[25] = s.b_r[24];
                s.b_r[24] = s.b_r[23];
                s.b_r[23] = s.b_r[22];
                s.b_r[22] = s.b_r[21];
                s.b_r[21] = s.b_r[20];
                s.b_r[20] = s.b_r[19];
                s.b_r[19] = s.b_r[18];
                s.b_r[18] = s.b_r[17];
                s.b_r[17] = s.b_r[16];
                s.b_r[16] = s.b_r[15];
                s.b_r[15] = s.b_r[14];
                s.b_r[14] = s.b_r[13];
                s.b_r[13] = s.b_r[12];
                s.b_r[12] = s.b_r[11];
                s.b_r[11] = s.b_r[10];
                s.b_r[10] = s.b_r[9];
                s.b_r[9] = s.b_r[8];
                s.b_r[8] = s.b_r[7];
                s.b_r[7] = s.b_r[6];
                s.b_r[6] = s.b_r[5];
                s.b_r[5] = s.b_r[4];
                s.b_r[4] = s.b_r[3];
                s.b_r[3] = s.b_r[2];
                s.b_r[2] = s.b_r[1];
                s.b_r[1] = s.b_r[0];
                s.b_r[0] = input_sample_r;
                input_sample_r += s.b_r[1] * (1.31698250313308396 - 0.08140616497621633 * s.b_r[1].abs());
                input_sample_r += s.b_r[2] * (1.47229016949915326 - 0.27680278993637253 * s.b_r[2].abs());
                input_sample_r += s.b_r[3] * (1.30410109086044956 - 0.35629113432046489 * s.b_r[3].abs());
                input_sample_r += s.b_r[4] * (0.81766210474551260 - 0.26808782337659753 * s.b_r[4].abs());
                input_sample_r += s.b_r[5] * (0.19868872545506663 - 0.11105517193919669 * s.b_r[5].abs());
                input_sample_r -= s.b_r[6] * (0.39115909132567039 - 0.12630622002682679 * s.b_r[6].abs());
                input_sample_r -= s.b_r[7] * (0.76881891559343574 - 0.40879849500403143 * s.b_r[7].abs());
                input_sample_r -= s.b_r[8] * (0.87146861782680340 - 0.59529560488000599 * s.b_r[8].abs());
                input_sample_r -= s.b_r[9] * (0.79504575932563670 - 0.60877047551611796 * s.b_r[9].abs());
                input_sample_r -= s.b_r[10] * (0.61653017622406314 - 0.47662851438557335 * s.b_r[10].abs());
                input_sample_r -= s.b_r[11] * (0.40718195794382067 - 0.24955839378539713 * s.b_r[11].abs());
                input_sample_r -= s.b_r[12] * (0.31794900040616203 - 0.04169792259600613 * s.b_r[12].abs());
                input_sample_r -= s.b_r[13] * (0.41075032540217843 + 0.00368483996076280 * s.b_r[13].abs());
                input_sample_r -= s.b_r[14] * (0.56901352922170667 - 0.11027360805893105 * s.b_r[14].abs());
                input_sample_r -= s.b_r[15] * (0.62443222391889264 - 0.22198075154245228 * s.b_r[15].abs());
                input_sample_r -= s.b_r[16] * (0.53462856723129204 - 0.22933544545324852 * s.b_r[16].abs());
                input_sample_r -= s.b_r[17] * (0.34441703361995046 - 0.12956809502269492 * s.b_r[17].abs());
                input_sample_r -= s.b_r[18] * (0.13947052337867882 + 0.00339775055962799 * s.b_r[18].abs());
                input_sample_r += s.b_r[19] * (0.03771252648928484 - 0.10863931549251718 * s.b_r[19].abs());
                input_sample_r += s.b_r[20] * (0.18280210770271693 - 0.17413646599296417 * s.b_r[20].abs());
                input_sample_r += s.b_r[21] * (0.24621986701761467 - 0.14547053270435095 * s.b_r[21].abs());
                input_sample_r += s.b_r[22] * (0.22347075142737360 - 0.02493869490104031 * s.b_r[22].abs());
                input_sample_r += s.b_r[23] * (0.14346348482123716 + 0.11284054747963246 * s.b_r[23].abs());
                input_sample_r += s.b_r[24] * (0.00834364862916028 + 0.24284684053733926 * s.b_r[24].abs());
                input_sample_r -= s.b_r[25] * (0.11559740296078347 - 0.32623054435304538 * s.b_r[25].abs());
                input_sample_r -= s.b_r[26] * (0.18067604561283060 - 0.32311481551122478 * s.b_r[26].abs());
                input_sample_r -= s.b_r[27] * (0.22927997789035612 - 0.26991539052832925 * s.b_r[27].abs());
                input_sample_r -= s.b_r[28] * (0.28487666578669446 - 0.22437227250279349 * s.b_r[28].abs());
                input_sample_r -= s.b_r[29] * (0.31992973037153838 - 0.15289876100963865 * s.b_r[29].abs());
                input_sample_r -= s.b_r[30] * (0.35174606303520733 - 0.05656293023086628 * s.b_r[30].abs());
                input_sample_r -= s.b_r[31] * (0.36894898011375254 + 0.04333925421463558 * s.b_r[31].abs());
                input_sample_r -= s.b_r[32] * (0.32567576055307507 + 0.14594589410921388 * s.b_r[32].abs());
                input_sample_r -= s.b_r[33] * (0.27440135050585784 + 0.15529667398122521 * s.b_r[33].abs());
                input_sample_r -= s.b_r[34] * (0.21998973785078091 + 0.05083553737157104 * s.b_r[34].abs());
                input_sample_r -= s.b_r[35] * (0.10323624876862457 - 0.04651829594199963 * s.b_r[35].abs());
                input_sample_r += s.b_r[36] * (0.02091603687851074 + 0.12000046818439322 * s.b_r[36].abs());
                input_sample_r += s.b_r[37] * (0.11344930914138468 + 0.17697142512225839 * s.b_r[37].abs());
                input_sample_r += s.b_r[38] * (0.22766779627643968 + 0.13645102964003858 * s.b_r[38].abs());
                input_sample_r += s.b_r[39] * (0.38378309953638229 - 0.01997653307333791 * s.b_r[39].abs());
                input_sample_r += s.b_r[40] * (0.52789400804568076 - 0.21409137428422448 * s.b_r[40].abs());
                input_sample_r += s.b_r[41] * (0.55444630296938280 - 0.32331980931576626 * s.b_r[41].abs());
                input_sample_r += s.b_r[42] * (0.42333237669264601 - 0.26855847463044280 * s.b_r[42].abs());
                input_sample_r += s.b_r[43] * (0.21942831522035078 - 0.12051365248820624 * s.b_r[43].abs());
                input_sample_r -= s.b_r[44] * (0.00584169427830633 - 0.03706970171280329 * s.b_r[44].abs());
                input_sample_r -= s.b_r[45] * (0.24279799124660351 - 0.17296440491477982 * s.b_r[45].abs());
                input_sample_r -= s.b_r[46] * (0.40173760787507085 - 0.21717989835163351 * s.b_r[46].abs());
                input_sample_r -= s.b_r[47] * (0.43930035724188155 - 0.16425928481378199 * s.b_r[47].abs());
                input_sample_r -= s.b_r[48] * (0.41067765934041811 - 0.10390115786636855 * s.b_r[48].abs());
                input_sample_r -= s.b_r[49] * (0.34409235547165967 - 0.07268159377411920 * s.b_r[49].abs());
                input_sample_r -= s.b_r[50] * (0.26542883122568151 - 0.05483457497365785 * s.b_r[50].abs());
                input_sample_r -= s.b_r[51] * (0.22024754776138800 - 0.06484897950087598 * s.b_r[51].abs());
                input_sample_r -= s.b_r[52] * (0.20394367993632415 - 0.08746309731952180 * s.b_r[52].abs());
                input_sample_r -= s.b_r[53] * (0.17565242431124092 - 0.07611309538078760 * s.b_r[53].abs());
                input_sample_r -= s.b_r[54] * (0.10116623231246825 - 0.00642818706295112 * s.b_r[54].abs());
                input_sample_r -= s.b_r[55] * (0.00782648272053632 + 0.08004141267685004 * s.b_r[55].abs());
                input_sample_r += s.b_r[56] * (0.05059046006747323 - 0.12436676387548490 * s.b_r[56].abs());
                input_sample_r += s.b_r[57] * (0.06241531553254467 - 0.11530779547021434 * s.b_r[57].abs());
                input_sample_r += s.b_r[58] * (0.04952694587101836 - 0.08340945324333944 * s.b_r[58].abs());
                input_sample_r += s.b_r[59] * (0.00843873294401687 - 0.03279659052562903 * s.b_r[59].abs());
                input_sample_r -= s.b_r[60] * (0.05161338949440241 - 0.03428181149163798 * s.b_r[60].abs());
                input_sample_r -= s.b_r[61] * (0.08165520146902012 - 0.08196746092283110 * s.b_r[61].abs());
                input_sample_r -= s.b_r[62] * (0.06639532849935320 - 0.09797462781896329 * s.b_r[62].abs());
                input_sample_r -= s.b_r[63] * (0.02953430910661621 - 0.09175612938515763 * s.b_r[63].abs());
                input_sample_r += s.b_r[64] * (0.00741058547442938 + 0.05442091048731967 * s.b_r[64].abs());
                input_sample_r += s.b_r[65] * (0.01832866125391727 + 0.00306243693643687 * s.b_r[65].abs());
                input_sample_r += s.b_r[66] * (0.00526964230373573 - 0.04364102661136410 * s.b_r[66].abs());
                input_sample_r -= s.b_r[67] * (0.00300984373848200 + 0.09742737841278880 * s.b_r[67].abs());
                input_sample_r -= s.b_r[68] * (0.00413616769576694 + 0.14380661694523073 * s.b_r[68].abs());
                input_sample_r -= s.b_r[69] * (0.00588769034931419 + 0.16012843578892538 * s.b_r[69].abs());
                input_sample_r -= s.b_r[70] * (0.00688588239450581 + 0.14074464279305798 * s.b_r[70].abs());
                input_sample_r -= s.b_r[71] * (0.02277307992926315 + 0.07914752191801366 * s.b_r[71].abs());
                input_sample_r -= s.b_r[72] * (0.04627166091180877 - 0.00192787268067208 * s.b_r[72].abs());
                input_sample_r -= s.b_r[73] * (0.05562045897455786 - 0.05932868727665747 * s.b_r[73].abs());
                input_sample_r -= s.b_r[74] * (0.05134243784922165 - 0.08245334798868090 * s.b_r[74].abs());
                input_sample_r -= s.b_r[75] * (0.04719409472239919 - 0.07498680629253825 * s.b_r[75].abs());
                input_sample_r -= s.b_r[76] * (0.05889738914266415 - 0.06116127018043697 * s.b_r[76].abs());
                input_sample_r -= s.b_r[77] * (0.09428363535111127 - 0.06535868867863834 * s.b_r[77].abs());
                input_sample_r -= s.b_r[78] * (0.15181756953225126 - 0.08982979655234427 * s.b_r[78].abs());
                input_sample_r -= s.b_r[79] * (0.20878969456036670 - 0.10761070891499538 * s.b_r[79].abs());
                input_sample_r -= s.b_r[80] * (0.22647885581813790 - 0.08462542510349125 * s.b_r[80].abs());
                input_sample_r -= s.b_r[81] * (0.19723482443646323 - 0.02665160920736287 * s.b_r[81].abs());
                input_sample_r -= s.b_r[82] * (0.16441643451155163 + 0.02314691954338197 * s.b_r[82].abs());
                input_sample_r -= s.b_r[83] * (0.15201914054931515 + 0.04424903493886839 * s.b_r[83].abs());
                input_sample_r -= s.b_r[84] * (0.15454370641307855 + 0.04223203797913008 * s.b_r[84].abs());
                temp = (input_sample_r + s.smooth_cab_br) / 3.0;
                s.smooth_cab_br = input_sample_r;
                input_sample_r = temp / 4.0;
                randy = s.fpd_r as f64 / u32::MAX as f64 * 0.057;
                dry_sample_r = ((input_sample_r * (1.0 - randy) + s.last_cab_sample_r * randy) * wet + dry_sample_r * (1.0 - wet)) * outputlevel;
                s.last_cab_sample_r = input_sample_r;
                input_sample_r = dry_sample_r;
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
            'sw1: {
                let sw: i32 = cycle_end;
                let mut ft = false;
                if ft || sw == 4 {
                    ft = true;
                    s.last_ref_l[8] = input_sample_l;
                    input_sample_l = (input_sample_l + s.last_ref_l[7]) * 0.5;
                    s.last_ref_l[7] = s.last_ref_l[8];
                    s.last_ref_r[8] = input_sample_r;
                    input_sample_r = (input_sample_r + s.last_ref_r[7]) * 0.5;
                    s.last_ref_r[7] = s.last_ref_r[8];
                }
                if ft || sw == 3 {
                    ft = true;
                    s.last_ref_l[8] = input_sample_l;
                    input_sample_l = (input_sample_l + s.last_ref_l[6]) * 0.5;
                    s.last_ref_l[6] = s.last_ref_l[8];
                    s.last_ref_r[8] = input_sample_r;
                    input_sample_r = (input_sample_r + s.last_ref_r[6]) * 0.5;
                    s.last_ref_r[6] = s.last_ref_r[8];
                }
                if ft || sw == 2 {
                    ft = true;
                    s.last_ref_l[8] = input_sample_l;
                    input_sample_l = (input_sample_l + s.last_ref_l[5]) * 0.5;
                    s.last_ref_l[5] = s.last_ref_l[8];
                    s.last_ref_r[8] = input_sample_r;
                    input_sample_r = (input_sample_r + s.last_ref_r[5]) * 0.5;
                    s.last_ref_r[5] = s.last_ref_r[8];
                }
                if ft || sw == 1 {
                    ft = true;
                    break 'sw1;
                }
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
