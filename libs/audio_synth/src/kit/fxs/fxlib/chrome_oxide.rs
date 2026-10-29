// Ported from ChromeOxide (ChromeOxideProc.cpp, ChromeOxide.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Chrome Oxide (sub-effect id 59): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Intense", 0.5),
    pf("Bias", 0.5),
];

#[allow(dead_code)]
pub struct ChromeOxide {
    p: [f32; 2],
    sr: f32,
    iir_sample_al: f64,
    iir_sample_bl: f64,
    iir_sample_cl: f64,
    iir_sample_dl: f64,
    second_sample_l: f64,
    third_sample_l: f64,
    fourth_sample_l: f64,
    fifth_sample_l: f64,
    iir_sample_ar: f64,
    iir_sample_br: f64,
    iir_sample_cr: f64,
    iir_sample_dr: f64,
    second_sample_r: f64,
    third_sample_r: f64,
    fourth_sample_r: f64,
    fifth_sample_r: f64,
    flip: bool,
    fpd_l: u32,
    fpd_r: u32,
}

impl ChromeOxide {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<ChromeOxide> {
        let mut s = Box::new(ChromeOxide {
            p: [0.5, 0.5],
            sr: sr as f32,
            iir_sample_al: 0.0,
            iir_sample_bl: 0.0,
            iir_sample_cl: 0.0,
            iir_sample_dl: 0.0,
            second_sample_l: 0.0,
            third_sample_l: 0.0,
            fourth_sample_l: 0.0,
            fifth_sample_l: 0.0,
            iir_sample_ar: 0.0,
            iir_sample_br: 0.0,
            iir_sample_cr: 0.0,
            iir_sample_dr: 0.0,
            second_sample_r: 0.0,
            third_sample_r: 0.0,
            fourth_sample_r: 0.0,
            fifth_sample_r: 0.0,
            flip: false,
            fpd_l: 0,
            fpd_r: 0,
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for ChromeOxide {
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
        let mut bass_sample_l: f64 = 0.0;
        let mut bass_sample_r: f64 = 0.0;
        let mut randy: f64 = 0.0;
        let mut bias: f64 = s.p[1] as f64 / 1.31578947368421;
        let mut intensity: f64 = 0.9 + (s.p[0] as f64).powf(2.0);
        let mut iir_amount: f64 = (1.0 - intensity / (10.0 + bias * 4.0)).powf(2.0) / overallscale;
        let mut bridgerectifier: f64 = 0.0;
        let mut treble_gain_trim: f64 = 1.0;
        let mut indrive: f64 = 1.0;
        let mut density_a: f64 = intensity * 80.0 + 1.0;
        let mut noise: f64 = intensity / (1.0 + bias);
        let mut bass_gain_trim: f64 = 1.0;
        let mut glitch: f64 = 0.0;
        bias *= overallscale;
        noise *= overallscale;
        if intensity > 1.0 {
            glitch = intensity - 1.0;
            indrive = intensity * intensity;
            bass_gain_trim /= intensity * intensity;
            treble_gain_trim = (intensity + 1.0) / 2.0;
        }
        for smp in 0..l.len() {
            let mut input_sample_l: f64 = l[smp] as f64;
            let mut input_sample_r: f64 = r[smp] as f64;
            if input_sample_l.abs() < 1.18e-23 {
                input_sample_l = s.fpd_l as f64 * 1.18e-17;
            }
            if input_sample_r.abs() < 1.18e-23 {
                input_sample_r = s.fpd_r as f64 * 1.18e-17;
            }
            input_sample_l *= indrive;
            input_sample_r *= indrive;
            bass_sample_l = input_sample_l;
            bass_sample_r = input_sample_r;
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
            bass_sample_l -= input_sample_l * (input_sample_l.abs() * glitch) * (input_sample_l.abs() * glitch);
            bass_sample_r -= input_sample_r * (input_sample_r.abs() * glitch) * (input_sample_r.abs() * glitch);
            if s.flip {
                s.iir_sample_cl = s.iir_sample_cl * (1.0 - iir_amount) + bass_sample_l * iir_amount;
                s.iir_sample_cr = s.iir_sample_cr * (1.0 - iir_amount) + bass_sample_r * iir_amount;
                bass_sample_l = s.iir_sample_cl;
                bass_sample_r = s.iir_sample_cr;
            } else {
                s.iir_sample_dl = s.iir_sample_dl * (1.0 - iir_amount) + bass_sample_l * iir_amount;
                s.iir_sample_dr = s.iir_sample_dr * (1.0 - iir_amount) + bass_sample_r * iir_amount;
                bass_sample_l = s.iir_sample_dl;
                bass_sample_r = s.iir_sample_dr;
            }
            s.flip = !s.flip;
            bridgerectifier = input_sample_l;
            randy = bias + s.fpd_l as f64 / u32::MAX as f64 * noise;
            if randy >= 0.0 && randy < 1.0 {
                bridgerectifier = input_sample_l * randy + s.second_sample_l * (1.0 - randy);
            }
            if randy >= 1.0 && randy < 2.0 {
                bridgerectifier = s.second_sample_l * (randy - 1.0) + s.third_sample_l * (2.0 - randy);
            }
            if randy >= 2.0 && randy < 3.0 {
                bridgerectifier = s.third_sample_l * (randy - 2.0) + s.fourth_sample_l * (3.0 - randy);
            }
            if randy >= 3.0 && randy < 4.0 {
                bridgerectifier = s.fourth_sample_l * (randy - 3.0) + s.fifth_sample_l * (4.0 - randy);
            }
            s.fifth_sample_l = s.fourth_sample_l;
            s.fourth_sample_l = s.third_sample_l;
            s.third_sample_l = s.second_sample_l;
            s.second_sample_l = input_sample_l;
            input_sample_l = bridgerectifier;
            bridgerectifier = input_sample_r;
            randy = bias + s.fpd_r as f64 / u32::MAX as f64 * noise;
            if randy >= 0.0 && randy < 1.0 {
                bridgerectifier = input_sample_r * randy + s.second_sample_r * (1.0 - randy);
            }
            if randy >= 1.0 && randy < 2.0 {
                bridgerectifier = s.second_sample_r * (randy - 1.0) + s.third_sample_r * (2.0 - randy);
            }
            if randy >= 2.0 && randy < 3.0 {
                bridgerectifier = s.third_sample_r * (randy - 2.0) + s.fourth_sample_r * (3.0 - randy);
            }
            if randy >= 3.0 && randy < 4.0 {
                bridgerectifier = s.fourth_sample_r * (randy - 3.0) + s.fifth_sample_r * (4.0 - randy);
            }
            s.fifth_sample_r = s.fourth_sample_r;
            s.fourth_sample_r = s.third_sample_r;
            s.third_sample_r = s.second_sample_r;
            s.second_sample_r = input_sample_r;
            input_sample_r = bridgerectifier;
            bridgerectifier = input_sample_l.abs() * density_a;
            if bridgerectifier > 1.57079633 {
                bridgerectifier = 1.57079633;
            }
            bridgerectifier = bridgerectifier.sin();
            if input_sample_l > 0.0 {
                input_sample_l = bridgerectifier / density_a;
            } else {
                input_sample_l = -bridgerectifier / density_a;
            }
            bridgerectifier = input_sample_r.abs() * density_a;
            if bridgerectifier > 1.57079633 {
                bridgerectifier = 1.57079633;
            }
            bridgerectifier = bridgerectifier.sin();
            if input_sample_r > 0.0 {
                input_sample_r = bridgerectifier / density_a;
            } else {
                input_sample_r = -bridgerectifier / density_a;
            }
            input_sample_l *= treble_gain_trim;
            input_sample_r *= treble_gain_trim;
            bass_sample_l *= bass_gain_trim;
            bass_sample_r *= bass_gain_trim;
            input_sample_l += bass_sample_l;
            input_sample_r += bass_sample_r;
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
