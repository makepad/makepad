// Ported from Compresaturator (CompresaturatorProc.cpp, Compresaturator.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Compresaturator (sub-effect id 3): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Drive", 0.5),
    pf("Clamp", 0.5),
    pf("Expand", 0.5),
    pf("Output", 1.0),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct Compresaturator {
    p: [f32; 5],
    sr: f32,
    fpd_l: u32,
    fpd_r: u32,
    d_count: i32,
    d_l: Vec<f32>,
    last_width_l: i32,
    pad_factor_l: f64,
    d_r: Vec<f32>,
    last_width_r: i32,
    pad_factor_r: f64,
}

impl Compresaturator {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Compresaturator> {
        let mut s = Box::new(Compresaturator {
            p: [0.5, 0.5, 0.5, 1.0, 1.0],
            sr: sr as f32,
            fpd_l: 0,
            fpd_r: 0,
            d_count: 0,
            d_l: vec![0.0; 11000],
            last_width_l: 0,
            pad_factor_l: 0.0,
            d_r: vec![0.0; 11000],
            last_width_r: 0,
            pad_factor_r: 0.0,
        });
        s.last_width_l = 500;
        s.last_width_r = 500;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for Compresaturator {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut inputgain: f64 = 10.0_f64.powf((s.p[0] as f64 * 24.0 - 12.0) / 20.0);
        let mut sat_comp: f64 = s.p[1] as f64 * 2.0;
        let mut widest_range: i32 = (s.p[2] * s.p[2] * s.p[2] * 5000.0) as i32;
        if widest_range < 50 {
            widest_range = 50;
        }
        sat_comp += widest_range as f64 / 3000.0 * sat_comp;
        let mut output: f64 = s.p[3] as f64;
        let mut wet: f64 = s.p[4] as f64;
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
            if s.d_count < 1 || s.d_count > 5000 {
                s.d_count = 5000;
            }
            let mut temp: f64 = input_sample_l;
            let mut vari_speed: f64 = 1.0 + s.pad_factor_l / s.last_width_l as f64 * sat_comp;
            if vari_speed < 1.0 {
                vari_speed = 1.0;
            }
            let mut totalgain: f64 = inputgain / vari_speed;
            if totalgain != 1.0 {
                input_sample_l *= totalgain;
                if totalgain < 1.0 {
                    temp *= totalgain;
                }
            }
            let mut bridgerectifier: f64 = input_sample_l.abs();
            let mut overspill: f64 = 0.0;
            let mut target_width: i32 = widest_range;
            if bridgerectifier < 0.01 {
                s.pad_factor_l *= 0.9999;
            }
            if bridgerectifier > 1.57079633 {
                bridgerectifier = 1.57079633;
                target_width = 8;
            }
            bridgerectifier = bridgerectifier.sin();
            if input_sample_l > 0.0 {
                input_sample_l = bridgerectifier;
                overspill = temp - bridgerectifier;
            }
            if input_sample_l < 0.0 {
                input_sample_l = -bridgerectifier;
                overspill = -temp - bridgerectifier;
            }
            temp = input_sample_r;
            vari_speed = 1.0 + s.pad_factor_r / s.last_width_r as f64 * sat_comp;
            if vari_speed < 1.0 {
                vari_speed = 1.0;
            }
            totalgain = inputgain / vari_speed;
            if totalgain != 1.0 {
                input_sample_r *= totalgain;
                if totalgain < 1.0 {
                    temp *= totalgain;
                }
            }
            bridgerectifier = input_sample_r.abs();
            overspill = 0.0;
            target_width = widest_range;
            if bridgerectifier < 0.01 {
                s.pad_factor_r *= 0.9999;
            }
            if bridgerectifier > 1.57079633 {
                bridgerectifier = 1.57079633;
                target_width = 8;
            }
            bridgerectifier = bridgerectifier.sin();
            if input_sample_r > 0.0 {
                input_sample_r = bridgerectifier;
                overspill = temp - bridgerectifier;
            }
            if input_sample_r < 0.0 {
                input_sample_r = -bridgerectifier;
                overspill = -temp - bridgerectifier;
            }
            s.d_l[s.d_count as usize] = (overspill * sat_comp) as f32;
            s.d_l[(s.d_count + 5000) as usize] = s.d_l[s.d_count as usize];
            s.d_r[s.d_count as usize] = (overspill * sat_comp) as f32;
            s.d_r[(s.d_count + 5000) as usize] = s.d_r[s.d_count as usize];
            s.d_count -= 1;
            s.pad_factor_l += s.d_l[s.d_count as usize] as f64;
            let mut randy: f64 = s.fpd_l as f64 / u32::MAX as f64;
            if target_width as f64 * randy > s.last_width_l as f64 {
                s.last_width_l += 1;
            } else {
                s.pad_factor_l -= s.d_l[(s.d_count + s.last_width_l) as usize] as f64;
                if target_width < s.last_width_l {
                    s.last_width_l -= 1;
                    if s.last_width_l < 2 {
                        s.last_width_l = 2;
                    }
                    s.pad_factor_l -= s.d_l[(s.d_count + s.last_width_l) as usize] as f64;
                }
            }
            if s.pad_factor_l < 0.0 {
                s.pad_factor_l = 0.0;
            }
            s.pad_factor_r += s.d_r[s.d_count as usize] as f64;
            randy = s.fpd_r as f64 / u32::MAX as f64;
            if target_width as f64 * randy > s.last_width_r as f64 {
                s.last_width_r += 1;
            } else {
                s.pad_factor_r -= s.d_r[(s.d_count + s.last_width_r) as usize] as f64;
                if target_width < s.last_width_r {
                    s.last_width_r -= 1;
                    if s.last_width_r < 2 {
                        s.last_width_r = 2;
                    }
                    s.pad_factor_r -= s.d_r[(s.d_count + s.last_width_r) as usize] as f64;
                }
            }
            if s.pad_factor_r < 0.0 {
                s.pad_factor_r = 0.0;
            }
            if output < 1.0 {
                input_sample_l *= output;
                input_sample_r *= output;
            }
            if wet < 1.0 {
                input_sample_l = input_sample_l * wet + dry_sample_l * (1.0 - wet);
                input_sample_r = input_sample_r * wet + dry_sample_r * (1.0 - wet);
            }
            let mut expon: i32 = 0;
            expon = frexpf_exp(input_sample_l as f32);
            s.fpd_l ^= s.fpd_l << 13;
            s.fpd_l ^= s.fpd_l >> 17;
            s.fpd_l ^= s.fpd_l << 5;
            input_sample_l += s.fpd_l as i32 as f64 * 5.960464655174751e-36 * 2.0_f64.powf((expon + 62) as f64);
            expon = frexpf_exp(input_sample_r as f32);
            s.fpd_r ^= s.fpd_r << 13;
            s.fpd_r ^= s.fpd_r >> 17;
            s.fpd_r ^= s.fpd_r << 5;
            input_sample_r += s.fpd_r as i32 as f64 * 5.960464655174751e-36 * 2.0_f64.powf((expon + 62) as f64);
            l[smp] = input_sample_l as f32;
            r[smp] = input_sample_r as f32;
        }
    }
}
