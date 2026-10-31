// Ported from DrumSlam (DrumSlamProc.cpp, DrumSlam.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Drum Slam (sub-effect id 45): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Drive", 0.0),
    pf("Output", 1.0),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct DrumSlam {
    p: [f32; 3],
    sr: f32,
    fpd_l: u32,
    fpd_r: u32,
    iir_sample_al: f64,
    iir_sample_bl: f64,
    iir_sample_cl: f64,
    iir_sample_dl: f64,
    iir_sample_el: f64,
    iir_sample_fl: f64,
    iir_sample_gl: f64,
    iir_sample_hl: f64,
    last_sample_l: f64,
    iir_sample_ar: f64,
    iir_sample_br: f64,
    iir_sample_cr: f64,
    iir_sample_dr: f64,
    iir_sample_er: f64,
    iir_sample_fr: f64,
    iir_sample_gr: f64,
    iir_sample_hr: f64,
    last_sample_r: f64,
    fp_flip: bool,
    d: f32,
    e: f32,
}

impl DrumSlam {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<DrumSlam> {
        let mut s = Box::new(DrumSlam {
            p: [0.0, 1.0, 1.0],
            sr: sr as f32,
            fpd_l: 0,
            fpd_r: 0,
            iir_sample_al: 0.0,
            iir_sample_bl: 0.0,
            iir_sample_cl: 0.0,
            iir_sample_dl: 0.0,
            iir_sample_el: 0.0,
            iir_sample_fl: 0.0,
            iir_sample_gl: 0.0,
            iir_sample_hl: 0.0,
            last_sample_l: 0.0,
            iir_sample_ar: 0.0,
            iir_sample_br: 0.0,
            iir_sample_cr: 0.0,
            iir_sample_dr: 0.0,
            iir_sample_er: 0.0,
            iir_sample_fr: 0.0,
            iir_sample_gr: 0.0,
            iir_sample_hr: 0.0,
            last_sample_r: 0.0,
            fp_flip: false,
            d: 0.0,
            e: 0.0,
        });
        s.fp_flip = true;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for DrumSlam {
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
        let mut iir_amount_l: f64 = 0.0819;
        iir_amount_l /= overallscale;
        let mut iir_amount_h: f64 = 0.377933067;
        iir_amount_h /= overallscale;
        let mut drive: f64 = s.p[0] as f64 * 3.0 + 1.0;
        let mut out: f64 = s.p[1] as f64;
        let mut wet: f64 = s.p[2] as f64;
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
            let mut low_sample_l: f64 = 0.0;
            let mut low_sample_r: f64 = 0.0;
            let mut mid_sample_l: f64 = 0.0;
            let mut mid_sample_r: f64 = 0.0;
            let mut high_sample_l: f64 = 0.0;
            let mut high_sample_r: f64 = 0.0;
            input_sample_l *= drive;
            input_sample_r *= drive;
            if s.fp_flip {
                s.iir_sample_al = s.iir_sample_al * (1.0 - iir_amount_l) + input_sample_l * iir_amount_l;
                s.iir_sample_bl = s.iir_sample_bl * (1.0 - iir_amount_l) + s.iir_sample_al * iir_amount_l;
                low_sample_l = s.iir_sample_bl;
                s.iir_sample_ar = s.iir_sample_ar * (1.0 - iir_amount_l) + input_sample_r * iir_amount_l;
                s.iir_sample_br = s.iir_sample_br * (1.0 - iir_amount_l) + s.iir_sample_ar * iir_amount_l;
                low_sample_r = s.iir_sample_br;
                s.iir_sample_el = s.iir_sample_el * (1.0 - iir_amount_h) + input_sample_l * iir_amount_h;
                s.iir_sample_fl = s.iir_sample_fl * (1.0 - iir_amount_h) + s.iir_sample_el * iir_amount_h;
                mid_sample_l = s.iir_sample_fl - s.iir_sample_bl;
                s.iir_sample_er = s.iir_sample_er * (1.0 - iir_amount_h) + input_sample_r * iir_amount_h;
                s.iir_sample_fr = s.iir_sample_fr * (1.0 - iir_amount_h) + s.iir_sample_er * iir_amount_h;
                mid_sample_r = s.iir_sample_fr - s.iir_sample_br;
                high_sample_l = input_sample_l - s.iir_sample_fl;
                high_sample_r = input_sample_r - s.iir_sample_fr;
            } else {
                s.iir_sample_cl = s.iir_sample_cl * (1.0 - iir_amount_l) + input_sample_l * iir_amount_l;
                s.iir_sample_dl = s.iir_sample_dl * (1.0 - iir_amount_l) + s.iir_sample_cl * iir_amount_l;
                low_sample_l = s.iir_sample_dl;
                s.iir_sample_cr = s.iir_sample_cr * (1.0 - iir_amount_l) + input_sample_r * iir_amount_l;
                s.iir_sample_dr = s.iir_sample_dr * (1.0 - iir_amount_l) + s.iir_sample_cr * iir_amount_l;
                low_sample_r = s.iir_sample_dr;
                s.iir_sample_gl = s.iir_sample_gl * (1.0 - iir_amount_h) + input_sample_l * iir_amount_h;
                s.iir_sample_hl = s.iir_sample_hl * (1.0 - iir_amount_h) + s.iir_sample_gl * iir_amount_h;
                mid_sample_l = s.iir_sample_hl - s.iir_sample_dl;
                s.iir_sample_gr = s.iir_sample_gr * (1.0 - iir_amount_h) + input_sample_r * iir_amount_h;
                s.iir_sample_hr = s.iir_sample_hr * (1.0 - iir_amount_h) + s.iir_sample_gr * iir_amount_h;
                mid_sample_r = s.iir_sample_hr - s.iir_sample_dr;
                high_sample_l = input_sample_l - s.iir_sample_hl;
                high_sample_r = input_sample_r - s.iir_sample_hr;
            }
            s.fp_flip = !s.fp_flip;
            if low_sample_l > 1.0 {
                low_sample_l = 1.0;
            }
            if low_sample_l < -1.0 {
                low_sample_l = -1.0;
            }
            if low_sample_r > 1.0 {
                low_sample_r = 1.0;
            }
            if low_sample_r < -1.0 {
                low_sample_r = -1.0;
            }
            low_sample_l -= low_sample_l * (low_sample_l.abs() * 0.448) * (low_sample_l.abs() * 0.448);
            low_sample_r -= low_sample_r * (low_sample_r.abs() * 0.448) * (low_sample_r.abs() * 0.448);
            low_sample_l *= drive;
            low_sample_r *= drive;
            if high_sample_l > 1.0 {
                high_sample_l = 1.0;
            }
            if high_sample_l < -1.0 {
                high_sample_l = -1.0;
            }
            if high_sample_r > 1.0 {
                high_sample_r = 1.0;
            }
            if high_sample_r < -1.0 {
                high_sample_r = -1.0;
            }
            high_sample_l -= high_sample_l * (high_sample_l.abs() * 0.599) * (high_sample_l.abs() * 0.599);
            high_sample_r -= high_sample_r * (high_sample_r.abs() * 0.599) * (high_sample_r.abs() * 0.599);
            high_sample_l *= drive;
            high_sample_r *= drive;
            mid_sample_l = mid_sample_l * drive;
            mid_sample_r = mid_sample_r * drive;
            let mut skew: f64 = mid_sample_l - s.last_sample_l;
            s.last_sample_l = mid_sample_l;
            let mut bridgerectifier: f64 = skew.abs();
            if bridgerectifier > 3.1415926 {
                bridgerectifier = 3.1415926;
            }
            bridgerectifier = bridgerectifier.sin();
            if skew > 0.0 {
                skew = bridgerectifier * 3.1415926;
            } else {
                skew = -bridgerectifier * 3.1415926;
            }
            skew *= mid_sample_l;
            skew *= 1.557079633;
            bridgerectifier = mid_sample_l.abs();
            bridgerectifier += skew;
            if bridgerectifier > 1.57079633 {
                bridgerectifier = 1.57079633;
            }
            bridgerectifier = bridgerectifier.sin();
            bridgerectifier *= drive;
            bridgerectifier += skew;
            if bridgerectifier > 1.57079633 {
                bridgerectifier = 1.57079633;
            }
            bridgerectifier = bridgerectifier.sin();
            if mid_sample_l > 0.0 {
                mid_sample_l = bridgerectifier;
            } else {
                mid_sample_l = -bridgerectifier;
            }
            skew = mid_sample_r - s.last_sample_r;
            s.last_sample_r = mid_sample_r;
            bridgerectifier = skew.abs();
            if bridgerectifier > 3.1415926 {
                bridgerectifier = 3.1415926;
            }
            bridgerectifier = bridgerectifier.sin();
            if skew > 0.0 {
                skew = bridgerectifier * 3.1415926;
            } else {
                skew = -bridgerectifier * 3.1415926;
            }
            skew *= mid_sample_r;
            skew *= 1.557079633;
            bridgerectifier = mid_sample_r.abs();
            bridgerectifier += skew;
            if bridgerectifier > 1.57079633 {
                bridgerectifier = 1.57079633;
            }
            bridgerectifier = bridgerectifier.sin();
            bridgerectifier *= drive;
            bridgerectifier += skew;
            if bridgerectifier > 1.57079633 {
                bridgerectifier = 1.57079633;
            }
            bridgerectifier = bridgerectifier.sin();
            if mid_sample_r > 0.0 {
                mid_sample_r = bridgerectifier;
            } else {
                mid_sample_r = -bridgerectifier;
            }
            input_sample_l = (low_sample_l + mid_sample_l + high_sample_l) / drive * out;
            input_sample_r = (low_sample_r + mid_sample_r + high_sample_r) / drive * out;
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
