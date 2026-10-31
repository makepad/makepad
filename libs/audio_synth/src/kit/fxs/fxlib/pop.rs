// Ported from Pop (PopProc.cpp, Pop.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Pop (sub-effect id 8): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Intenst", 0.3),
    pf("Output", 1.0),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct Pop {
    p: [f32; 3],
    sr: f32,
    fpd_l: u32,
    fpd_r: u32,
    mu_vary_l: f64,
    mu_attack_l: f64,
    mu_new_speed_l: f64,
    mu_speed_al: f64,
    mu_speed_bl: f64,
    mu_coefficient_al: f64,
    mu_coefficient_bl: f64,
    thicken_l: f64,
    previous_l: f64,
    previous2_l: f64,
    previous3_l: f64,
    previous4_l: f64,
    previous5_l: f64,
    d_l: Vec<f64>,
    mu_vary_r: f64,
    mu_attack_r: f64,
    mu_new_speed_r: f64,
    mu_speed_ar: f64,
    mu_speed_br: f64,
    mu_coefficient_ar: f64,
    mu_coefficient_br: f64,
    thicken_r: f64,
    previous_r: f64,
    previous2_r: f64,
    previous3_r: f64,
    previous4_r: f64,
    previous5_r: f64,
    d_r: Vec<f64>,
    delay: i32,
    flip: bool,
}

impl Pop {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Pop> {
        let mut s = Box::new(Pop {
            p: [0.3, 1.0, 1.0],
            sr: sr as f32,
            fpd_l: 0,
            fpd_r: 0,
            mu_vary_l: 0.0,
            mu_attack_l: 0.0,
            mu_new_speed_l: 0.0,
            mu_speed_al: 0.0,
            mu_speed_bl: 0.0,
            mu_coefficient_al: 0.0,
            mu_coefficient_bl: 0.0,
            thicken_l: 0.0,
            previous_l: 0.0,
            previous2_l: 0.0,
            previous3_l: 0.0,
            previous4_l: 0.0,
            previous5_l: 0.0,
            d_l: vec![0.0; 10001],
            mu_vary_r: 0.0,
            mu_attack_r: 0.0,
            mu_new_speed_r: 0.0,
            mu_speed_ar: 0.0,
            mu_speed_br: 0.0,
            mu_coefficient_ar: 0.0,
            mu_coefficient_br: 0.0,
            thicken_r: 0.0,
            previous_r: 0.0,
            previous2_r: 0.0,
            previous3_r: 0.0,
            previous4_r: 0.0,
            previous5_r: 0.0,
            d_r: vec![0.0; 10001],
            delay: 0,
            flip: false,
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s.mu_speed_al = 10000.0;
        s.mu_speed_bl = 10000.0;
        s.mu_coefficient_al = 1.0;
        s.mu_coefficient_bl = 1.0;
        s.thicken_l = 1.0;
        s.mu_vary_l = 1.0;
        s.mu_speed_ar = 10000.0;
        s.mu_speed_br = 10000.0;
        s.mu_coefficient_ar = 1.0;
        s.mu_coefficient_br = 1.0;
        s.thicken_r = 1.0;
        s.mu_vary_r = 1.0;
        s
    }
}

impl Sub for Pop {
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
        let mut high_gain_offset: f64 = (s.p[0] as f64).powf(2.0) * 0.023;
        let mut threshold: f64 = 1.001 - (1.0 - (1.0 - s.p[0] as f64).powf(5.0));
        let mut mu_makeup_gain: f64 = (1.0 / threshold).sqrt();
        let mut release: f64 = s.p[0] as f64 * 100000.0 + 300000.0;
        let mut maxdelay: i32 = (1450.0 * overallscale) as i32;
        if maxdelay > 9999 {
            maxdelay = 9999;
        }
        release /= overallscale;
        let mut fastest: f64 = release.sqrt();
        let mut output: f64 = s.p[1] as f64;
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
            s.d_l[s.delay as usize] = input_sample_l;
            s.d_r[s.delay as usize] = input_sample_r;
            s.delay -= 1;
            if s.delay < 0 || s.delay > maxdelay {
                s.delay = maxdelay;
            }
            input_sample_l = input_sample_l * s.thicken_l + s.d_l[s.delay as usize] * (1.0 - s.thicken_l);
            input_sample_r = input_sample_r * s.thicken_r + s.d_r[s.delay as usize] * (1.0 - s.thicken_r);
            let mut lowest_sample_l: f64 = input_sample_l;
            if input_sample_l.abs() > s.previous_l.abs() {
                lowest_sample_l = s.previous_l;
            }
            if lowest_sample_l.abs() > s.previous2_l.abs() {
                lowest_sample_l = (lowest_sample_l + s.previous2_l) / 1.99;
            }
            if lowest_sample_l.abs() > s.previous3_l.abs() {
                lowest_sample_l = (lowest_sample_l + s.previous3_l) / 1.98;
            }
            if lowest_sample_l.abs() > s.previous4_l.abs() {
                lowest_sample_l = (lowest_sample_l + s.previous4_l) / 1.97;
            }
            if lowest_sample_l.abs() > s.previous5_l.abs() {
                lowest_sample_l = (lowest_sample_l + s.previous5_l) / 1.96;
            }
            s.previous5_l = s.previous4_l;
            s.previous4_l = s.previous3_l;
            s.previous3_l = s.previous2_l;
            s.previous2_l = s.previous_l;
            s.previous_l = input_sample_l;
            input_sample_l *= mu_makeup_gain;
            let mut punchiness_l: f64 = 0.95 - (input_sample_l * 0.08).abs();
            if punchiness_l < 0.65 {
                punchiness_l = 0.65;
            }
            let mut lowest_sample_r: f64 = input_sample_r;
            if input_sample_r.abs() > s.previous_r.abs() {
                lowest_sample_r = s.previous_r;
            }
            if lowest_sample_r.abs() > s.previous2_r.abs() {
                lowest_sample_r = (lowest_sample_r + s.previous2_r) / 1.99;
            }
            if lowest_sample_r.abs() > s.previous3_r.abs() {
                lowest_sample_r = (lowest_sample_r + s.previous3_r) / 1.98;
            }
            if lowest_sample_r.abs() > s.previous4_r.abs() {
                lowest_sample_r = (lowest_sample_r + s.previous4_r) / 1.97;
            }
            if lowest_sample_r.abs() > s.previous5_r.abs() {
                lowest_sample_r = (lowest_sample_r + s.previous5_r) / 1.96;
            }
            s.previous5_r = s.previous4_r;
            s.previous4_r = s.previous3_r;
            s.previous3_r = s.previous2_r;
            s.previous2_r = s.previous_r;
            s.previous_r = input_sample_r;
            input_sample_r *= mu_makeup_gain;
            let mut punchiness_r: f64 = 0.95 - (input_sample_r * 0.08).abs();
            if punchiness_r < 0.65 {
                punchiness_r = 0.65;
            }
            if s.flip {
                if lowest_sample_l.abs() > threshold {
                    s.mu_vary_l = threshold / lowest_sample_l.abs();
                    s.mu_attack_l = s.mu_speed_al.abs().sqrt();
                    s.mu_coefficient_al = s.mu_coefficient_al * (s.mu_attack_l - 1.0);
                    if s.mu_vary_l < threshold {
                        s.mu_coefficient_al = s.mu_coefficient_al + threshold;
                    } else {
                        s.mu_coefficient_al = s.mu_coefficient_al + s.mu_vary_l;
                    }
                    s.mu_coefficient_al = s.mu_coefficient_al / s.mu_attack_l;
                } else {
                    s.mu_coefficient_al = s.mu_coefficient_al * (s.mu_speed_al * s.mu_speed_al - 1.0);
                    s.mu_coefficient_al = s.mu_coefficient_al + 1.0;
                    s.mu_coefficient_al = s.mu_coefficient_al / (s.mu_speed_al * s.mu_speed_al);
                }
                s.mu_new_speed_l = s.mu_speed_al * (s.mu_speed_al - 1.0);
                s.mu_new_speed_l = s.mu_new_speed_l + (lowest_sample_l * release).abs() + fastest;
                s.mu_speed_al = s.mu_new_speed_l / s.mu_speed_al;
            } else {
                if lowest_sample_l.abs() > threshold {
                    s.mu_vary_l = threshold / lowest_sample_l.abs();
                    s.mu_attack_l = s.mu_speed_bl.abs().sqrt();
                    s.mu_coefficient_bl = s.mu_coefficient_bl * (s.mu_attack_l - 1.0);
                    if s.mu_vary_l < threshold {
                        s.mu_coefficient_bl = s.mu_coefficient_bl + threshold;
                    } else {
                        s.mu_coefficient_bl = s.mu_coefficient_bl + s.mu_vary_l;
                    }
                    s.mu_coefficient_bl = s.mu_coefficient_bl / s.mu_attack_l;
                } else {
                    s.mu_coefficient_bl = s.mu_coefficient_bl * (s.mu_speed_bl * s.mu_speed_bl - 1.0);
                    s.mu_coefficient_bl = s.mu_coefficient_bl + 1.0;
                    s.mu_coefficient_bl = s.mu_coefficient_bl / (s.mu_speed_bl * s.mu_speed_bl);
                }
                s.mu_new_speed_l = s.mu_speed_bl * (s.mu_speed_bl - 1.0);
                s.mu_new_speed_l = s.mu_new_speed_l + (lowest_sample_l * release).abs() + fastest;
                s.mu_speed_bl = s.mu_new_speed_l / s.mu_speed_bl;
            }
            if s.flip {
                if lowest_sample_r.abs() > threshold {
                    s.mu_vary_r = threshold / lowest_sample_r.abs();
                    s.mu_attack_r = s.mu_speed_ar.abs().sqrt();
                    s.mu_coefficient_ar = s.mu_coefficient_ar * (s.mu_attack_r - 1.0);
                    if s.mu_vary_r < threshold {
                        s.mu_coefficient_ar = s.mu_coefficient_ar + threshold;
                    } else {
                        s.mu_coefficient_ar = s.mu_coefficient_ar + s.mu_vary_r;
                    }
                    s.mu_coefficient_ar = s.mu_coefficient_ar / s.mu_attack_r;
                } else {
                    s.mu_coefficient_ar = s.mu_coefficient_ar * (s.mu_speed_ar * s.mu_speed_ar - 1.0);
                    s.mu_coefficient_ar = s.mu_coefficient_ar + 1.0;
                    s.mu_coefficient_ar = s.mu_coefficient_ar / (s.mu_speed_ar * s.mu_speed_ar);
                }
                s.mu_new_speed_r = s.mu_speed_ar * (s.mu_speed_ar - 1.0);
                s.mu_new_speed_r = s.mu_new_speed_r + (lowest_sample_r * release).abs() + fastest;
                s.mu_speed_ar = s.mu_new_speed_r / s.mu_speed_ar;
            } else {
                if lowest_sample_r.abs() > threshold {
                    s.mu_vary_r = threshold / lowest_sample_r.abs();
                    s.mu_attack_r = s.mu_speed_br.abs().sqrt();
                    s.mu_coefficient_br = s.mu_coefficient_br * (s.mu_attack_r - 1.0);
                    if s.mu_vary_r < threshold {
                        s.mu_coefficient_br = s.mu_coefficient_br + threshold;
                    } else {
                        s.mu_coefficient_br = s.mu_coefficient_br + s.mu_vary_r;
                    }
                    s.mu_coefficient_br = s.mu_coefficient_br / s.mu_attack_r;
                } else {
                    s.mu_coefficient_br = s.mu_coefficient_br * (s.mu_speed_br * s.mu_speed_br - 1.0);
                    s.mu_coefficient_br = s.mu_coefficient_br + 1.0;
                    s.mu_coefficient_br = s.mu_coefficient_br / (s.mu_speed_br * s.mu_speed_br);
                }
                s.mu_new_speed_r = s.mu_speed_br * (s.mu_speed_br - 1.0);
                s.mu_new_speed_r = s.mu_new_speed_r + (lowest_sample_r * release).abs() + fastest;
                s.mu_speed_br = s.mu_new_speed_r / s.mu_speed_br;
            }
            let mut coefficient_l: f64 = high_gain_offset;
            if s.flip {
                coefficient_l += s.mu_coefficient_al.powf(2.0);
            } else {
                coefficient_l += s.mu_coefficient_bl.powf(2.0);
            }
            input_sample_l *= coefficient_l;
            s.thicken_l = coefficient_l / 5.0 + punchiness_l;
            s.thicken_l = 1.0 - wet + wet * s.thicken_l;
            let mut coefficient_r: f64 = high_gain_offset;
            if s.flip {
                coefficient_r += s.mu_coefficient_ar.powf(2.0);
            } else {
                coefficient_r += s.mu_coefficient_br.powf(2.0);
            }
            input_sample_r *= coefficient_r;
            s.thicken_r = coefficient_r / 5.0 + punchiness_r;
            s.thicken_r = 1.0 - wet + wet * s.thicken_r;
            let mut bridgerectifier: f64 = input_sample_l.abs();
            if bridgerectifier > 1.2533141373155 {
                bridgerectifier = 1.2533141373155;
            }
            bridgerectifier = (bridgerectifier * bridgerectifier.abs()).sin() / (if bridgerectifier.abs() == 0.0 { 1.0 } else { bridgerectifier.abs() });
            if input_sample_l > 0.0 {
                input_sample_l = input_sample_l * coefficient_l + bridgerectifier * (1.0 - coefficient_l);
            } else {
                input_sample_l = input_sample_l * coefficient_l - bridgerectifier * (1.0 - coefficient_l);
            }
            bridgerectifier = input_sample_r.abs();
            if bridgerectifier > 1.2533141373155 {
                bridgerectifier = 1.2533141373155;
            }
            bridgerectifier = (bridgerectifier * bridgerectifier.abs()).sin() / (if bridgerectifier.abs() == 0.0 { 1.0 } else { bridgerectifier.abs() });
            if input_sample_r > 0.0 {
                input_sample_r = input_sample_r * coefficient_r + bridgerectifier * (1.0 - coefficient_r);
            } else {
                input_sample_r = input_sample_r * coefficient_r - bridgerectifier * (1.0 - coefficient_r);
            }
            s.flip = !s.flip;
            if output < 1.0 {
                input_sample_l *= output;
                input_sample_r *= output;
            }
            if wet < 1.0 {
                input_sample_l = dry_sample_l * (1.0 - wet) + input_sample_l * wet;
                input_sample_r = dry_sample_r * (1.0 - wet) + input_sample_r * wet;
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
