// Ported from BussColors4 (BussColors4Proc.cpp, BussColors4.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Buss Colors (sub-effect id 44): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    SlotDef::choice("Color", &["Dark", "Rock", "Lush", "Vibe", "Holo", "Punch", "Steel", "Tube"], 0.0),
    SlotDef::float("Input Trim", 0.0, 1.0, 0.5, crate::kit::core::SlotUnit::None),
    SlotDef::float("Output Trim", 0.0, 1.0, 0.5, crate::kit::core::SlotUnit::None),
    SlotDef::float("Dry/Wet", 0.0, 1.0, 1.0, crate::kit::core::SlotUnit::None),
];

#[allow(dead_code)]
pub struct BussColors4 {
    p: [f32; 4],
    sr: f32,
    b_l: Vec<f64>,
    b_r: Vec<f64>,
    d_l: Vec<f64>,
    d_r: Vec<f64>,
    c: [i32; 35],
    g: [f64; 9],
    outg: [f64; 9],
    control_l: f64,
    control_r: f64,
    slowdyn_l: f64,
    slowdyn_r: f64,
    gcount: i32,
    fpd_l: u32,
    fpd_r: u32,
}

impl BussColors4 {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<BussColors4> {
        let mut s = Box::new(BussColors4 {
            p: [0.0, 0.5, 0.5, 1.0],
            sr: sr as f32,
            b_l: vec![0.0; 175],
            b_r: vec![0.0; 175],
            d_l: vec![0.0; 100],
            d_r: vec![0.0; 100],
            c: [0; 35],
            g: [0.0; 9],
            outg: [0.0; 9],
            control_l: 0.0,
            control_r: 0.0,
            slowdyn_l: 0.0,
            slowdyn_r: 0.0,
            gcount: 0,
            fpd_l: 0,
            fpd_r: 0,
        });
        {
            let mut count: i32 = 0;
            while count < 34 {
                s.c[count as usize] = count;
                count += 1;
            }
        }
        s.g[1] = 10.0_f64.powf(-5.2 / 14.0);
        s.g[2] = 10.0_f64.powf(-6.2 / 14.0);
        s.g[3] = 10.0_f64.powf(-2.9 / 14.0);
        s.g[4] = 10.0_f64.powf(-1.1 / 14.0);
        s.g[5] = 10.0_f64.powf(-5.1 / 14.0);
        s.g[6] = 10.0_f64.powf(-3.6 / 14.0);
        s.g[7] = 10.0_f64.powf(-2.3 / 14.0);
        s.g[8] = 10.0_f64.powf(-2.9 / 14.0);
        s.outg[1] = 10.0_f64.powf(-0.3 / 14.0);
        s.outg[2] = 10.0_f64.powf(0.5 / 14.0);
        s.outg[3] = 10.0_f64.powf(-0.7 / 14.0);
        s.outg[4] = 10.0_f64.powf(-0.6 / 14.0);
        s.outg[5] = 10.0_f64.powf(-0.2 / 14.0);
        s.outg[6] = 10.0_f64.powf(0.3 / 14.0);
        s.outg[7] = 10.0_f64.powf(0.1 / 14.0);
        s.outg[8] = 10.0_f64.powf(0.9 / 14.0);
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for BussColors4 {
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
        if overallscale < 1.0 {
            overallscale = 1.0;
        }
        if overallscale > 4.5 {
            overallscale = 4.5;
        }
        let mut max_convolution_buffer_size: i32 = (34.0 * overallscale) as i32;
        {
            let mut count: i32 = 0;
            while count < 34 {
                s.c[count as usize] = (count as f64 * overallscale) as i32;
                count += 1;
            }
        }
        let mut dry_sample_l: f64 = 0.0;
        let mut dry_sample_r: f64 = 0.0;
        let mut applyconv_l: f64 = 0.0;
        let mut applyconv_r: f64 = 0.0;
        let mut offset_a: i32 = 3;
        let mut dynamicconv_l: f64 = 3.0;
        let mut dynamicconv_r: f64 = 3.0;
        let mut gain: f64 = 0.436;
        let mut outgain: f64 = 1.0;
        let mut bridgerectifier: f64 = 0.0;
        let mut input_sample_l: f64 = 0.0;
        let mut input_sample_r: f64 = 0.0;
        let mut console: i32 = (s.p[0] as f64 * 7.999) as i32 + 1;
        match console {
            1 => {
                offset_a = 4;
                gain = s.g[1];
                outgain = s.outg[1];
            }
            2 => {
                offset_a = 3;
                gain = s.g[2];
                outgain = s.outg[2];
            }
            3 => {
                offset_a = 5;
                gain = s.g[3];
                outgain = s.outg[3];
            }
            4 => {
                offset_a = 8;
                gain = s.g[4];
                outgain = s.outg[4];
            }
            5 => {
                offset_a = 5;
                gain = s.g[5];
                outgain = s.outg[5];
            }
            6 => {
                offset_a = 7;
                gain = s.g[6];
                outgain = s.outg[6];
            }
            7 => {
                offset_a = 7;
                gain = s.g[7];
                outgain = s.outg[7];
            }
            8 => {
                offset_a = 6;
                gain = s.g[8];
                outgain = s.outg[8];
            }
            _ => {}
        }
        offset_a = (offset_a as f64 * overallscale) as i32;
        if offset_a < 3 {
            offset_a = 3;
        }
        gain *= 10.0_f64.powf((s.p[1] as f64 * 36.0 - 18.0) / 14.0);
        outgain *= 10.0_f64.powf((s.p[2] as f64 * 36.0 - 18.0 + 3.3) / 14.0);
        let mut wet: f64 = s.p[3] as f64;
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
            if gain != 1.0 {
                input_sample_l *= gain;
                input_sample_r *= gain;
            }
            bridgerectifier = input_sample_l.abs();
            s.slowdyn_l *= 0.999;
            s.slowdyn_l += bridgerectifier;
            if s.slowdyn_l > 1.5 {
                s.slowdyn_l = 1.5;
            }
            dynamicconv_l = 2.5 + s.slowdyn_l;
            if bridgerectifier > 1.57079633 {
                bridgerectifier = 1.0;
            } else {
                bridgerectifier = bridgerectifier.sin();
            }
            if input_sample_l > 0.0 {
                input_sample_l = bridgerectifier;
            } else {
                input_sample_l = -bridgerectifier;
            }
            bridgerectifier = input_sample_r.abs();
            s.slowdyn_r *= 0.999;
            s.slowdyn_r += bridgerectifier;
            if s.slowdyn_r > 1.5 {
                s.slowdyn_r = 1.5;
            }
            dynamicconv_r = 2.5 + s.slowdyn_r;
            if bridgerectifier > 1.57079633 {
                bridgerectifier = 1.0;
            } else {
                bridgerectifier = bridgerectifier.sin();
            }
            if input_sample_r > 0.0 {
                input_sample_r = bridgerectifier;
            } else {
                input_sample_r = -bridgerectifier;
            }
            if s.gcount < 0 || s.gcount > 44 {
                s.gcount = 44;
            }
            s.d_l[s.gcount as usize] = input_sample_l.abs();
            s.d_l[(s.gcount + 44) as usize] = s.d_l[s.gcount as usize];
            s.control_l += s.d_l[s.gcount as usize] / offset_a as f64;
            s.control_l -= s.d_l[(s.gcount + offset_a) as usize] / offset_a as f64;
            s.control_l -= 0.000001;
            if s.control_l < 0.0 {
                s.control_l = 0.0;
            }
            if s.control_l > 100.0 {
                s.control_l = 100.0;
            }
            applyconv_l = s.control_l / offset_a as f64 * dynamicconv_l;
            s.d_r[s.gcount as usize] = input_sample_r.abs();
            s.d_r[(s.gcount + 44) as usize] = s.d_r[s.gcount as usize];
            s.control_r += s.d_r[s.gcount as usize] / offset_a as f64;
            s.control_r -= s.d_r[(s.gcount + offset_a) as usize] / offset_a as f64;
            s.control_r -= 0.000001;
            if s.control_r < 0.0 {
                s.control_r = 0.0;
            }
            if s.control_r > 100.0 {
                s.control_r = 100.0;
            }
            applyconv_r = s.control_r / offset_a as f64 * dynamicconv_r;
            s.gcount -= 1;
            {
                let mut count: i32 = max_convolution_buffer_size;
                while count > 0 {
                    s.b_l[count as usize] = s.b_l[(count - 1) as usize];
                    count -= 1;
                }
            }
            s.b_l[0] = input_sample_l;
            {
                let mut count: i32 = max_convolution_buffer_size;
                while count > 0 {
                    s.b_r[count as usize] = s.b_r[(count - 1) as usize];
                    count -= 1;
                }
            }
            s.b_r[0] = input_sample_r;
            match console {
                1 => {
                    input_sample_l += s.b_l[s.c[1] as usize] * (0.61283288942201319 + 0.00024011410669522 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[2] as usize] * (0.24036380659761222 - 0.00020789518206241 * applyconv_l);
                    input_sample_l += s.b_l[s.c[3] as usize] * (0.09104669761717916 + 0.00012829642741548 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[4] as usize] * (0.02378290768554025 - 0.00017673646470440 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[5] as usize] * (0.02832818490275965 - 0.00013536187747384 * applyconv_l);
                    input_sample_l += s.b_l[s.c[6] as usize] * (0.03268797679215937 + 0.00015035126653359 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[7] as usize] * (0.04024464202655586 - 0.00015034923056735 * applyconv_l);
                    input_sample_l += s.b_l[s.c[8] as usize] * (0.01864890074318696 + 0.00014513281680642 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[9] as usize] * (0.01632731954100322 - 0.00015509089075614 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[10] as usize] * (0.00318907090555589 - 0.00014784812076550 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[11] as usize] * (0.00208573465221869 - 0.00015350520779465 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[12] as usize] * (0.00907033901519614 - 0.00015442964157250 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[13] as usize] * (0.00199458794148013 - 0.00015595640046297 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[14] as usize] * (0.00705979153201755 - 0.00015730069418051 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[15] as usize] * (0.00429488975412722 - 0.00015743697943505 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[16] as usize] * (0.00497724878704936 - 0.00016014760011861 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[17] as usize] * (0.00506059305562353 - 0.00016194824072466 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[18] as usize] * (0.00483432223285621 - 0.00016329050124225 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[19] as usize] * (0.00495100420886005 - 0.00016297509798749 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[20] as usize] * (0.00489319520555115 - 0.00016472839684661 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[21] as usize] * (0.00489177657970308 - 0.00016791875866630 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[22] as usize] * (0.00487900894707044 - 0.00016755993898534 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[23] as usize] * (0.00486234009335561 - 0.00016968157345446 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[24] as usize] * (0.00485737490288736 - 0.00017180713324431 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[25] as usize] * (0.00484106070563455 - 0.00017251073661092 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[26] as usize] * (0.00483219429408410 - 0.00017321683790891 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[27] as usize] * (0.00482013597437550 - 0.00017392186866488 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[28] as usize] * (0.00480949628051497 - 0.00017569098775602 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[29] as usize] * (0.00479992055604049 - 0.00017746046369449 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[30] as usize] * (0.00478750757986987 - 0.00017745630047554 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[31] as usize] * (0.00477828651185740 - 0.00017958043287604 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[32] as usize] * (0.00476906544384494 - 0.00018170456527653 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[33] as usize] * (0.00475700712413634 - 0.00018099144598088 * applyconv_l);
                }
                2 => {
                    input_sample_l += s.b_l[s.c[1] as usize] * (0.67887916185274055 + 0.00068787552301086 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[2] as usize] * (0.25671050678827934 + 0.00017691749454490 * applyconv_l);
                    input_sample_l += s.b_l[s.c[3] as usize] * (0.15135839896615280 + 0.00007481480365043 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[4] as usize] * (0.11813512969090802 + 0.00005191138121359 * applyconv_l);
                    input_sample_l += s.b_l[s.c[5] as usize] * (0.08329104347166429 + 0.00001871054659794 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[6] as usize] * (0.07663817456103936 + 0.00002751359071705 * applyconv_l);
                    input_sample_l += s.b_l[s.c[7] as usize] * (0.05477586152148759 + 0.00000744843212679 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[8] as usize] * (0.05547314737187786 + 0.00001025289931145 * applyconv_l);
                    input_sample_l += s.b_l[s.c[9] as usize] * (0.03822948356540711 - 0.00000249791561457 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[10] as usize] * (0.04199383340841713 - 0.00000067328840674 * applyconv_l);
                    input_sample_l += s.b_l[s.c[11] as usize] * (0.02695796542339694 - 0.00000796704606548 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[12] as usize] * (0.03228715059431878 - 0.00000579711816722 * applyconv_l);
                    input_sample_l += s.b_l[s.c[13] as usize] * (0.01846929689819187 - 0.00000984017804950 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[14] as usize] * (0.02528050435045951 - 0.00000701189792484 * applyconv_l);
                    input_sample_l += s.b_l[s.c[15] as usize] * (0.01207844846859765 - 0.00001522630289356 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[16] as usize] * (0.01894464378378515 - 0.00001205456372080 * applyconv_l);
                    input_sample_l += s.b_l[s.c[17] as usize] * (0.00667804407593324 - 0.00001343604283817 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[18] as usize] * (0.01408418045473130 - 0.00001246443581504 * applyconv_l);
                    input_sample_l += s.b_l[s.c[19] as usize] * (0.00228696509481569 - 0.00001506764046927 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[20] as usize] * (0.01006277891348454 - 0.00000970723079112 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[21] as usize] * (0.00132368373546377 + 0.00001188847238761 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[22] as usize] * (0.00676615715578373 - 0.00001209129844861 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[23] as usize] * (0.00426288438418556 + 0.00001286836943559 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[24] as usize] * (0.00408897698639688 - 0.00001102542567911 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[25] as usize] * (0.00662040619382751 + 0.00001206328529063 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[26] as usize] * (0.00196101294183599 - 0.00000950703614981 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[27] as usize] * (0.00845620581010342 + 0.00001279970295678 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[28] as usize] * (0.00032595215043616 - 0.00000920518241371 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[29] as usize] * (0.00982957737435458 + 0.00001177745362317 * applyconv_l);
                    input_sample_l += s.b_l[s.c[30] as usize] * (0.00086920573760513 + 0.00000913758382404 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[31] as usize] * (0.01079020871452061 + 0.00000900750153697 * applyconv_l);
                    input_sample_l += s.b_l[s.c[32] as usize] * (0.00167613606334460 + 0.00000732769151038 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[33] as usize] * (0.01138050011044332 + 0.00000946908207442 * applyconv_l);
                }
                3 => {
                    input_sample_l += s.b_l[s.c[1] as usize] * (0.20641602693167951 - 0.00078952185394898 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[2] as usize] * (0.07601816702459827 + 0.00022786334179951 * applyconv_l);
                    input_sample_l += s.b_l[s.c[3] as usize] * (0.03929765560019285 - 0.00054517993246352 * applyconv_l);
                    input_sample_l += s.b_l[s.c[4] as usize] * (0.00298333157711103 - 0.00033083756545638 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[5] as usize] * (0.00724006282304610 + 0.00045483683460812 * applyconv_l);
                    input_sample_l += s.b_l[s.c[6] as usize] * (0.03073108963506036 - 0.00038190060537423 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[7] as usize] * (0.02332434692533051 + 0.00040347288688932 * applyconv_l);
                    input_sample_l += s.b_l[s.c[8] as usize] * (0.03792606869061214 - 0.00039673687335892 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[9] as usize] * (0.02437059376675688 + 0.00037221210539535 * applyconv_l);
                    input_sample_l += s.b_l[s.c[10] as usize] * (0.03416764311979521 - 0.00040314850796953 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[11] as usize] * (0.01761669868102127 + 0.00035989484330131 * applyconv_l);
                    input_sample_l += s.b_l[s.c[12] as usize] * (0.02538237753523052 - 0.00040149119125394 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[13] as usize] * (0.00770737340728377 + 0.00035462118723555 * applyconv_l);
                    input_sample_l += s.b_l[s.c[14] as usize] * (0.01580706228482803 - 0.00037563141307594 * applyconv_l);
                    input_sample_l += s.b_l[s.c[15] as usize] * (0.00055119240005586 - 0.00035409299268971 * applyconv_l);
                    input_sample_l += s.b_l[s.c[16] as usize] * (0.00818552143438768 - 0.00036507661042180 * applyconv_l);
                    input_sample_l += s.b_l[s.c[17] as usize] * (0.00661842703548304 - 0.00034550528559056 * applyconv_l);
                    input_sample_l += s.b_l[s.c[18] as usize] * (0.00362447476272098 - 0.00035553012761240 * applyconv_l);
                    input_sample_l += s.b_l[s.c[19] as usize] * (0.00957098027225745 - 0.00034091691045338 * applyconv_l);
                    input_sample_l += s.b_l[s.c[20] as usize] * (0.00193621774016660 - 0.00034554529131668 * applyconv_l);
                    input_sample_l += s.b_l[s.c[21] as usize] * (0.01005433027357935 - 0.00033878223153845 * applyconv_l);
                    input_sample_l += s.b_l[s.c[22] as usize] * (0.00221712428802004 - 0.00033481410137711 * applyconv_l);
                    input_sample_l += s.b_l[s.c[23] as usize] * (0.00911255639207995 - 0.00033263425232666 * applyconv_l);
                    input_sample_l += s.b_l[s.c[24] as usize] * (0.00339667169034909 - 0.00032634428038430 * applyconv_l);
                    input_sample_l += s.b_l[s.c[25] as usize] * (0.00774096948249924 - 0.00032599868802996 * applyconv_l);
                    input_sample_l += s.b_l[s.c[26] as usize] * (0.00463907626773794 - 0.00032131993173361 * applyconv_l);
                    input_sample_l += s.b_l[s.c[27] as usize] * (0.00658222997260378 - 0.00032014977430211 * applyconv_l);
                    input_sample_l += s.b_l[s.c[28] as usize] * (0.00550347079924993 - 0.00031557153256653 * applyconv_l);
                    input_sample_l += s.b_l[s.c[29] as usize] * (0.00588754981375325 - 0.00032041307242303 * applyconv_l);
                    input_sample_l += s.b_l[s.c[30] as usize] * (0.00590293898419892 - 0.00030457857428714 * applyconv_l);
                    input_sample_l += s.b_l[s.c[31] as usize] * (0.00558952010441800 - 0.00030448053548086 * applyconv_l);
                    input_sample_l += s.b_l[s.c[32] as usize] * (0.00598183557634295 - 0.00030715064323181 * applyconv_l);
                    input_sample_l += s.b_l[s.c[33] as usize] * (0.00555223929714115 - 0.00030319367948553 * applyconv_l);
                }
                4 => {
                    input_sample_l -= s.b_l[s.c[1] as usize] * (0.25867935358656502 - 0.00045755657070112 * applyconv_l);
                    input_sample_l += s.b_l[s.c[2] as usize] * (0.11509367290253694 - 0.00017494270657228 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[3] as usize] * (0.06709853575891785 - 0.00058913102597723 * applyconv_l);
                    input_sample_l += s.b_l[s.c[4] as usize] * (0.01871006356851681 - 0.00003387358004645 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[5] as usize] * (0.00794797957360465 - 0.00044224784691203 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[6] as usize] * (0.01956921817394220 - 0.00006718936750076 * applyconv_l);
                    input_sample_l += s.b_l[s.c[7] as usize] * (0.01682120257195205 + 0.00032857446292230 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[8] as usize] * (0.03401069039824205 - 0.00013634182872897 * applyconv_l);
                    input_sample_l += s.b_l[s.c[9] as usize] * (0.02369950268232634 + 0.00023112685751657 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[10] as usize] * (0.03477071178117132 - 0.00018029792231600 * applyconv_l);
                    input_sample_l += s.b_l[s.c[11] as usize] * (0.02024369717958201 + 0.00017337813374202 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[12] as usize] * (0.02819087729102172 - 0.00021438538665420 * applyconv_l);
                    input_sample_l += s.b_l[s.c[13] as usize] * (0.01147946743141303 + 0.00014424066034649 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[14] as usize] * (0.01894777011468867 - 0.00021549146262408 * applyconv_l);
                    input_sample_l += s.b_l[s.c[15] as usize] * (0.00301370330346873 + 0.00013527460148394 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[16] as usize] * (0.01067147835815486 - 0.00020960689910868 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[17] as usize] * (0.00402715397506384 - 0.00014421582712470 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[18] as usize] * (0.00502221703392005 - 0.00019805767015024 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[19] as usize] * (0.00808788533308497 - 0.00016095444141931 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[20] as usize] * (0.00232696588842683 - 0.00018384470981829 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[21] as usize] * (0.00943950821324531 - 0.00017098987347593 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[22] as usize] * (0.00193709517200834 - 0.00018151995939591 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[23] as usize] * (0.00899713952612659 - 0.00017385835059948 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[24] as usize] * (0.00280584331659089 - 0.00017742164162470 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[25] as usize] * (0.00780381001954970 - 0.00018002500755708 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[26] as usize] * (0.00400370310490333 - 0.00017471691087957 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[27] as usize] * (0.00661527728186928 - 0.00018137323370347 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[28] as usize] * (0.00496545526864518 - 0.00017681872601767 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[29] as usize] * (0.00580728820997532 - 0.00018186220389790 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[30] as usize] * (0.00549309984725666 - 0.00017722985399075 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[31] as usize] * (0.00542194777529239 - 0.00018486900185338 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[32] as usize] * (0.00565992080998939 - 0.00018005824393118 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[33] as usize] * (0.00532121562846656 - 0.00018643189636216 * applyconv_l);
                }
                5 => {
                    input_sample_l += s.b_l[s.c[1] as usize] * (0.59188440274551890 - 0.00008361469668405 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[2] as usize] * (0.24439750948076133 + 0.00002651678396848 * applyconv_l);
                    input_sample_l += s.b_l[s.c[3] as usize] * (0.14109876103205621 - 0.00000840487181372 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[4] as usize] * (0.10053507128157971 + 0.00001768100964598 * applyconv_l);
                    input_sample_l += s.b_l[s.c[5] as usize] * (0.05859287880626238 - 0.00000361398065989 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[6] as usize] * (0.04337406889823660 + 0.00000735941182117 * applyconv_l);
                    input_sample_l += s.b_l[s.c[7] as usize] * (0.01589900680531097 + 0.00000207347387987 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[8] as usize] * (0.01087234854973281 + 0.00000732123412029 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[9] as usize] * (0.00845782429679176 - 0.00000133058605071 * applyconv_l);
                    input_sample_l += s.b_l[s.c[10] as usize] * (0.00662278586618295 - 0.00000424594730611 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[11] as usize] * (0.02000592193760155 + 0.00000632896879068 * applyconv_l);
                    input_sample_l += s.b_l[s.c[12] as usize] * (0.01321157777167565 - 0.00001421171592570 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[13] as usize] * (0.02249955362988238 + 0.00000163937127317 * applyconv_l);
                    input_sample_l += s.b_l[s.c[14] as usize] * (0.01196492077581504 - 0.00000535385220676 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[15] as usize] * (0.01905917427000097 + 0.00000121672882030 * applyconv_l);
                    input_sample_l += s.b_l[s.c[16] as usize] * (0.00761909482108073 - 0.00000326242895115 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[17] as usize] * (0.01362744780256239 + 0.00000359274216003 * applyconv_l);
                    input_sample_l += s.b_l[s.c[18] as usize] * (0.00200183122683721 - 0.00000089207452791 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[19] as usize] * (0.00833042637239315 + 0.00000946767677294 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[20] as usize] * (0.00258481175207224 - 0.00000087429351464 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[21] as usize] * (0.00459744479712244 - 0.00000049519758701 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[22] as usize] * (0.00534277030993820 + 0.00000397547847155 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[23] as usize] * (0.00272332919605675 + 0.00000040077229097 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[24] as usize] * (0.00637243782359372 - 0.00000139419072176 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[25] as usize] * (0.00233001590327504 + 0.00000420129915747 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[26] as usize] * (0.00623296727793041 + 0.00000019010664856 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[27] as usize] * (0.00276177096376805 + 0.00000580301901385 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[28] as usize] * (0.00559184754866264 + 0.00000080597287792 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[29] as usize] * (0.00343180144395919 - 0.00000243701142085 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[30] as usize] * (0.00493325428861701 + 0.00000300985740900 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[31] as usize] * (0.00396140827680823 - 0.00000051459681789 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[32] as usize] * (0.00448497879902493 + 0.00000744412841743 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[33] as usize] * (0.00425146888772076 - 0.00000082346016542 * applyconv_l);
                }
                6 => {
                    input_sample_l += s.b_l[s.c[1] as usize] * (0.09299870608542582 - 0.00009582362368873 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[2] as usize] * (0.11947847710741009 - 0.00004500891602770 * applyconv_l);
                    input_sample_l += s.b_l[s.c[3] as usize] * (0.09071606264761795 + 0.00005639498984741 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[4] as usize] * (0.08561982770836980 - 0.00004964855606916 * applyconv_l);
                    input_sample_l += s.b_l[s.c[5] as usize] * (0.06440549220820363 + 0.00002428052139507 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[6] as usize] * (0.05987991812840746 + 0.00000101867082290 * applyconv_l);
                    input_sample_l += s.b_l[s.c[7] as usize] * (0.03980233135839382 + 0.00003312430049041 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[8] as usize] * (0.03648402630896925 - 0.00002116186381142 * applyconv_l);
                    input_sample_l += s.b_l[s.c[9] as usize] * (0.01826860869525248 + 0.00003115110025396 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[10] as usize] * (0.01723968622495364 - 0.00002450634121718 * applyconv_l);
                    input_sample_l += s.b_l[s.c[11] as usize] * (0.00187588812316724 + 0.00002838206198968 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[12] as usize] * (0.00381796423957237 - 0.00003155815499462 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[13] as usize] * (0.00852092214496733 - 0.00001702651162392 * applyconv_l);
                    input_sample_l += s.b_l[s.c[14] as usize] * (0.00315560292270588 + 0.00002547861676047 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[15] as usize] * (0.01258630914496868 - 0.00004555319243213 * applyconv_l);
                    input_sample_l += s.b_l[s.c[16] as usize] * (0.00536435648963575 + 0.00001812393657101 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[17] as usize] * (0.01272975658159178 - 0.00004103775306121 * applyconv_l);
                    input_sample_l += s.b_l[s.c[18] as usize] * (0.00403818975172755 + 0.00003764615492871 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[19] as usize] * (0.01042617366897483 - 0.00003605210426041 * applyconv_l);
                    input_sample_l += s.b_l[s.c[20] as usize] * (0.00126599583390057 + 0.00004305458668852 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[21] as usize] * (0.00747876207688339 - 0.00003731207018977 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[22] as usize] * (0.00149873689175324 - 0.00005086601800791 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[23] as usize] * (0.00503221309488033 - 0.00003636086782783 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[24] as usize] * (0.00342998224655821 - 0.00004103091180506 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[25] as usize] * (0.00355585977903117 - 0.00003698982145400 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[26] as usize] * (0.00437201792934817 - 0.00002720235666939 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[27] as usize] * (0.00299217874451556 - 0.00004446954727956 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[28] as usize] * (0.00457924652487249 - 0.00003859065778860 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[29] as usize] * (0.00298182934892027 - 0.00002064710931733 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[30] as usize] * (0.00438838441540584 - 0.00005223008424866 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[31] as usize] * (0.00323984218794705 - 0.00003397987535887 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[32] as usize] * (0.00407693981307314 - 0.00003935772436894 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[33] as usize] * (0.00350435348467321 - 0.00005525463935338 * applyconv_l);
                }
                7 => {
                    input_sample_l -= s.b_l[s.c[1] as usize] * (0.23505923670562212 - 0.00028312859289245 * applyconv_l);
                    input_sample_l += s.b_l[s.c[2] as usize] * (0.08188436704577637 - 0.00008817721351341 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[3] as usize] * (0.05075798481700617 - 0.00018817166632483 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[4] as usize] * (0.00455811821873093 + 0.00001922902995296 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[5] as usize] * (0.00027610521433660 - 0.00013252525469291 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[6] as usize] * (0.03529246280346626 - 0.00002772989223299 * applyconv_l);
                    input_sample_l += s.b_l[s.c[7] as usize] * (0.01784111585586136 + 0.00010230276997291 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[8] as usize] * (0.04394950700298298 - 0.00005910607126944 * applyconv_l);
                    input_sample_l += s.b_l[s.c[9] as usize] * (0.01990770780547606 + 0.00007640328340556 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[10] as usize] * (0.04073629569741782 - 0.00007712327117090 * applyconv_l);
                    input_sample_l += s.b_l[s.c[11] as usize] * (0.01349648572795252 + 0.00005959130575917 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[12] as usize] * (0.03191590248003717 - 0.00008418000575151 * applyconv_l);
                    input_sample_l += s.b_l[s.c[13] as usize] * (0.00348795527924766 + 0.00005489156318238 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[14] as usize] * (0.02198496281481767 - 0.00008471601187581 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[15] as usize] * (0.00504771152505089 - 0.00005525060587917 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[16] as usize] * (0.01391075698598491 - 0.00007929630732607 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[17] as usize] * (0.01142762504081717 - 0.00005967036737742 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[18] as usize] * (0.00893541815021255 - 0.00007535697758141 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[19] as usize] * (0.01459704973464936 - 0.00005969199602841 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[20] as usize] * (0.00694755135226282 - 0.00006930127097865 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[21] as usize] * (0.01516695630808575 - 0.00006365800069826 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[22] as usize] * (0.00705917318113651 - 0.00006497209096539 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[23] as usize] * (0.01420501209177591 - 0.00006555654576113 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[24] as usize] * (0.00815905656808701 - 0.00006105622534761 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[25] as usize] * (0.01274326525552961 - 0.00006542652857017 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[26] as usize] * (0.00937146927845488 - 0.00006051267868722 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[27] as usize] * (0.01146573981165209 - 0.00006381511607749 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[28] as usize] * (0.01021294359409007 - 0.00005930397856398 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[29] as usize] * (0.01065217095323532 - 0.00006371505438319 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[30] as usize] * (0.01058751196699751 - 0.00006042857480233 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[31] as usize] * (0.01026557827762401 - 0.00006007776163871 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[32] as usize] * (0.01060929183604604 - 0.00006114703012726 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[33] as usize] * (0.01014533525058528 - 0.00005963567932887 * applyconv_l);
                }
                8 => {
                    input_sample_l -= s.b_l[s.c[1] as usize] * (0.20641602693167951 - 0.00078952185394898 * applyconv_l);
                    input_sample_l += s.b_l[s.c[2] as usize] * (0.07601816702459827 + 0.00022786334179951 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[3] as usize] * (0.03929765560019285 - 0.00054517993246352 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[4] as usize] * (0.00298333157711103 - 0.00033083756545638 * applyconv_l);
                    input_sample_l += s.b_l[s.c[5] as usize] * (0.00724006282304610 + 0.00045483683460812 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[6] as usize] * (0.03073108963506036 - 0.00038190060537423 * applyconv_l);
                    input_sample_l += s.b_l[s.c[7] as usize] * (0.02332434692533051 + 0.00040347288688932 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[8] as usize] * (0.03792606869061214 - 0.00039673687335892 * applyconv_l);
                    input_sample_l += s.b_l[s.c[9] as usize] * (0.02437059376675688 + 0.00037221210539535 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[10] as usize] * (0.03416764311979521 - 0.00040314850796953 * applyconv_l);
                    input_sample_l += s.b_l[s.c[11] as usize] * (0.01761669868102127 + 0.00035989484330131 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[12] as usize] * (0.02538237753523052 - 0.00040149119125394 * applyconv_l);
                    input_sample_l += s.b_l[s.c[13] as usize] * (0.00770737340728377 + 0.00035462118723555 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[14] as usize] * (0.01580706228482803 - 0.00037563141307594 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[15] as usize] * (0.00055119240005586 - 0.00035409299268971 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[16] as usize] * (0.00818552143438768 - 0.00036507661042180 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[17] as usize] * (0.00661842703548304 - 0.00034550528559056 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[18] as usize] * (0.00362447476272098 - 0.00035553012761240 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[19] as usize] * (0.00957098027225745 - 0.00034091691045338 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[20] as usize] * (0.00193621774016660 - 0.00034554529131668 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[21] as usize] * (0.01005433027357935 - 0.00033878223153845 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[22] as usize] * (0.00221712428802004 - 0.00033481410137711 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[23] as usize] * (0.00911255639207995 - 0.00033263425232666 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[24] as usize] * (0.00339667169034909 - 0.00032634428038430 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[25] as usize] * (0.00774096948249924 - 0.00032599868802996 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[26] as usize] * (0.00463907626773794 - 0.00032131993173361 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[27] as usize] * (0.00658222997260378 - 0.00032014977430211 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[28] as usize] * (0.00550347079924993 - 0.00031557153256653 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[29] as usize] * (0.00588754981375325 - 0.00032041307242303 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[30] as usize] * (0.00590293898419892 - 0.00030457857428714 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[31] as usize] * (0.00558952010441800 - 0.00030448053548086 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[32] as usize] * (0.00598183557634295 - 0.00030715064323181 * applyconv_l);
                    input_sample_l -= s.b_l[s.c[33] as usize] * (0.00555223929714115 - 0.00030319367948553 * applyconv_l);
                }
                _ => {}
            }
            match console {
                1 => {
                    input_sample_r += s.b_r[s.c[1] as usize] * (0.61283288942201319 + 0.00024011410669522 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[2] as usize] * (0.24036380659761222 - 0.00020789518206241 * applyconv_r);
                    input_sample_r += s.b_r[s.c[3] as usize] * (0.09104669761717916 + 0.00012829642741548 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[4] as usize] * (0.02378290768554025 - 0.00017673646470440 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[5] as usize] * (0.02832818490275965 - 0.00013536187747384 * applyconv_r);
                    input_sample_r += s.b_r[s.c[6] as usize] * (0.03268797679215937 + 0.00015035126653359 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[7] as usize] * (0.04024464202655586 - 0.00015034923056735 * applyconv_r);
                    input_sample_r += s.b_r[s.c[8] as usize] * (0.01864890074318696 + 0.00014513281680642 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[9] as usize] * (0.01632731954100322 - 0.00015509089075614 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[10] as usize] * (0.00318907090555589 - 0.00014784812076550 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[11] as usize] * (0.00208573465221869 - 0.00015350520779465 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[12] as usize] * (0.00907033901519614 - 0.00015442964157250 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[13] as usize] * (0.00199458794148013 - 0.00015595640046297 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[14] as usize] * (0.00705979153201755 - 0.00015730069418051 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[15] as usize] * (0.00429488975412722 - 0.00015743697943505 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[16] as usize] * (0.00497724878704936 - 0.00016014760011861 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[17] as usize] * (0.00506059305562353 - 0.00016194824072466 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[18] as usize] * (0.00483432223285621 - 0.00016329050124225 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[19] as usize] * (0.00495100420886005 - 0.00016297509798749 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[20] as usize] * (0.00489319520555115 - 0.00016472839684661 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[21] as usize] * (0.00489177657970308 - 0.00016791875866630 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[22] as usize] * (0.00487900894707044 - 0.00016755993898534 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[23] as usize] * (0.00486234009335561 - 0.00016968157345446 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[24] as usize] * (0.00485737490288736 - 0.00017180713324431 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[25] as usize] * (0.00484106070563455 - 0.00017251073661092 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[26] as usize] * (0.00483219429408410 - 0.00017321683790891 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[27] as usize] * (0.00482013597437550 - 0.00017392186866488 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[28] as usize] * (0.00480949628051497 - 0.00017569098775602 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[29] as usize] * (0.00479992055604049 - 0.00017746046369449 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[30] as usize] * (0.00478750757986987 - 0.00017745630047554 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[31] as usize] * (0.00477828651185740 - 0.00017958043287604 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[32] as usize] * (0.00476906544384494 - 0.00018170456527653 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[33] as usize] * (0.00475700712413634 - 0.00018099144598088 * applyconv_r);
                }
                2 => {
                    input_sample_r += s.b_r[s.c[1] as usize] * (0.67887916185274055 + 0.00068787552301086 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[2] as usize] * (0.25671050678827934 + 0.00017691749454490 * applyconv_r);
                    input_sample_r += s.b_r[s.c[3] as usize] * (0.15135839896615280 + 0.00007481480365043 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[4] as usize] * (0.11813512969090802 + 0.00005191138121359 * applyconv_r);
                    input_sample_r += s.b_r[s.c[5] as usize] * (0.08329104347166429 + 0.00001871054659794 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[6] as usize] * (0.07663817456103936 + 0.00002751359071705 * applyconv_r);
                    input_sample_r += s.b_r[s.c[7] as usize] * (0.05477586152148759 + 0.00000744843212679 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[8] as usize] * (0.05547314737187786 + 0.00001025289931145 * applyconv_r);
                    input_sample_r += s.b_r[s.c[9] as usize] * (0.03822948356540711 - 0.00000249791561457 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[10] as usize] * (0.04199383340841713 - 0.00000067328840674 * applyconv_r);
                    input_sample_r += s.b_r[s.c[11] as usize] * (0.02695796542339694 - 0.00000796704606548 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[12] as usize] * (0.03228715059431878 - 0.00000579711816722 * applyconv_r);
                    input_sample_r += s.b_r[s.c[13] as usize] * (0.01846929689819187 - 0.00000984017804950 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[14] as usize] * (0.02528050435045951 - 0.00000701189792484 * applyconv_r);
                    input_sample_r += s.b_r[s.c[15] as usize] * (0.01207844846859765 - 0.00001522630289356 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[16] as usize] * (0.01894464378378515 - 0.00001205456372080 * applyconv_r);
                    input_sample_r += s.b_r[s.c[17] as usize] * (0.00667804407593324 - 0.00001343604283817 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[18] as usize] * (0.01408418045473130 - 0.00001246443581504 * applyconv_r);
                    input_sample_r += s.b_r[s.c[19] as usize] * (0.00228696509481569 - 0.00001506764046927 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[20] as usize] * (0.01006277891348454 - 0.00000970723079112 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[21] as usize] * (0.00132368373546377 + 0.00001188847238761 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[22] as usize] * (0.00676615715578373 - 0.00001209129844861 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[23] as usize] * (0.00426288438418556 + 0.00001286836943559 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[24] as usize] * (0.00408897698639688 - 0.00001102542567911 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[25] as usize] * (0.00662040619382751 + 0.00001206328529063 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[26] as usize] * (0.00196101294183599 - 0.00000950703614981 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[27] as usize] * (0.00845620581010342 + 0.00001279970295678 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[28] as usize] * (0.00032595215043616 - 0.00000920518241371 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[29] as usize] * (0.00982957737435458 + 0.00001177745362317 * applyconv_r);
                    input_sample_r += s.b_r[s.c[30] as usize] * (0.00086920573760513 + 0.00000913758382404 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[31] as usize] * (0.01079020871452061 + 0.00000900750153697 * applyconv_r);
                    input_sample_r += s.b_r[s.c[32] as usize] * (0.00167613606334460 + 0.00000732769151038 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[33] as usize] * (0.01138050011044332 + 0.00000946908207442 * applyconv_r);
                }
                3 => {
                    input_sample_r += s.b_r[s.c[1] as usize] * (0.20641602693167951 - 0.00078952185394898 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[2] as usize] * (0.07601816702459827 + 0.00022786334179951 * applyconv_r);
                    input_sample_r += s.b_r[s.c[3] as usize] * (0.03929765560019285 - 0.00054517993246352 * applyconv_r);
                    input_sample_r += s.b_r[s.c[4] as usize] * (0.00298333157711103 - 0.00033083756545638 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[5] as usize] * (0.00724006282304610 + 0.00045483683460812 * applyconv_r);
                    input_sample_r += s.b_r[s.c[6] as usize] * (0.03073108963506036 - 0.00038190060537423 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[7] as usize] * (0.02332434692533051 + 0.00040347288688932 * applyconv_r);
                    input_sample_r += s.b_r[s.c[8] as usize] * (0.03792606869061214 - 0.00039673687335892 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[9] as usize] * (0.02437059376675688 + 0.00037221210539535 * applyconv_r);
                    input_sample_r += s.b_r[s.c[10] as usize] * (0.03416764311979521 - 0.00040314850796953 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[11] as usize] * (0.01761669868102127 + 0.00035989484330131 * applyconv_r);
                    input_sample_r += s.b_r[s.c[12] as usize] * (0.02538237753523052 - 0.00040149119125394 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[13] as usize] * (0.00770737340728377 + 0.00035462118723555 * applyconv_r);
                    input_sample_r += s.b_r[s.c[14] as usize] * (0.01580706228482803 - 0.00037563141307594 * applyconv_r);
                    input_sample_r += s.b_r[s.c[15] as usize] * (0.00055119240005586 - 0.00035409299268971 * applyconv_r);
                    input_sample_r += s.b_r[s.c[16] as usize] * (0.00818552143438768 - 0.00036507661042180 * applyconv_r);
                    input_sample_r += s.b_r[s.c[17] as usize] * (0.00661842703548304 - 0.00034550528559056 * applyconv_r);
                    input_sample_r += s.b_r[s.c[18] as usize] * (0.00362447476272098 - 0.00035553012761240 * applyconv_r);
                    input_sample_r += s.b_r[s.c[19] as usize] * (0.00957098027225745 - 0.00034091691045338 * applyconv_r);
                    input_sample_r += s.b_r[s.c[20] as usize] * (0.00193621774016660 - 0.00034554529131668 * applyconv_r);
                    input_sample_r += s.b_r[s.c[21] as usize] * (0.01005433027357935 - 0.00033878223153845 * applyconv_r);
                    input_sample_r += s.b_r[s.c[22] as usize] * (0.00221712428802004 - 0.00033481410137711 * applyconv_r);
                    input_sample_r += s.b_r[s.c[23] as usize] * (0.00911255639207995 - 0.00033263425232666 * applyconv_r);
                    input_sample_r += s.b_r[s.c[24] as usize] * (0.00339667169034909 - 0.00032634428038430 * applyconv_r);
                    input_sample_r += s.b_r[s.c[25] as usize] * (0.00774096948249924 - 0.00032599868802996 * applyconv_r);
                    input_sample_r += s.b_r[s.c[26] as usize] * (0.00463907626773794 - 0.00032131993173361 * applyconv_r);
                    input_sample_r += s.b_r[s.c[27] as usize] * (0.00658222997260378 - 0.00032014977430211 * applyconv_r);
                    input_sample_r += s.b_r[s.c[28] as usize] * (0.00550347079924993 - 0.00031557153256653 * applyconv_r);
                    input_sample_r += s.b_r[s.c[29] as usize] * (0.00588754981375325 - 0.00032041307242303 * applyconv_r);
                    input_sample_r += s.b_r[s.c[30] as usize] * (0.00590293898419892 - 0.00030457857428714 * applyconv_r);
                    input_sample_r += s.b_r[s.c[31] as usize] * (0.00558952010441800 - 0.00030448053548086 * applyconv_r);
                    input_sample_r += s.b_r[s.c[32] as usize] * (0.00598183557634295 - 0.00030715064323181 * applyconv_r);
                    input_sample_r += s.b_r[s.c[33] as usize] * (0.00555223929714115 - 0.00030319367948553 * applyconv_r);
                }
                4 => {
                    input_sample_r -= s.b_r[s.c[1] as usize] * (0.25867935358656502 - 0.00045755657070112 * applyconv_r);
                    input_sample_r += s.b_r[s.c[2] as usize] * (0.11509367290253694 - 0.00017494270657228 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[3] as usize] * (0.06709853575891785 - 0.00058913102597723 * applyconv_r);
                    input_sample_r += s.b_r[s.c[4] as usize] * (0.01871006356851681 - 0.00003387358004645 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[5] as usize] * (0.00794797957360465 - 0.00044224784691203 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[6] as usize] * (0.01956921817394220 - 0.00006718936750076 * applyconv_r);
                    input_sample_r += s.b_r[s.c[7] as usize] * (0.01682120257195205 + 0.00032857446292230 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[8] as usize] * (0.03401069039824205 - 0.00013634182872897 * applyconv_r);
                    input_sample_r += s.b_r[s.c[9] as usize] * (0.02369950268232634 + 0.00023112685751657 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[10] as usize] * (0.03477071178117132 - 0.00018029792231600 * applyconv_r);
                    input_sample_r += s.b_r[s.c[11] as usize] * (0.02024369717958201 + 0.00017337813374202 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[12] as usize] * (0.02819087729102172 - 0.00021438538665420 * applyconv_r);
                    input_sample_r += s.b_r[s.c[13] as usize] * (0.01147946743141303 + 0.00014424066034649 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[14] as usize] * (0.01894777011468867 - 0.00021549146262408 * applyconv_r);
                    input_sample_r += s.b_r[s.c[15] as usize] * (0.00301370330346873 + 0.00013527460148394 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[16] as usize] * (0.01067147835815486 - 0.00020960689910868 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[17] as usize] * (0.00402715397506384 - 0.00014421582712470 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[18] as usize] * (0.00502221703392005 - 0.00019805767015024 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[19] as usize] * (0.00808788533308497 - 0.00016095444141931 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[20] as usize] * (0.00232696588842683 - 0.00018384470981829 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[21] as usize] * (0.00943950821324531 - 0.00017098987347593 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[22] as usize] * (0.00193709517200834 - 0.00018151995939591 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[23] as usize] * (0.00899713952612659 - 0.00017385835059948 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[24] as usize] * (0.00280584331659089 - 0.00017742164162470 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[25] as usize] * (0.00780381001954970 - 0.00018002500755708 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[26] as usize] * (0.00400370310490333 - 0.00017471691087957 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[27] as usize] * (0.00661527728186928 - 0.00018137323370347 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[28] as usize] * (0.00496545526864518 - 0.00017681872601767 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[29] as usize] * (0.00580728820997532 - 0.00018186220389790 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[30] as usize] * (0.00549309984725666 - 0.00017722985399075 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[31] as usize] * (0.00542194777529239 - 0.00018486900185338 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[32] as usize] * (0.00565992080998939 - 0.00018005824393118 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[33] as usize] * (0.00532121562846656 - 0.00018643189636216 * applyconv_r);
                }
                5 => {
                    input_sample_r += s.b_r[s.c[1] as usize] * (0.59188440274551890 - 0.00008361469668405 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[2] as usize] * (0.24439750948076133 + 0.00002651678396848 * applyconv_r);
                    input_sample_r += s.b_r[s.c[3] as usize] * (0.14109876103205621 - 0.00000840487181372 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[4] as usize] * (0.10053507128157971 + 0.00001768100964598 * applyconv_r);
                    input_sample_r += s.b_r[s.c[5] as usize] * (0.05859287880626238 - 0.00000361398065989 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[6] as usize] * (0.04337406889823660 + 0.00000735941182117 * applyconv_r);
                    input_sample_r += s.b_r[s.c[7] as usize] * (0.01589900680531097 + 0.00000207347387987 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[8] as usize] * (0.01087234854973281 + 0.00000732123412029 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[9] as usize] * (0.00845782429679176 - 0.00000133058605071 * applyconv_r);
                    input_sample_r += s.b_r[s.c[10] as usize] * (0.00662278586618295 - 0.00000424594730611 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[11] as usize] * (0.02000592193760155 + 0.00000632896879068 * applyconv_r);
                    input_sample_r += s.b_r[s.c[12] as usize] * (0.01321157777167565 - 0.00001421171592570 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[13] as usize] * (0.02249955362988238 + 0.00000163937127317 * applyconv_r);
                    input_sample_r += s.b_r[s.c[14] as usize] * (0.01196492077581504 - 0.00000535385220676 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[15] as usize] * (0.01905917427000097 + 0.00000121672882030 * applyconv_r);
                    input_sample_r += s.b_r[s.c[16] as usize] * (0.00761909482108073 - 0.00000326242895115 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[17] as usize] * (0.01362744780256239 + 0.00000359274216003 * applyconv_r);
                    input_sample_r += s.b_r[s.c[18] as usize] * (0.00200183122683721 - 0.00000089207452791 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[19] as usize] * (0.00833042637239315 + 0.00000946767677294 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[20] as usize] * (0.00258481175207224 - 0.00000087429351464 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[21] as usize] * (0.00459744479712244 - 0.00000049519758701 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[22] as usize] * (0.00534277030993820 + 0.00000397547847155 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[23] as usize] * (0.00272332919605675 + 0.00000040077229097 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[24] as usize] * (0.00637243782359372 - 0.00000139419072176 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[25] as usize] * (0.00233001590327504 + 0.00000420129915747 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[26] as usize] * (0.00623296727793041 + 0.00000019010664856 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[27] as usize] * (0.00276177096376805 + 0.00000580301901385 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[28] as usize] * (0.00559184754866264 + 0.00000080597287792 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[29] as usize] * (0.00343180144395919 - 0.00000243701142085 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[30] as usize] * (0.00493325428861701 + 0.00000300985740900 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[31] as usize] * (0.00396140827680823 - 0.00000051459681789 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[32] as usize] * (0.00448497879902493 + 0.00000744412841743 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[33] as usize] * (0.00425146888772076 - 0.00000082346016542 * applyconv_r);
                }
                6 => {
                    input_sample_r += s.b_r[s.c[1] as usize] * (0.09299870608542582 - 0.00009582362368873 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[2] as usize] * (0.11947847710741009 - 0.00004500891602770 * applyconv_r);
                    input_sample_r += s.b_r[s.c[3] as usize] * (0.09071606264761795 + 0.00005639498984741 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[4] as usize] * (0.08561982770836980 - 0.00004964855606916 * applyconv_r);
                    input_sample_r += s.b_r[s.c[5] as usize] * (0.06440549220820363 + 0.00002428052139507 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[6] as usize] * (0.05987991812840746 + 0.00000101867082290 * applyconv_r);
                    input_sample_r += s.b_r[s.c[7] as usize] * (0.03980233135839382 + 0.00003312430049041 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[8] as usize] * (0.03648402630896925 - 0.00002116186381142 * applyconv_r);
                    input_sample_r += s.b_r[s.c[9] as usize] * (0.01826860869525248 + 0.00003115110025396 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[10] as usize] * (0.01723968622495364 - 0.00002450634121718 * applyconv_r);
                    input_sample_r += s.b_r[s.c[11] as usize] * (0.00187588812316724 + 0.00002838206198968 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[12] as usize] * (0.00381796423957237 - 0.00003155815499462 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[13] as usize] * (0.00852092214496733 - 0.00001702651162392 * applyconv_r);
                    input_sample_r += s.b_r[s.c[14] as usize] * (0.00315560292270588 + 0.00002547861676047 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[15] as usize] * (0.01258630914496868 - 0.00004555319243213 * applyconv_r);
                    input_sample_r += s.b_r[s.c[16] as usize] * (0.00536435648963575 + 0.00001812393657101 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[17] as usize] * (0.01272975658159178 - 0.00004103775306121 * applyconv_r);
                    input_sample_r += s.b_r[s.c[18] as usize] * (0.00403818975172755 + 0.00003764615492871 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[19] as usize] * (0.01042617366897483 - 0.00003605210426041 * applyconv_r);
                    input_sample_r += s.b_r[s.c[20] as usize] * (0.00126599583390057 + 0.00004305458668852 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[21] as usize] * (0.00747876207688339 - 0.00003731207018977 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[22] as usize] * (0.00149873689175324 - 0.00005086601800791 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[23] as usize] * (0.00503221309488033 - 0.00003636086782783 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[24] as usize] * (0.00342998224655821 - 0.00004103091180506 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[25] as usize] * (0.00355585977903117 - 0.00003698982145400 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[26] as usize] * (0.00437201792934817 - 0.00002720235666939 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[27] as usize] * (0.00299217874451556 - 0.00004446954727956 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[28] as usize] * (0.00457924652487249 - 0.00003859065778860 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[29] as usize] * (0.00298182934892027 - 0.00002064710931733 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[30] as usize] * (0.00438838441540584 - 0.00005223008424866 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[31] as usize] * (0.00323984218794705 - 0.00003397987535887 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[32] as usize] * (0.00407693981307314 - 0.00003935772436894 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[33] as usize] * (0.00350435348467321 - 0.00005525463935338 * applyconv_r);
                }
                7 => {
                    input_sample_r -= s.b_r[s.c[1] as usize] * (0.23505923670562212 - 0.00028312859289245 * applyconv_r);
                    input_sample_r += s.b_r[s.c[2] as usize] * (0.08188436704577637 - 0.00008817721351341 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[3] as usize] * (0.05075798481700617 - 0.00018817166632483 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[4] as usize] * (0.00455811821873093 + 0.00001922902995296 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[5] as usize] * (0.00027610521433660 - 0.00013252525469291 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[6] as usize] * (0.03529246280346626 - 0.00002772989223299 * applyconv_r);
                    input_sample_r += s.b_r[s.c[7] as usize] * (0.01784111585586136 + 0.00010230276997291 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[8] as usize] * (0.04394950700298298 - 0.00005910607126944 * applyconv_r);
                    input_sample_r += s.b_r[s.c[9] as usize] * (0.01990770780547606 + 0.00007640328340556 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[10] as usize] * (0.04073629569741782 - 0.00007712327117090 * applyconv_r);
                    input_sample_r += s.b_r[s.c[11] as usize] * (0.01349648572795252 + 0.00005959130575917 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[12] as usize] * (0.03191590248003717 - 0.00008418000575151 * applyconv_r);
                    input_sample_r += s.b_r[s.c[13] as usize] * (0.00348795527924766 + 0.00005489156318238 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[14] as usize] * (0.02198496281481767 - 0.00008471601187581 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[15] as usize] * (0.00504771152505089 - 0.00005525060587917 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[16] as usize] * (0.01391075698598491 - 0.00007929630732607 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[17] as usize] * (0.01142762504081717 - 0.00005967036737742 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[18] as usize] * (0.00893541815021255 - 0.00007535697758141 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[19] as usize] * (0.01459704973464936 - 0.00005969199602841 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[20] as usize] * (0.00694755135226282 - 0.00006930127097865 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[21] as usize] * (0.01516695630808575 - 0.00006365800069826 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[22] as usize] * (0.00705917318113651 - 0.00006497209096539 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[23] as usize] * (0.01420501209177591 - 0.00006555654576113 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[24] as usize] * (0.00815905656808701 - 0.00006105622534761 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[25] as usize] * (0.01274326525552961 - 0.00006542652857017 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[26] as usize] * (0.00937146927845488 - 0.00006051267868722 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[27] as usize] * (0.01146573981165209 - 0.00006381511607749 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[28] as usize] * (0.01021294359409007 - 0.00005930397856398 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[29] as usize] * (0.01065217095323532 - 0.00006371505438319 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[30] as usize] * (0.01058751196699751 - 0.00006042857480233 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[31] as usize] * (0.01026557827762401 - 0.00006007776163871 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[32] as usize] * (0.01060929183604604 - 0.00006114703012726 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[33] as usize] * (0.01014533525058528 - 0.00005963567932887 * applyconv_r);
                }
                8 => {
                    input_sample_r -= s.b_r[s.c[1] as usize] * (0.20641602693167951 - 0.00078952185394898 * applyconv_r);
                    input_sample_r += s.b_r[s.c[2] as usize] * (0.07601816702459827 + 0.00022786334179951 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[3] as usize] * (0.03929765560019285 - 0.00054517993246352 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[4] as usize] * (0.00298333157711103 - 0.00033083756545638 * applyconv_r);
                    input_sample_r += s.b_r[s.c[5] as usize] * (0.00724006282304610 + 0.00045483683460812 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[6] as usize] * (0.03073108963506036 - 0.00038190060537423 * applyconv_r);
                    input_sample_r += s.b_r[s.c[7] as usize] * (0.02332434692533051 + 0.00040347288688932 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[8] as usize] * (0.03792606869061214 - 0.00039673687335892 * applyconv_r);
                    input_sample_r += s.b_r[s.c[9] as usize] * (0.02437059376675688 + 0.00037221210539535 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[10] as usize] * (0.03416764311979521 - 0.00040314850796953 * applyconv_r);
                    input_sample_r += s.b_r[s.c[11] as usize] * (0.01761669868102127 + 0.00035989484330131 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[12] as usize] * (0.02538237753523052 - 0.00040149119125394 * applyconv_r);
                    input_sample_r += s.b_r[s.c[13] as usize] * (0.00770737340728377 + 0.00035462118723555 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[14] as usize] * (0.01580706228482803 - 0.00037563141307594 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[15] as usize] * (0.00055119240005586 - 0.00035409299268971 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[16] as usize] * (0.00818552143438768 - 0.00036507661042180 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[17] as usize] * (0.00661842703548304 - 0.00034550528559056 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[18] as usize] * (0.00362447476272098 - 0.00035553012761240 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[19] as usize] * (0.00957098027225745 - 0.00034091691045338 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[20] as usize] * (0.00193621774016660 - 0.00034554529131668 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[21] as usize] * (0.01005433027357935 - 0.00033878223153845 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[22] as usize] * (0.00221712428802004 - 0.00033481410137711 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[23] as usize] * (0.00911255639207995 - 0.00033263425232666 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[24] as usize] * (0.00339667169034909 - 0.00032634428038430 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[25] as usize] * (0.00774096948249924 - 0.00032599868802996 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[26] as usize] * (0.00463907626773794 - 0.00032131993173361 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[27] as usize] * (0.00658222997260378 - 0.00032014977430211 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[28] as usize] * (0.00550347079924993 - 0.00031557153256653 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[29] as usize] * (0.00588754981375325 - 0.00032041307242303 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[30] as usize] * (0.00590293898419892 - 0.00030457857428714 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[31] as usize] * (0.00558952010441800 - 0.00030448053548086 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[32] as usize] * (0.00598183557634295 - 0.00030715064323181 * applyconv_r);
                    input_sample_r -= s.b_r[s.c[33] as usize] * (0.00555223929714115 - 0.00030319367948553 * applyconv_r);
                }
                _ => {}
            }
            bridgerectifier = input_sample_l.abs();
            bridgerectifier = 1.0 - bridgerectifier.cos();
            if input_sample_l > 0.0 {
                input_sample_l -= bridgerectifier;
            } else {
                input_sample_l += bridgerectifier;
            }
            bridgerectifier = input_sample_r.abs();
            bridgerectifier = 1.0 - bridgerectifier.cos();
            if input_sample_r > 0.0 {
                input_sample_r -= bridgerectifier;
            } else {
                input_sample_r += bridgerectifier;
            }
            if outgain != 1.0 {
                input_sample_l *= outgain;
                input_sample_r *= outgain;
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
