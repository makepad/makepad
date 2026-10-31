// Ported from VariMu (VariMuProc.cpp, VariMu.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Vari-Mu (sub-effect id 12): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Intensty", 0.0),
    pf("Speed", 0.5),
    pf("Output", 1.0),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct VariMu {
    p: [f32; 4],
    sr: f32,
    mu_vary_l: f64,
    mu_attack_l: f64,
    mu_new_speed_l: f64,
    mu_speed_al: f64,
    mu_speed_bl: f64,
    mu_coefficient_al: f64,
    mu_coefficient_bl: f64,
    previous_l: f64,
    mu_vary_r: f64,
    mu_attack_r: f64,
    mu_new_speed_r: f64,
    mu_speed_ar: f64,
    mu_speed_br: f64,
    mu_coefficient_ar: f64,
    mu_coefficient_br: f64,
    previous_r: f64,
    flip: bool,
    fpd_l: u32,
    fpd_r: u32,
    /// upstream: a function-local static (process-wide there; per instance here)
    noisesource_l: i32,
    /// upstream: a function-local static (process-wide there; per instance here)
    noisesource_r: i32,
}

impl VariMu {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<VariMu> {
        let mut s = Box::new(VariMu {
            p: [0.0, 0.5, 1.0, 1.0],
            sr: sr as f32,
            mu_vary_l: 0.0,
            mu_attack_l: 0.0,
            mu_new_speed_l: 0.0,
            mu_speed_al: 0.0,
            mu_speed_bl: 0.0,
            mu_coefficient_al: 0.0,
            mu_coefficient_bl: 0.0,
            previous_l: 0.0,
            mu_vary_r: 0.0,
            mu_attack_r: 0.0,
            mu_new_speed_r: 0.0,
            mu_speed_ar: 0.0,
            mu_speed_br: 0.0,
            mu_coefficient_ar: 0.0,
            mu_coefficient_br: 0.0,
            previous_r: 0.0,
            flip: false,
            fpd_l: 0,
            fpd_r: 0,
            noisesource_l: 0,
            noisesource_r: 850010,
        });
        s.mu_speed_al = 10000.0;
        s.mu_speed_bl = 10000.0;
        s.mu_coefficient_al = 1.0;
        s.mu_coefficient_bl = 1.0;
        s.mu_vary_l = 1.0;
        s.mu_speed_ar = 10000.0;
        s.mu_speed_br = 10000.0;
        s.mu_coefficient_ar = 1.0;
        s.mu_coefficient_br = 1.0;
        s.mu_vary_r = 1.0;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for VariMu {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut overallscale: f64 = 2.0;
        overallscale /= 44100.0;
        overallscale *= s.sr as f64;
        let mut threshold: f64 = 1.001 - (1.0 - (1.0 - s.p[0] as f64).powf(3.0));
        let mut mu_makeup_gain: f64 = (1.0 / threshold).sqrt();
        mu_makeup_gain = (mu_makeup_gain + mu_makeup_gain.sqrt()) / 2.0;
        mu_makeup_gain = mu_makeup_gain.sqrt();
        let mut out_gain: f64 = mu_makeup_gain.sqrt();
        let mut release: f64 = (1.15 - s.p[1] as f64).powf(5.0) * 32768.0;
        release /= overallscale;
        let mut fastest: f64 = release.sqrt();
        let mut coefficient: f64 = 0.0;
        let mut output: f64 = out_gain * s.p[2] as f64;
        let mut wet: f64 = s.p[3] as f64;
        let mut squared_sample_l: f64 = 0.0;
        let mut squared_sample_r: f64 = 0.0;
        for smp in 0..l.len() {
            let mut input_sample_l: f64 = l[smp] as f64;
            let mut input_sample_r: f64 = r[smp] as f64;
            let mut residue: i32 = 0;
            let mut applyresidue: f64 = 0.0;
            s.noisesource_l = s.noisesource_l % 1700021;
            s.noisesource_l += 1;
            residue = s.noisesource_l.wrapping_mul(s.noisesource_l);
            residue = residue % 170003;
            residue *= residue;
            residue = residue % 17011;
            residue *= residue;
            residue = residue % 1709;
            residue *= residue;
            residue = residue % 173;
            residue *= residue;
            residue = residue % 17;
            applyresidue = residue as f64;
            applyresidue *= 0.00000001;
            applyresidue *= 0.00000001;
            input_sample_l += applyresidue;
            if input_sample_l < 1.2e-38 && -input_sample_l < 1.2e-38 {
                input_sample_l -= applyresidue;
            }
            s.noisesource_r = s.noisesource_r % 1700021;
            s.noisesource_r += 1;
            residue = s.noisesource_r.wrapping_mul(s.noisesource_r);
            residue = residue % 170003;
            residue *= residue;
            residue = residue % 17011;
            residue *= residue;
            residue = residue % 1709;
            residue *= residue;
            residue = residue % 173;
            residue *= residue;
            residue = residue % 17;
            applyresidue = residue as f64;
            applyresidue *= 0.00000001;
            applyresidue *= 0.00000001;
            input_sample_r += applyresidue;
            if input_sample_r < 1.2e-38 && -input_sample_r < 1.2e-38 {
                input_sample_r -= applyresidue;
            }
            let mut dry_sample_l: f64 = input_sample_l;
            let mut dry_sample_r: f64 = input_sample_r;
            if input_sample_l.abs() > s.previous_l.abs() {
                squared_sample_l = s.previous_l * s.previous_l;
            } else {
                squared_sample_l = input_sample_l * input_sample_l;
            }
            s.previous_l = input_sample_l;
            input_sample_l *= mu_makeup_gain;
            if input_sample_r.abs() > s.previous_r.abs() {
                squared_sample_r = s.previous_r * s.previous_r;
            } else {
                squared_sample_r = input_sample_r * input_sample_r;
            }
            s.previous_r = input_sample_r;
            input_sample_r *= mu_makeup_gain;
            if s.flip {
                if squared_sample_l.abs() > threshold {
                    s.mu_vary_l = threshold / squared_sample_l.abs();
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
                s.mu_new_speed_l = s.mu_new_speed_l + (squared_sample_l * release).abs() + fastest;
                s.mu_speed_al = s.mu_new_speed_l / s.mu_speed_al;
            } else {
                if squared_sample_l.abs() > threshold {
                    s.mu_vary_l = threshold / squared_sample_l.abs();
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
                s.mu_new_speed_l = s.mu_new_speed_l + (squared_sample_l * release).abs() + fastest;
                s.mu_speed_bl = s.mu_new_speed_l / s.mu_speed_bl;
            }
            if s.flip {
                if squared_sample_r.abs() > threshold {
                    s.mu_vary_r = threshold / squared_sample_r.abs();
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
                s.mu_new_speed_r = s.mu_new_speed_r + (squared_sample_r * release).abs() + fastest;
                s.mu_speed_ar = s.mu_new_speed_r / s.mu_speed_ar;
            } else {
                if squared_sample_r.abs() > threshold {
                    s.mu_vary_r = threshold / squared_sample_r.abs();
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
                s.mu_new_speed_r = s.mu_new_speed_r + (squared_sample_r * release).abs() + fastest;
                s.mu_speed_br = s.mu_new_speed_r / s.mu_speed_br;
            }
            if s.flip {
                coefficient = (s.mu_coefficient_al + s.mu_coefficient_al.powf(2.0)) / 2.0;
                input_sample_l *= coefficient;
                coefficient = (s.mu_coefficient_ar + s.mu_coefficient_ar.powf(2.0)) / 2.0;
                input_sample_r *= coefficient;
            } else {
                coefficient = (s.mu_coefficient_bl + s.mu_coefficient_bl.powf(2.0)) / 2.0;
                input_sample_l *= coefficient;
                coefficient = (s.mu_coefficient_br + s.mu_coefficient_br.powf(2.0)) / 2.0;
                input_sample_r *= coefficient;
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
