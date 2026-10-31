// Ported from Pressure4 (Pressure4Proc.cpp, Pressure4.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Pressure (sub-effect id 9): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Pressure", 0.0),
    pf("Speed", 0.2),
    pf("Mewiness", 1.0),
    pf("Output Gain", 1.0),
];

#[allow(dead_code)]
pub struct Pressure4 {
    p: [f32; 4],
    sr: f32,
    mu_vary: f64,
    mu_attack: f64,
    mu_new_speed: f64,
    mu_speed_a: f64,
    mu_speed_b: f64,
    mu_coefficient_a: f64,
    mu_coefficient_b: f64,
    fpd_l: u32,
    fpd_r: u32,
    flip: bool,
}

impl Pressure4 {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Pressure4> {
        let mut s = Box::new(Pressure4 {
            p: [0.0, 0.2, 1.0, 1.0],
            sr: sr as f32,
            mu_vary: 0.0,
            mu_attack: 0.0,
            mu_new_speed: 0.0,
            mu_speed_a: 0.0,
            mu_speed_b: 0.0,
            mu_coefficient_a: 0.0,
            mu_coefficient_b: 0.0,
            fpd_l: 0,
            fpd_r: 0,
            flip: false,
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s.mu_speed_a = 10000.0;
        s.mu_speed_b = 10000.0;
        s.mu_coefficient_a = 1.0;
        s.mu_coefficient_b = 1.0;
        s.mu_vary = 1.0;
        s
    }
}

impl Sub for Pressure4 {
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
        let mut threshold: f64 = 1.0 - s.p[0] as f64 * 0.95;
        let mut mu_makeup_gain: f64 = 1.0 / threshold;
        let mut release: f64 = (1.28 - s.p[1] as f64).powf(5.0) * 32768.0;
        release /= overallscale;
        let mut fastest: f64 = release.sqrt();
        let mut bridgerectifier: f64 = 0.0;
        let mut coefficient: f64 = 0.0;
        let mut input_sense: f64 = 0.0;
        let mut mewiness: f64 = s.p[2] as f64 * 2.0 - 1.0;
        let mut unmewiness: f64 = 0.0;
        let mut output_gain: f64 = s.p[3] as f64;
        let mut positivemu: bool = false;
        if mewiness >= 0.0 {
            positivemu = true;
            unmewiness = 1.0 - mewiness;
        } else {
            positivemu = false;
            mewiness = -mewiness;
            unmewiness = 1.0 - mewiness;
        }
        let mut input_sample_l: f64 = 0.0;
        let mut input_sample_r: f64 = 0.0;
        for smp in 0..l.len() {
            input_sample_l = l[smp] as f64;
            input_sample_r = r[smp] as f64;
            if input_sample_l.abs() < 1.18e-23 {
                input_sample_l = s.fpd_l as f64 * 1.18e-17;
            }
            if input_sample_r.abs() < 1.18e-23 {
                input_sample_r = s.fpd_r as f64 * 1.18e-17;
            }
            input_sample_l = input_sample_l * mu_makeup_gain;
            input_sample_r = input_sample_r * mu_makeup_gain;
            input_sense = input_sample_l.abs();
            if input_sample_r.abs() > input_sense {
                input_sense = input_sample_r.abs();
            }
            if s.flip {
                if input_sense > threshold {
                    s.mu_vary = threshold / input_sense;
                    s.mu_attack = s.mu_speed_a.abs().sqrt();
                    s.mu_coefficient_a = s.mu_coefficient_a * (s.mu_attack - 1.0);
                    if s.mu_vary < threshold {
                        s.mu_coefficient_a = s.mu_coefficient_a + threshold;
                    } else {
                        s.mu_coefficient_a = s.mu_coefficient_a + s.mu_vary;
                    }
                    s.mu_coefficient_a = s.mu_coefficient_a / s.mu_attack;
                } else {
                    s.mu_coefficient_a = s.mu_coefficient_a * (s.mu_speed_a * s.mu_speed_a - 1.0);
                    s.mu_coefficient_a = s.mu_coefficient_a + 1.0;
                    s.mu_coefficient_a = s.mu_coefficient_a / (s.mu_speed_a * s.mu_speed_a);
                }
                s.mu_new_speed = s.mu_speed_a * (s.mu_speed_a - 1.0);
                s.mu_new_speed = s.mu_new_speed + (input_sense * release).abs() + fastest;
                s.mu_speed_a = s.mu_new_speed / s.mu_speed_a;
            } else {
                if input_sense > threshold {
                    s.mu_vary = threshold / input_sense;
                    s.mu_attack = s.mu_speed_b.abs().sqrt();
                    s.mu_coefficient_b = s.mu_coefficient_b * (s.mu_attack - 1.0);
                    if s.mu_vary < threshold {
                        s.mu_coefficient_b = s.mu_coefficient_b + threshold;
                    } else {
                        s.mu_coefficient_b = s.mu_coefficient_b + s.mu_vary;
                    }
                    s.mu_coefficient_b = s.mu_coefficient_b / s.mu_attack;
                } else {
                    s.mu_coefficient_b = s.mu_coefficient_b * (s.mu_speed_b * s.mu_speed_b - 1.0);
                    s.mu_coefficient_b = s.mu_coefficient_b + 1.0;
                    s.mu_coefficient_b = s.mu_coefficient_b / (s.mu_speed_b * s.mu_speed_b);
                }
                s.mu_new_speed = s.mu_speed_b * (s.mu_speed_b - 1.0);
                s.mu_new_speed = s.mu_new_speed + (input_sense * release).abs() + fastest;
                s.mu_speed_b = s.mu_new_speed / s.mu_speed_b;
            }
            if s.flip {
                if positivemu {
                    coefficient = s.mu_coefficient_a.powf(2.0);
                } else {
                    coefficient = s.mu_coefficient_a.sqrt();
                }
                coefficient = coefficient * mewiness + s.mu_coefficient_a * unmewiness;
                input_sample_l *= coefficient;
                input_sample_r *= coefficient;
            } else {
                if positivemu {
                    coefficient = s.mu_coefficient_b.powf(2.0);
                } else {
                    coefficient = s.mu_coefficient_b.sqrt();
                }
                coefficient = coefficient * mewiness + s.mu_coefficient_b * unmewiness;
                input_sample_l *= coefficient;
                input_sample_r *= coefficient;
            }
            if output_gain != 1.0 {
                input_sample_l *= output_gain;
                input_sample_r *= output_gain;
            }
            bridgerectifier = input_sample_l.abs();
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
