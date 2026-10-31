// Ported from BlockParty (BlockPartyProc.cpp, BlockParty.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Block Party (sub-effect id 1): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Pound", 0.0),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct BlockParty {
    p: [f32; 2],
    sr: f32,
    fpd_l: u32,
    fpd_r: u32,
    mu_vary_l: f64,
    mu_attack_l: f64,
    mu_new_speed_l: f64,
    mu_speed_al: f64,
    mu_speed_bl: f64,
    mu_speed_cl: f64,
    mu_speed_dl: f64,
    mu_speed_el: f64,
    mu_coefficient_al: f64,
    mu_coefficient_bl: f64,
    mu_coefficient_cl: f64,
    mu_coefficient_dl: f64,
    mu_coefficient_el: f64,
    last_coefficient_al: f64,
    last_coefficient_bl: f64,
    last_coefficient_cl: f64,
    last_coefficient_dl: f64,
    merged_coefficients_l: f64,
    threshold_l: f64,
    threshold_bl: f64,
    mu_vary_r: f64,
    mu_attack_r: f64,
    mu_new_speed_r: f64,
    mu_speed_ar: f64,
    mu_speed_br: f64,
    mu_speed_cr: f64,
    mu_speed_dr: f64,
    mu_speed_er: f64,
    mu_coefficient_ar: f64,
    mu_coefficient_br: f64,
    mu_coefficient_cr: f64,
    mu_coefficient_dr: f64,
    mu_coefficient_er: f64,
    last_coefficient_ar: f64,
    last_coefficient_br: f64,
    last_coefficient_cr: f64,
    last_coefficient_dr: f64,
    merged_coefficients_r: f64,
    threshold_r: f64,
    threshold_br: f64,
    count: i32,
    fp_flip: bool,
}

impl BlockParty {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<BlockParty> {
        let mut s = Box::new(BlockParty {
            p: [0.0, 1.0],
            sr: sr as f32,
            fpd_l: 0,
            fpd_r: 0,
            mu_vary_l: 0.0,
            mu_attack_l: 0.0,
            mu_new_speed_l: 0.0,
            mu_speed_al: 0.0,
            mu_speed_bl: 0.0,
            mu_speed_cl: 0.0,
            mu_speed_dl: 0.0,
            mu_speed_el: 0.0,
            mu_coefficient_al: 0.0,
            mu_coefficient_bl: 0.0,
            mu_coefficient_cl: 0.0,
            mu_coefficient_dl: 0.0,
            mu_coefficient_el: 0.0,
            last_coefficient_al: 0.0,
            last_coefficient_bl: 0.0,
            last_coefficient_cl: 0.0,
            last_coefficient_dl: 0.0,
            merged_coefficients_l: 0.0,
            threshold_l: 0.0,
            threshold_bl: 0.0,
            mu_vary_r: 0.0,
            mu_attack_r: 0.0,
            mu_new_speed_r: 0.0,
            mu_speed_ar: 0.0,
            mu_speed_br: 0.0,
            mu_speed_cr: 0.0,
            mu_speed_dr: 0.0,
            mu_speed_er: 0.0,
            mu_coefficient_ar: 0.0,
            mu_coefficient_br: 0.0,
            mu_coefficient_cr: 0.0,
            mu_coefficient_dr: 0.0,
            mu_coefficient_er: 0.0,
            last_coefficient_ar: 0.0,
            last_coefficient_br: 0.0,
            last_coefficient_cr: 0.0,
            last_coefficient_dr: 0.0,
            merged_coefficients_r: 0.0,
            threshold_r: 0.0,
            threshold_br: 0.0,
            count: 0,
            fp_flip: false,
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s.mu_speed_al = 10000.0;
        s.mu_speed_bl = 10000.0;
        s.mu_speed_cl = 10000.0;
        s.mu_speed_dl = 10000.0;
        s.mu_speed_el = 10000.0;
        s.mu_coefficient_al = 1.0;
        s.mu_coefficient_bl = 1.0;
        s.mu_coefficient_cl = 1.0;
        s.mu_coefficient_dl = 1.0;
        s.mu_coefficient_el = 1.0;
        s.last_coefficient_al = 1.0;
        s.last_coefficient_bl = 1.0;
        s.last_coefficient_cl = 1.0;
        s.last_coefficient_dl = 1.0;
        s.merged_coefficients_l = 1.0;
        s.threshold_l = 1.0;
        s.threshold_bl = 1.0;
        s.mu_vary_l = 1.0;
        s.mu_speed_ar = 10000.0;
        s.mu_speed_br = 10000.0;
        s.mu_speed_cr = 10000.0;
        s.mu_speed_dr = 10000.0;
        s.mu_speed_er = 10000.0;
        s.mu_coefficient_ar = 1.0;
        s.mu_coefficient_br = 1.0;
        s.mu_coefficient_cr = 1.0;
        s.mu_coefficient_dr = 1.0;
        s.mu_coefficient_er = 1.0;
        s.last_coefficient_ar = 1.0;
        s.last_coefficient_br = 1.0;
        s.last_coefficient_cr = 1.0;
        s.last_coefficient_dr = 1.0;
        s.merged_coefficients_r = 1.0;
        s.threshold_r = 1.0;
        s.threshold_br = 1.0;
        s.mu_vary_r = 1.0;
        s.count = 1;
        s.fp_flip = true;
        s
    }
}

impl Sub for BlockParty {
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
        let mut targetthreshold: f64 = 1.01 - (1.0 - (1.0 - s.p[0] as f64 * 0.5).powf(4.0));
        let mut wet: f64 = s.p[1] as f64;
        let mut voicing: f64 = 0.618033988749894848204586;
        if overallscale > 0.0 {
            voicing /= overallscale;
        }
        if voicing < 0.0 {
            voicing = 0.0;
        }
        if voicing > 1.0 {
            voicing = 1.0;
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
            let mut dry_sample_l: f64 = input_sample_l;
            let mut dry_sample_r: f64 = input_sample_r;
            let mut mu_makeup_gain_l: f64 = 1.0 / s.threshold_l;
            let mut out_makeup_gain_l: f64 = mu_makeup_gain_l.sqrt();
            mu_makeup_gain_l += out_makeup_gain_l;
            mu_makeup_gain_l *= 0.5;
            let mut release_l: f64 = s.merged_coefficients_l * 32768.0;
            release_l /= overallscale;
            let mut fastest_l: f64 = release_l.sqrt();
            let mut last_correction_l: f64 = s.merged_coefficients_l;
            let mut mu_makeup_gain_r: f64 = 1.0 / s.threshold_r;
            let mut out_makeup_gain_r: f64 = mu_makeup_gain_r.sqrt();
            mu_makeup_gain_r += out_makeup_gain_r;
            mu_makeup_gain_r *= 0.5;
            let mut release_r: f64 = s.merged_coefficients_r * 32768.0;
            release_r /= overallscale;
            let mut fastest_r: f64 = release_r.sqrt();
            let mut last_correction_r: f64 = s.merged_coefficients_r;
            if mu_makeup_gain_l != 1.0 {
                input_sample_l = input_sample_l * mu_makeup_gain_l;
            }
            if mu_makeup_gain_r != 1.0 {
                input_sample_r = input_sample_r * mu_makeup_gain_r;
            }
            if s.count < 1 || s.count > 3 {
                s.count = 1;
            }
            match s.count {
                1 => {
                    if input_sample_l.abs() > s.threshold_l {
                        if input_sample_l > 0.0 {
                            input_sample_l = input_sample_l * voicing + targetthreshold * (1.0 - voicing);
                            s.threshold_l = input_sample_l.abs();
                        } else {
                            input_sample_l = input_sample_l * voicing - targetthreshold * (1.0 - voicing);
                            s.threshold_l = input_sample_l.abs();
                        }
                        s.mu_vary_l = targetthreshold / input_sample_l.abs();
                        s.mu_attack_l = s.mu_speed_al.abs().sqrt();
                        s.mu_coefficient_al = s.mu_coefficient_al * (s.mu_attack_l - 1.0);
                        if s.mu_vary_l < s.threshold_l {
                            s.mu_coefficient_al = s.mu_coefficient_al + targetthreshold;
                        } else {
                            s.mu_coefficient_al = s.mu_coefficient_al + s.mu_vary_l;
                        }
                        s.mu_coefficient_al = s.mu_coefficient_al / s.mu_attack_l;
                    } else {
                        s.threshold_l = targetthreshold;
                        s.mu_coefficient_al = s.mu_coefficient_al * (s.mu_speed_al * s.mu_speed_al - 1.0);
                        s.mu_coefficient_al = s.mu_coefficient_al + 1.0;
                        s.mu_coefficient_al = s.mu_coefficient_al / (s.mu_speed_al * s.mu_speed_al);
                    }
                    s.mu_new_speed_l = s.mu_speed_al * (s.mu_speed_al - 1.0);
                    s.mu_new_speed_l = s.mu_new_speed_l + (input_sample_l * release_l).abs() + fastest_l;
                    s.mu_speed_al = s.mu_new_speed_l / s.mu_speed_al;
                    s.last_coefficient_al = s.mu_coefficient_al.powf(2.0);
                    s.merged_coefficients_l = s.last_coefficient_bl;
                    s.merged_coefficients_l += s.last_coefficient_al;
                    s.last_coefficient_al *= 1.0 - last_correction_l;
                    s.last_coefficient_al += s.mu_coefficient_al * last_correction_l;
                    s.last_coefficient_bl = s.last_coefficient_al;
                    if input_sample_r.abs() > s.threshold_r {
                        if input_sample_r > 0.0 {
                            input_sample_r = input_sample_r * voicing + targetthreshold * (1.0 - voicing);
                            s.threshold_r = input_sample_r.abs();
                        } else {
                            input_sample_r = input_sample_r * voicing - targetthreshold * (1.0 - voicing);
                            s.threshold_r = input_sample_r.abs();
                        }
                        s.mu_vary_r = targetthreshold / input_sample_r.abs();
                        s.mu_attack_r = s.mu_speed_ar.abs().sqrt();
                        s.mu_coefficient_ar = s.mu_coefficient_ar * (s.mu_attack_r - 1.0);
                        if s.mu_vary_r < s.threshold_r {
                            s.mu_coefficient_ar = s.mu_coefficient_ar + targetthreshold;
                        } else {
                            s.mu_coefficient_ar = s.mu_coefficient_ar + s.mu_vary_r;
                        }
                        s.mu_coefficient_ar = s.mu_coefficient_ar / s.mu_attack_r;
                    } else {
                        s.threshold_r = targetthreshold;
                        s.mu_coefficient_ar = s.mu_coefficient_ar * (s.mu_speed_ar * s.mu_speed_ar - 1.0);
                        s.mu_coefficient_ar = s.mu_coefficient_ar + 1.0;
                        s.mu_coefficient_ar = s.mu_coefficient_ar / (s.mu_speed_ar * s.mu_speed_ar);
                    }
                    s.mu_new_speed_r = s.mu_speed_ar * (s.mu_speed_ar - 1.0);
                    s.mu_new_speed_r = s.mu_new_speed_r + (input_sample_r * release_r).abs() + fastest_r;
                    s.mu_speed_ar = s.mu_new_speed_r / s.mu_speed_ar;
                    s.last_coefficient_ar = s.mu_coefficient_ar.powf(2.0);
                    s.merged_coefficients_r = s.last_coefficient_br;
                    s.merged_coefficients_r += s.last_coefficient_ar;
                    s.last_coefficient_ar *= 1.0 - last_correction_r;
                    s.last_coefficient_ar += s.mu_coefficient_ar * last_correction_r;
                    s.last_coefficient_br = s.last_coefficient_ar;
                }
                2 => {
                    if input_sample_l.abs() > s.threshold_l {
                        if input_sample_l > 0.0 {
                            input_sample_l = input_sample_l * voicing + targetthreshold * (1.0 - voicing);
                            s.threshold_l = input_sample_l.abs();
                        } else {
                            input_sample_l = input_sample_l * voicing - targetthreshold * (1.0 - voicing);
                            s.threshold_l = input_sample_l.abs();
                        }
                        s.mu_vary_l = targetthreshold / input_sample_l.abs();
                        s.mu_attack_l = s.mu_speed_bl.abs().sqrt();
                        s.mu_coefficient_bl = s.mu_coefficient_bl * (s.mu_attack_l - 1.0);
                        if s.mu_vary_l < s.threshold_l {
                            s.mu_coefficient_bl = s.mu_coefficient_bl + targetthreshold;
                        } else {
                            s.mu_coefficient_bl = s.mu_coefficient_bl + s.mu_vary_l;
                        }
                        s.mu_coefficient_bl = s.mu_coefficient_bl / s.mu_attack_l;
                    } else {
                        s.threshold_l = targetthreshold;
                        s.mu_coefficient_bl = s.mu_coefficient_bl * (s.mu_speed_bl * s.mu_speed_bl - 1.0);
                        s.mu_coefficient_bl = s.mu_coefficient_bl + 1.0;
                        s.mu_coefficient_bl = s.mu_coefficient_bl / (s.mu_speed_bl * s.mu_speed_bl);
                    }
                    s.mu_new_speed_l = s.mu_speed_bl * (s.mu_speed_bl - 1.0);
                    s.mu_new_speed_l = s.mu_new_speed_l + (input_sample_l * release_l).abs() + fastest_l;
                    s.mu_speed_bl = s.mu_new_speed_l / s.mu_speed_bl;
                    s.last_coefficient_al = s.mu_coefficient_bl.powf(2.0);
                    s.merged_coefficients_l = s.last_coefficient_bl;
                    s.merged_coefficients_l += s.last_coefficient_al;
                    s.last_coefficient_al *= 1.0 - last_correction_l;
                    s.last_coefficient_al += s.mu_coefficient_bl * last_correction_l;
                    s.last_coefficient_bl = s.last_coefficient_al;
                    if input_sample_r.abs() > s.threshold_r {
                        if input_sample_r > 0.0 {
                            input_sample_r = input_sample_r * voicing + targetthreshold * (1.0 - voicing);
                            s.threshold_r = input_sample_r.abs();
                        } else {
                            input_sample_r = input_sample_r * voicing - targetthreshold * (1.0 - voicing);
                            s.threshold_r = input_sample_r.abs();
                        }
                        s.mu_vary_r = targetthreshold / input_sample_r.abs();
                        s.mu_attack_r = s.mu_speed_br.abs().sqrt();
                        s.mu_coefficient_br = s.mu_coefficient_br * (s.mu_attack_r - 1.0);
                        if s.mu_vary_r < s.threshold_r {
                            s.mu_coefficient_br = s.mu_coefficient_br + targetthreshold;
                        } else {
                            s.mu_coefficient_br = s.mu_coefficient_br + s.mu_vary_r;
                        }
                        s.mu_coefficient_br = s.mu_coefficient_br / s.mu_attack_r;
                    } else {
                        s.threshold_r = targetthreshold;
                        s.mu_coefficient_br = s.mu_coefficient_br * (s.mu_speed_br * s.mu_speed_br - 1.0);
                        s.mu_coefficient_br = s.mu_coefficient_br + 1.0;
                        s.mu_coefficient_br = s.mu_coefficient_br / (s.mu_speed_br * s.mu_speed_br);
                    }
                    s.mu_new_speed_r = s.mu_speed_br * (s.mu_speed_br - 1.0);
                    s.mu_new_speed_r = s.mu_new_speed_r + (input_sample_r * release_r).abs() + fastest_r;
                    s.mu_speed_br = s.mu_new_speed_r / s.mu_speed_br;
                    s.last_coefficient_ar = s.mu_coefficient_br.powf(2.0);
                    s.merged_coefficients_r = s.last_coefficient_br;
                    s.merged_coefficients_r += s.last_coefficient_ar;
                    s.last_coefficient_ar *= 1.0 - last_correction_r;
                    s.last_coefficient_ar += s.mu_coefficient_br * last_correction_r;
                    s.last_coefficient_br = s.last_coefficient_ar;
                }
                3 => {
                    if input_sample_l.abs() > s.threshold_l {
                        if input_sample_l > 0.0 {
                            input_sample_l = input_sample_l * voicing + targetthreshold * (1.0 - voicing);
                            s.threshold_l = input_sample_l.abs();
                        } else {
                            input_sample_l = input_sample_l * voicing - targetthreshold * (1.0 - voicing);
                            s.threshold_l = input_sample_l.abs();
                        }
                        s.mu_vary_l = targetthreshold / input_sample_l.abs();
                        s.mu_attack_l = s.mu_speed_cl.abs().sqrt();
                        s.mu_coefficient_cl = s.mu_coefficient_cl * (s.mu_attack_l - 1.0);
                        if s.mu_vary_l < s.threshold_l {
                            s.mu_coefficient_cl = s.mu_coefficient_cl + targetthreshold;
                        } else {
                            s.mu_coefficient_cl = s.mu_coefficient_cl + s.mu_vary_l;
                        }
                        s.mu_coefficient_cl = s.mu_coefficient_cl / s.mu_attack_l;
                    } else {
                        s.threshold_l = targetthreshold;
                        s.mu_coefficient_cl = s.mu_coefficient_cl * (s.mu_speed_cl * s.mu_speed_cl - 1.0);
                        s.mu_coefficient_cl = s.mu_coefficient_cl + 1.0;
                        s.mu_coefficient_cl = s.mu_coefficient_cl / (s.mu_speed_cl * s.mu_speed_cl);
                    }
                    s.mu_new_speed_l = s.mu_speed_cl * (s.mu_speed_cl - 1.0);
                    s.mu_new_speed_l = s.mu_new_speed_l + (input_sample_l * release_l).abs() + fastest_l;
                    s.mu_speed_cl = s.mu_new_speed_l / s.mu_speed_cl;
                    s.last_coefficient_al = s.mu_coefficient_cl.powf(2.0);
                    s.merged_coefficients_l = s.last_coefficient_bl;
                    s.merged_coefficients_l += s.last_coefficient_al;
                    s.last_coefficient_al *= 1.0 - last_correction_l;
                    s.last_coefficient_al += s.mu_coefficient_cl * last_correction_l;
                    s.last_coefficient_bl = s.last_coefficient_al;
                    if input_sample_r.abs() > s.threshold_r {
                        if input_sample_r > 0.0 {
                            input_sample_r = input_sample_r * voicing + targetthreshold * (1.0 - voicing);
                            s.threshold_r = input_sample_r.abs();
                        } else {
                            input_sample_r = input_sample_r * voicing - targetthreshold * (1.0 - voicing);
                            s.threshold_r = input_sample_r.abs();
                        }
                        s.mu_vary_r = targetthreshold / input_sample_r.abs();
                        s.mu_attack_r = s.mu_speed_cr.abs().sqrt();
                        s.mu_coefficient_cr = s.mu_coefficient_cr * (s.mu_attack_r - 1.0);
                        if s.mu_vary_r < s.threshold_r {
                            s.mu_coefficient_cr = s.mu_coefficient_cr + targetthreshold;
                        } else {
                            s.mu_coefficient_cr = s.mu_coefficient_cr + s.mu_vary_r;
                        }
                        s.mu_coefficient_cr = s.mu_coefficient_cr / s.mu_attack_r;
                    } else {
                        s.threshold_r = targetthreshold;
                        s.mu_coefficient_cr = s.mu_coefficient_cr * (s.mu_speed_cr * s.mu_speed_cr - 1.0);
                        s.mu_coefficient_cr = s.mu_coefficient_cr + 1.0;
                        s.mu_coefficient_cr = s.mu_coefficient_cr / (s.mu_speed_cr * s.mu_speed_cr);
                    }
                    s.mu_new_speed_r = s.mu_speed_cr * (s.mu_speed_cr - 1.0);
                    s.mu_new_speed_r = s.mu_new_speed_r + (input_sample_r * release_r).abs() + fastest_r;
                    s.mu_speed_cr = s.mu_new_speed_r / s.mu_speed_cr;
                    s.last_coefficient_ar = s.mu_coefficient_cr.powf(2.0);
                    s.merged_coefficients_r = s.last_coefficient_br;
                    s.merged_coefficients_r += s.last_coefficient_ar;
                    s.last_coefficient_ar *= 1.0 - last_correction_r;
                    s.last_coefficient_ar += s.mu_coefficient_cr * last_correction_r;
                    s.last_coefficient_br = s.last_coefficient_ar;
                }
                _ => {}
            }
            s.count += 1;
            if s.fp_flip {
                if input_sample_l.abs() > s.threshold_bl {
                    if input_sample_l > 0.0 {
                        input_sample_l = input_sample_l * voicing + targetthreshold * (1.0 - voicing);
                        s.threshold_bl = input_sample_l.abs();
                    } else {
                        input_sample_l = input_sample_l * voicing - targetthreshold * (1.0 - voicing);
                        s.threshold_bl = input_sample_l.abs();
                    }
                    s.mu_vary_l = targetthreshold / input_sample_l.abs();
                    s.mu_attack_l = s.mu_speed_dl.abs().sqrt();
                    s.mu_coefficient_dl = s.mu_coefficient_dl * (s.mu_attack_l - 1.0);
                    if s.mu_vary_l < s.threshold_bl {
                        s.mu_coefficient_dl = s.mu_coefficient_dl + targetthreshold;
                    } else {
                        s.mu_coefficient_dl = s.mu_coefficient_dl + s.mu_vary_l;
                    }
                    s.mu_coefficient_dl = s.mu_coefficient_dl / s.mu_attack_l;
                } else {
                    s.threshold_bl = targetthreshold;
                    s.mu_coefficient_dl = s.mu_coefficient_dl * (s.mu_speed_dl * s.mu_speed_dl - 1.0);
                    s.mu_coefficient_dl = s.mu_coefficient_dl + 1.0;
                    s.mu_coefficient_dl = s.mu_coefficient_dl / (s.mu_speed_dl * s.mu_speed_dl);
                }
                s.mu_new_speed_l = s.mu_speed_dl * (s.mu_speed_dl - 1.0);
                s.mu_new_speed_l = s.mu_new_speed_l + (input_sample_l * release_l).abs() + fastest_l;
                s.mu_speed_dl = s.mu_new_speed_l / s.mu_speed_dl;
                s.last_coefficient_cl = s.mu_coefficient_el.powf(2.0);
                s.merged_coefficients_l += s.last_coefficient_dl;
                s.merged_coefficients_l += s.last_coefficient_cl;
                s.last_coefficient_cl *= 1.0 - last_correction_l;
                s.last_coefficient_cl += s.mu_coefficient_dl * last_correction_l;
                s.last_coefficient_dl = s.last_coefficient_cl;
                if input_sample_r.abs() > s.threshold_br {
                    if input_sample_r > 0.0 {
                        input_sample_r = input_sample_r * voicing + targetthreshold * (1.0 - voicing);
                        s.threshold_br = input_sample_r.abs();
                    } else {
                        input_sample_r = input_sample_r * voicing - targetthreshold * (1.0 - voicing);
                        s.threshold_br = input_sample_r.abs();
                    }
                    s.mu_vary_r = targetthreshold / input_sample_r.abs();
                    s.mu_attack_r = s.mu_speed_dr.abs().sqrt();
                    s.mu_coefficient_dr = s.mu_coefficient_dr * (s.mu_attack_r - 1.0);
                    if s.mu_vary_r < s.threshold_br {
                        s.mu_coefficient_dr = s.mu_coefficient_dr + targetthreshold;
                    } else {
                        s.mu_coefficient_dr = s.mu_coefficient_dr + s.mu_vary_r;
                    }
                    s.mu_coefficient_dr = s.mu_coefficient_dr / s.mu_attack_r;
                } else {
                    s.threshold_br = targetthreshold;
                    s.mu_coefficient_dr = s.mu_coefficient_dr * (s.mu_speed_dr * s.mu_speed_dr - 1.0);
                    s.mu_coefficient_dr = s.mu_coefficient_dr + 1.0;
                    s.mu_coefficient_dr = s.mu_coefficient_dr / (s.mu_speed_dr * s.mu_speed_dr);
                }
                s.mu_new_speed_r = s.mu_speed_dr * (s.mu_speed_dr - 1.0);
                s.mu_new_speed_r = s.mu_new_speed_r + (input_sample_r * release_r).abs() + fastest_r;
                s.mu_speed_dr = s.mu_new_speed_r / s.mu_speed_dr;
                s.last_coefficient_cr = s.mu_coefficient_er.powf(2.0);
                s.merged_coefficients_r += s.last_coefficient_dr;
                s.merged_coefficients_r += s.last_coefficient_cr;
                s.last_coefficient_cr *= 1.0 - last_correction_r;
                s.last_coefficient_cr += s.mu_coefficient_dr * last_correction_r;
                s.last_coefficient_dr = s.last_coefficient_cr;
            } else {
                if input_sample_l.abs() > s.threshold_bl {
                    if input_sample_l > 0.0 {
                        input_sample_l = input_sample_l * voicing + targetthreshold * (1.0 - voicing);
                        s.threshold_bl = input_sample_l.abs();
                    } else {
                        input_sample_l = input_sample_l * voicing - targetthreshold * (1.0 - voicing);
                        s.threshold_bl = input_sample_l.abs();
                    }
                    s.mu_vary_l = targetthreshold / input_sample_l.abs();
                    s.mu_attack_l = s.mu_speed_el.abs().sqrt();
                    s.mu_coefficient_el = s.mu_coefficient_el * (s.mu_attack_l - 1.0);
                    if s.mu_vary_l < s.threshold_bl {
                        s.mu_coefficient_el = s.mu_coefficient_el + targetthreshold;
                    } else {
                        s.mu_coefficient_el = s.mu_coefficient_el + s.mu_vary_l;
                    }
                    s.mu_coefficient_el = s.mu_coefficient_el / s.mu_attack_l;
                } else {
                    s.threshold_bl = targetthreshold;
                    s.mu_coefficient_el = s.mu_coefficient_el * (s.mu_speed_el * s.mu_speed_el - 1.0);
                    s.mu_coefficient_el = s.mu_coefficient_el + 1.0;
                    s.mu_coefficient_el = s.mu_coefficient_el / (s.mu_speed_el * s.mu_speed_el);
                }
                s.mu_new_speed_l = s.mu_speed_el * (s.mu_speed_el - 1.0);
                s.mu_new_speed_l = s.mu_new_speed_l + (input_sample_l * release_l).abs() + fastest_l;
                s.mu_speed_el = s.mu_new_speed_l / s.mu_speed_el;
                s.last_coefficient_cl = s.mu_coefficient_el.powf(2.0);
                s.merged_coefficients_l += s.last_coefficient_dl;
                s.merged_coefficients_l += s.last_coefficient_cl;
                s.last_coefficient_cl *= 1.0 - last_correction_l;
                s.last_coefficient_cl += s.mu_coefficient_el * last_correction_l;
                s.last_coefficient_dl = s.last_coefficient_cl;
                if input_sample_r.abs() > s.threshold_br {
                    if input_sample_r > 0.0 {
                        input_sample_r = input_sample_r * voicing + targetthreshold * (1.0 - voicing);
                        s.threshold_br = input_sample_r.abs();
                    } else {
                        input_sample_r = input_sample_r * voicing - targetthreshold * (1.0 - voicing);
                        s.threshold_br = input_sample_r.abs();
                    }
                    s.mu_vary_r = targetthreshold / input_sample_r.abs();
                    s.mu_attack_r = s.mu_speed_er.abs().sqrt();
                    s.mu_coefficient_er = s.mu_coefficient_er * (s.mu_attack_r - 1.0);
                    if s.mu_vary_r < s.threshold_br {
                        s.mu_coefficient_er = s.mu_coefficient_er + targetthreshold;
                    } else {
                        s.mu_coefficient_er = s.mu_coefficient_er + s.mu_vary_r;
                    }
                    s.mu_coefficient_er = s.mu_coefficient_er / s.mu_attack_r;
                } else {
                    s.threshold_br = targetthreshold;
                    s.mu_coefficient_er = s.mu_coefficient_er * (s.mu_speed_er * s.mu_speed_er - 1.0);
                    s.mu_coefficient_er = s.mu_coefficient_er + 1.0;
                    s.mu_coefficient_er = s.mu_coefficient_er / (s.mu_speed_er * s.mu_speed_er);
                }
                s.mu_new_speed_r = s.mu_speed_er * (s.mu_speed_er - 1.0);
                s.mu_new_speed_r = s.mu_new_speed_r + (input_sample_r * release_r).abs() + fastest_r;
                s.mu_speed_er = s.mu_new_speed_r / s.mu_speed_er;
                s.last_coefficient_cr = s.mu_coefficient_er.powf(2.0);
                s.merged_coefficients_r += s.last_coefficient_dr;
                s.merged_coefficients_r += s.last_coefficient_cr;
                s.last_coefficient_cr *= 1.0 - last_correction_r;
                s.last_coefficient_cr += s.mu_coefficient_er * last_correction_r;
                s.last_coefficient_dr = s.last_coefficient_cr;
            }
            s.merged_coefficients_l *= 0.25;
            input_sample_l *= s.merged_coefficients_l;
            s.merged_coefficients_r *= 0.25;
            input_sample_r *= s.merged_coefficients_r;
            if out_makeup_gain_l != 1.0 {
                input_sample_l = input_sample_l * out_makeup_gain_l;
            }
            if out_makeup_gain_r != 1.0 {
                input_sample_r = input_sample_r * out_makeup_gain_r;
            }
            s.fp_flip = !s.fp_flip;
            if wet < 1.0 {
                input_sample_l = input_sample_l * wet + dry_sample_l * (1.0 - wet);
                input_sample_r = input_sample_r * wet + dry_sample_r * (1.0 - wet);
            }
            if input_sample_l > 0.999 {
                input_sample_l = 0.999;
            }
            if input_sample_l < -0.999 {
                input_sample_l = -0.999;
            }
            if input_sample_r > 0.999 {
                input_sample_r = 0.999;
            }
            if input_sample_r < -0.999 {
                input_sample_r = -0.999;
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
