// Ported from IronOxide5 (IronOxide5Proc.cpp, IronOxide5.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Iron Oxide (sub-effect id 41): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Input Trim", 0.5),
    pf("Tape High", 0.562341325190349),
    pf("Tape Low", 0.562341325190349),
    pf("Flutter", 0.5),
    pf("Noise", 0.5),
    pf("Output Trim", 0.5),
    pf("Inv/Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct IronOxide5 {
    p: [f32; 7],
    sr: f32,
    iir_sampleh_al: f64,
    iir_sampleh_bl: f64,
    iir_sample_al: f64,
    iir_sample_bl: f64,
    d_l: Vec<f64>,
    fast_iiral: f64,
    fast_iirbl: f64,
    slow_iiral: f64,
    slow_iirbl: f64,
    fast_iihal: f64,
    fast_iihbl: f64,
    slow_iihal: f64,
    slow_iihbl: f64,
    prev_input_sample_l: f64,
    iir_sampleh_ar: f64,
    iir_sampleh_br: f64,
    iir_sample_ar: f64,
    iir_sample_br: f64,
    d_r: Vec<f64>,
    fast_iirar: f64,
    fast_iirbr: f64,
    slow_iirar: f64,
    slow_iirbr: f64,
    fast_iihar: f64,
    fast_iihbr: f64,
    slow_iihar: f64,
    slow_iihbr: f64,
    prev_input_sample_r: f64,
    gcount: i32,
    flip: bool,
    fl_l: Vec<f64>,
    fl_r: Vec<f64>,
    fstoredcount: i32,
    rateof: f64,
    sweep: f64,
    nextmax: f64,
    fpd_l: u32,
    fpd_r: u32,
}

impl IronOxide5 {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<IronOxide5> {
        let mut s = Box::new(IronOxide5 {
            p: [0.5, 0.562341325190349, 0.562341325190349, 0.5, 0.5, 0.5, 1.0],
            sr: sr as f32,
            iir_sampleh_al: 0.0,
            iir_sampleh_bl: 0.0,
            iir_sample_al: 0.0,
            iir_sample_bl: 0.0,
            d_l: vec![0.0; 264],
            fast_iiral: 0.0,
            fast_iirbl: 0.0,
            slow_iiral: 0.0,
            slow_iirbl: 0.0,
            fast_iihal: 0.0,
            fast_iihbl: 0.0,
            slow_iihal: 0.0,
            slow_iihbl: 0.0,
            prev_input_sample_l: 0.0,
            iir_sampleh_ar: 0.0,
            iir_sampleh_br: 0.0,
            iir_sample_ar: 0.0,
            iir_sample_br: 0.0,
            d_r: vec![0.0; 264],
            fast_iirar: 0.0,
            fast_iirbr: 0.0,
            slow_iirar: 0.0,
            slow_iirbr: 0.0,
            fast_iihar: 0.0,
            fast_iihbr: 0.0,
            slow_iihar: 0.0,
            slow_iihbr: 0.0,
            prev_input_sample_r: 0.0,
            gcount: 0,
            flip: false,
            fl_l: vec![0.0; 100],
            fl_r: vec![0.0; 100],
            fstoredcount: 0,
            rateof: 0.0,
            sweep: 0.0,
            nextmax: 0.0,
            fpd_l: 0,
            fpd_r: 0,
        });
        s.rateof = 0.5;
        s.nextmax = 0.5;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for IronOxide5 {
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
        let mut inputgain: f64 = 10.0_f64.powf((s.p[0] as f64 * 36.0 - 18.0) / 20.0);
        let mut outputgain: f64 = 10.0_f64.powf((s.p[5] as f64 * 36.0 - 18.0) / 20.0);
        let mut ips: f64 = ((s.p[1] * s.p[1] * (s.p[1] * s.p[1])) as f64 * 148.5 + 1.5) * 1.1;
        if ips < 1.0 || ips > 200.0 {
            ips = 33.0;
        }
        let mut temp_randy: f64 = 0.04 + 0.11 / ips.sqrt();
        let mut randy: f64 = 0.0;
        let mut lps: f64 = ((s.p[2] * s.p[2] * (s.p[2] * s.p[2])) as f64 * 148.5 + 1.5) * 1.1;
        if lps < 1.0 || lps > 200.0 {
            lps = 33.0;
        }
        let mut iir_amount: f64 = lps / 430.0;
        let mut bridgerectifier_l: f64 = 0.0;
        let mut bridgerectifier_r: f64 = 0.0;
        let mut fast_taper: f64 = ips / 15.0;
        let mut slow_taper: f64 = 2.0 / (lps * lps);
        let mut lowspeedscale: f64 = 5.0 / ips;
        let mut count: i32 = 0;
        let mut flutcount: i32 = 0;
        let mut flutterrandy: f64 = 0.0;
        let mut temp: f64 = 0.0;
        let mut depth: f64 = (s.p[3] as f64).powf(2.0) * overallscale;
        let mut fluttertrim: f64 = 0.00581 / overallscale;
        let mut sweeptrim: f64 = 0.0005 * depth / overallscale;
        let mut offset: f64 = 0.0;
        let mut flooroffset: f64 = 0.0;
        let mut tupi: f64 = 3.141592653589793238 * 2.0;
        let mut newrate: f64 = 0.006 / overallscale;
        let mut oldrate: f64 = 1.0 - newrate;
        if overallscale == 0.0 {
            fast_taper += 1.0;
            slow_taper += 1.0;
        } else {
            iir_amount /= overallscale;
            lowspeedscale *= overallscale;
            fast_taper = 1.0 + fast_taper / overallscale;
            slow_taper = 1.0 + slow_taper / overallscale;
        }
        let mut noise: f64 = s.p[4] as f64 * 0.5;
        let mut invdrywet: f64 = s.p[6] as f64 * 2.0 - 1.0;
        let mut dry: f64 = 1.0;
        if invdrywet > 0.0 {
            dry -= invdrywet;
        }
        let mut input_sample_l: f64 = 0.0;
        let mut input_sample_r: f64 = 0.0;
        let mut dry_sample_l: f64 = 0.0;
        let mut dry_sample_r: f64 = 0.0;
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
            flutterrandy = s.fpd_l as f64 / u32::MAX as f64;
            if s.fstoredcount < 0 || s.fstoredcount > 30 {
                s.fstoredcount = 30;
            }
            flutcount = s.fstoredcount;
            s.fl_l[flutcount as usize] = input_sample_l;
            s.fl_l[(flutcount + 31) as usize] = s.fl_l[flutcount as usize];
            s.fl_r[flutcount as usize] = input_sample_r;
            s.fl_r[(flutcount + 31) as usize] = s.fl_r[flutcount as usize];
            offset = (1.0 + s.sweep.sin()) * depth;
            flooroffset = offset - offset.floor();
            flutcount += offset.floor() as i32;
            bridgerectifier_l = s.fl_l[flutcount as usize] * (1.0 - flooroffset);
            bridgerectifier_r = s.fl_r[flutcount as usize] * (1.0 - flooroffset);
            bridgerectifier_l += s.fl_l[(flutcount + 1) as usize] * flooroffset;
            bridgerectifier_r += s.fl_r[(flutcount + 1) as usize] * flooroffset;
            s.rateof = s.nextmax * newrate + s.rateof * oldrate;
            s.sweep += s.rateof * fluttertrim;
            s.sweep += s.sweep * sweeptrim;
            if s.sweep >= tupi {
                s.sweep = 0.0;
                s.nextmax = 0.02 + flutterrandy * 0.98;
            }
            s.fstoredcount -= 1;
            input_sample_l = bridgerectifier_l;
            input_sample_r = bridgerectifier_r;
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
            input_sample_l *= inputgain;
            input_sample_r *= inputgain;
            bridgerectifier_l = input_sample_l.abs();
            if bridgerectifier_l > 1.57079633 {
                bridgerectifier_l = 1.57079633;
            }
            bridgerectifier_l = bridgerectifier_l.sin();
            if input_sample_l > 0.0 {
                input_sample_l = bridgerectifier_l;
            } else {
                input_sample_l = -bridgerectifier_l;
            }
            bridgerectifier_r = input_sample_r.abs();
            if bridgerectifier_r > 1.57079633 {
                bridgerectifier_r = 1.57079633;
            }
            bridgerectifier_r = bridgerectifier_r.sin();
            if input_sample_r > 0.0 {
                input_sample_r = bridgerectifier_r;
            } else {
                input_sample_r = -bridgerectifier_r;
            }
            if s.gcount < 0 || s.gcount > 131 {
                s.gcount = 131;
            }
            count = s.gcount;
            s.d_l[count as usize] = input_sample_l;
            s.d_l[(count + 131) as usize] = s.d_l[count as usize];
            s.d_r[count as usize] = input_sample_r;
            s.d_r[(count + 131) as usize] = s.d_r[count as usize];
            if s.flip {
                s.fast_iiral = s.fast_iiral / fast_taper;
                s.slow_iiral = s.slow_iiral / slow_taper;
                s.fast_iiral += s.d_l[count as usize];
                s.fast_iirar = s.fast_iirar / fast_taper;
                s.slow_iirar = s.slow_iirar / slow_taper;
                s.fast_iirar += s.d_r[count as usize];
                count += 3;
                temp = s.d_l[(count + 127) as usize];
                temp += s.d_l[(count + 113) as usize];
                temp += s.d_l[(count + 109) as usize];
                temp += s.d_l[(count + 107) as usize];
                temp += s.d_l[(count + 103) as usize];
                temp += s.d_l[(count + 101) as usize];
                temp += s.d_l[(count + 97) as usize];
                temp += s.d_l[(count + 89) as usize];
                temp += s.d_l[(count + 83) as usize];
                temp /= 2.0;
                temp += s.d_l[(count + 79) as usize];
                temp += s.d_l[(count + 73) as usize];
                temp += s.d_l[(count + 71) as usize];
                temp += s.d_l[(count + 67) as usize];
                temp += s.d_l[(count + 61) as usize];
                temp += s.d_l[(count + 59) as usize];
                temp += s.d_l[(count + 53) as usize];
                temp += s.d_l[(count + 47) as usize];
                temp += s.d_l[(count + 43) as usize];
                temp += s.d_l[(count + 41) as usize];
                temp += s.d_l[(count + 37) as usize];
                temp += s.d_l[(count + 31) as usize];
                temp += s.d_l[(count + 29) as usize];
                temp /= 2.0;
                temp += s.d_l[(count + 23) as usize];
                temp += s.d_l[(count + 19) as usize];
                temp += s.d_l[(count + 17) as usize];
                temp += s.d_l[(count + 13) as usize];
                temp += s.d_l[(count + 11) as usize];
                temp /= 2.0;
                temp += s.d_l[(count + 7) as usize];
                temp += s.d_l[(count + 5) as usize];
                temp += s.d_l[(count + 3) as usize];
                temp /= 2.0;
                temp += s.d_l[(count + 2) as usize];
                temp += s.d_l[(count + 1) as usize];
                s.slow_iiral += temp / 128.0;
                temp = s.d_r[(count + 127) as usize];
                temp += s.d_r[(count + 113) as usize];
                temp += s.d_r[(count + 109) as usize];
                temp += s.d_r[(count + 107) as usize];
                temp += s.d_r[(count + 103) as usize];
                temp += s.d_r[(count + 101) as usize];
                temp += s.d_r[(count + 97) as usize];
                temp += s.d_r[(count + 89) as usize];
                temp += s.d_r[(count + 83) as usize];
                temp /= 2.0;
                temp += s.d_r[(count + 79) as usize];
                temp += s.d_r[(count + 73) as usize];
                temp += s.d_r[(count + 71) as usize];
                temp += s.d_r[(count + 67) as usize];
                temp += s.d_r[(count + 61) as usize];
                temp += s.d_r[(count + 59) as usize];
                temp += s.d_r[(count + 53) as usize];
                temp += s.d_r[(count + 47) as usize];
                temp += s.d_r[(count + 43) as usize];
                temp += s.d_r[(count + 41) as usize];
                temp += s.d_r[(count + 37) as usize];
                temp += s.d_r[(count + 31) as usize];
                temp += s.d_r[(count + 29) as usize];
                temp /= 2.0;
                temp += s.d_r[(count + 23) as usize];
                temp += s.d_r[(count + 19) as usize];
                temp += s.d_r[(count + 17) as usize];
                temp += s.d_r[(count + 13) as usize];
                temp += s.d_r[(count + 11) as usize];
                temp /= 2.0;
                temp += s.d_r[(count + 7) as usize];
                temp += s.d_r[(count + 5) as usize];
                temp += s.d_r[(count + 3) as usize];
                temp /= 2.0;
                temp += s.d_r[(count + 2) as usize];
                temp += s.d_r[(count + 1) as usize];
                s.slow_iirar += temp / 128.0;
                input_sample_l = s.fast_iiral - s.slow_iiral / slow_taper;
                input_sample_r = s.fast_iirar - s.slow_iirar / slow_taper;
            } else {
                s.fast_iirbl = s.fast_iirbl / fast_taper;
                s.slow_iirbl = s.slow_iirbl / slow_taper;
                s.fast_iirbl += s.d_l[count as usize];
                s.fast_iirbr = s.fast_iirbr / fast_taper;
                s.slow_iirbr = s.slow_iirbr / slow_taper;
                s.fast_iirbr += s.d_r[count as usize];
                count += 3;
                temp = s.d_l[(count + 127) as usize];
                temp += s.d_l[(count + 113) as usize];
                temp += s.d_l[(count + 109) as usize];
                temp += s.d_l[(count + 107) as usize];
                temp += s.d_l[(count + 103) as usize];
                temp += s.d_l[(count + 101) as usize];
                temp += s.d_l[(count + 97) as usize];
                temp += s.d_l[(count + 89) as usize];
                temp += s.d_l[(count + 83) as usize];
                temp /= 2.0;
                temp += s.d_l[(count + 79) as usize];
                temp += s.d_l[(count + 73) as usize];
                temp += s.d_l[(count + 71) as usize];
                temp += s.d_l[(count + 67) as usize];
                temp += s.d_l[(count + 61) as usize];
                temp += s.d_l[(count + 59) as usize];
                temp += s.d_l[(count + 53) as usize];
                temp += s.d_l[(count + 47) as usize];
                temp += s.d_l[(count + 43) as usize];
                temp += s.d_l[(count + 41) as usize];
                temp += s.d_l[(count + 37) as usize];
                temp += s.d_l[(count + 31) as usize];
                temp += s.d_l[(count + 29) as usize];
                temp /= 2.0;
                temp += s.d_l[(count + 23) as usize];
                temp += s.d_l[(count + 19) as usize];
                temp += s.d_l[(count + 17) as usize];
                temp += s.d_l[(count + 13) as usize];
                temp += s.d_l[(count + 11) as usize];
                temp /= 2.0;
                temp += s.d_l[(count + 7) as usize];
                temp += s.d_l[(count + 5) as usize];
                temp += s.d_l[(count + 3) as usize];
                temp /= 2.0;
                temp += s.d_l[(count + 2) as usize];
                temp += s.d_l[(count + 1) as usize];
                s.slow_iirbl += temp / 128.0;
                temp = s.d_r[(count + 127) as usize];
                temp += s.d_r[(count + 113) as usize];
                temp += s.d_r[(count + 109) as usize];
                temp += s.d_r[(count + 107) as usize];
                temp += s.d_r[(count + 103) as usize];
                temp += s.d_r[(count + 101) as usize];
                temp += s.d_r[(count + 97) as usize];
                temp += s.d_r[(count + 89) as usize];
                temp += s.d_r[(count + 83) as usize];
                temp /= 2.0;
                temp += s.d_r[(count + 79) as usize];
                temp += s.d_r[(count + 73) as usize];
                temp += s.d_r[(count + 71) as usize];
                temp += s.d_r[(count + 67) as usize];
                temp += s.d_r[(count + 61) as usize];
                temp += s.d_r[(count + 59) as usize];
                temp += s.d_r[(count + 53) as usize];
                temp += s.d_r[(count + 47) as usize];
                temp += s.d_r[(count + 43) as usize];
                temp += s.d_r[(count + 41) as usize];
                temp += s.d_r[(count + 37) as usize];
                temp += s.d_r[(count + 31) as usize];
                temp += s.d_r[(count + 29) as usize];
                temp /= 2.0;
                temp += s.d_r[(count + 23) as usize];
                temp += s.d_r[(count + 19) as usize];
                temp += s.d_r[(count + 17) as usize];
                temp += s.d_r[(count + 13) as usize];
                temp += s.d_r[(count + 11) as usize];
                temp /= 2.0;
                temp += s.d_r[(count + 7) as usize];
                temp += s.d_r[(count + 5) as usize];
                temp += s.d_r[(count + 3) as usize];
                temp /= 2.0;
                temp += s.d_r[(count + 2) as usize];
                temp += s.d_r[(count + 1) as usize];
                s.slow_iirbr += temp / 128.0;
                input_sample_l = s.fast_iirbl - s.slow_iirbl / slow_taper;
                input_sample_r = s.fast_iirbr - s.slow_iirbr / slow_taper;
            }
            input_sample_l /= fast_taper;
            input_sample_r /= fast_taper;
            input_sample_l /= lowspeedscale;
            input_sample_r /= lowspeedscale;
            bridgerectifier_l = input_sample_l.abs();
            if bridgerectifier_l > 1.57079633 {
                bridgerectifier_l = 1.57079633;
            }
            bridgerectifier_l = bridgerectifier_l.sin();
            if input_sample_l > 0.0 {
                input_sample_l = bridgerectifier_l;
            } else {
                input_sample_l = -bridgerectifier_l;
            }
            bridgerectifier_r = input_sample_r.abs();
            if bridgerectifier_r > 1.57079633 {
                bridgerectifier_r = 1.57079633;
            }
            bridgerectifier_r = bridgerectifier_r.sin();
            if input_sample_r > 0.0 {
                input_sample_r = bridgerectifier_r;
            } else {
                input_sample_r = -bridgerectifier_r;
            }
            randy = (0.55 + temp_randy + s.fpd_r as f64 / u32::MAX as f64 * temp_randy) * noise;
            input_sample_l *= 1.0 - randy;
            input_sample_l += s.prev_input_sample_l * randy;
            s.prev_input_sample_l = dry_sample_l;
            input_sample_r *= 1.0 - randy;
            input_sample_r += s.prev_input_sample_r * randy;
            s.prev_input_sample_r = dry_sample_r;
            s.flip = !s.flip;
            if outputgain != 1.0 {
                input_sample_l *= outputgain;
                input_sample_r *= outputgain;
            }
            if invdrywet != 1.0 {
                input_sample_l *= invdrywet;
                input_sample_r *= invdrywet;
            }
            if dry != 1.0 {
                dry_sample_l *= dry;
                dry_sample_r *= dry;
            }
            if dry_sample_l.abs() > 0.0 {
                input_sample_l += dry_sample_l;
            }
            if dry_sample_r.abs() > 0.0 {
                input_sample_r += dry_sample_r;
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
