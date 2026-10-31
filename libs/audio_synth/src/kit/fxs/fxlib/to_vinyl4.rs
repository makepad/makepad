// Ported from ToVinyl4 (ToVinyl4Proc.cpp, ToVinyl4.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! To Vinyl (sub-effect id 68): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Mid HiP", 0.203419),
    pf("SideHiP", 0.3424051),
    pf("H Limit", 0.32),
    pf("Gv Wear", 0.064),
];

#[allow(dead_code)]
pub struct ToVinyl4 {
    p: [f32; 4],
    sr: f32,
    ata_last_out_l: f64,
    ata_last_out_r: f64,
    s1_l: f64,
    s2_l: f64,
    s3_l: f64,
    o1_l: f64,
    o2_l: f64,
    o3_l: f64,
    m1_l: f64,
    m2_l: f64,
    s1_r: f64,
    s2_r: f64,
    s3_r: f64,
    o1_r: f64,
    o2_r: f64,
    o3_r: f64,
    m1_r: f64,
    m2_r: f64,
    des_l: f64,
    des_r: f64,
    mid_sample_a: f64,
    mid_sample_b: f64,
    mid_sample_c: f64,
    mid_sample_d: f64,
    mid_sample_e: f64,
    mid_sample_f: f64,
    mid_sample_g: f64,
    mid_sample_h: f64,
    mid_sample_i: f64,
    mid_sample_j: f64,
    mid_sample_k: f64,
    mid_sample_l: f64,
    mid_sample_m: f64,
    mid_sample_n: f64,
    mid_sample_o: f64,
    mid_sample_p: f64,
    mid_sample_q: f64,
    mid_sample_r: f64,
    mid_sample_s: f64,
    mid_sample_t: f64,
    mid_sample_u: f64,
    mid_sample_v: f64,
    mid_sample_w: f64,
    mid_sample_x: f64,
    mid_sample_y: f64,
    mid_sample_z: f64,
    side_sample_a: f64,
    side_sample_b: f64,
    side_sample_c: f64,
    side_sample_d: f64,
    side_sample_e: f64,
    side_sample_f: f64,
    side_sample_g: f64,
    side_sample_h: f64,
    side_sample_i: f64,
    side_sample_j: f64,
    side_sample_k: f64,
    side_sample_l: f64,
    side_sample_m: f64,
    side_sample_n: f64,
    side_sample_o: f64,
    side_sample_p: f64,
    side_sample_q: f64,
    side_sample_r: f64,
    side_sample_s: f64,
    side_sample_t: f64,
    side_sample_u: f64,
    side_sample_v: f64,
    side_sample_w: f64,
    side_sample_x: f64,
    side_sample_y: f64,
    side_sample_z: f64,
    a_mid: [f64; 11],
    b_mid: [f64; 11],
    f_mid: [f64; 11],
    a_side: [f64; 11],
    b_side: [f64; 11],
    f_side: [f64; 11],
    a_mid_prev: f64,
    a_side_prev: f64,
    b_mid_prev: f64,
    b_side_prev: f64,
    fpd_l: u32,
    fpd_r: u32,
}

impl ToVinyl4 {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<ToVinyl4> {
        let mut s = Box::new(ToVinyl4 {
            p: [0.203419, 0.3424051, 0.32, 0.064],
            sr: sr as f32,
            ata_last_out_l: 0.0,
            ata_last_out_r: 0.0,
            s1_l: 0.0,
            s2_l: 0.0,
            s3_l: 0.0,
            o1_l: 0.0,
            o2_l: 0.0,
            o3_l: 0.0,
            m1_l: 0.0,
            m2_l: 0.0,
            s1_r: 0.0,
            s2_r: 0.0,
            s3_r: 0.0,
            o1_r: 0.0,
            o2_r: 0.0,
            o3_r: 0.0,
            m1_r: 0.0,
            m2_r: 0.0,
            des_l: 0.0,
            des_r: 0.0,
            mid_sample_a: 0.0,
            mid_sample_b: 0.0,
            mid_sample_c: 0.0,
            mid_sample_d: 0.0,
            mid_sample_e: 0.0,
            mid_sample_f: 0.0,
            mid_sample_g: 0.0,
            mid_sample_h: 0.0,
            mid_sample_i: 0.0,
            mid_sample_j: 0.0,
            mid_sample_k: 0.0,
            mid_sample_l: 0.0,
            mid_sample_m: 0.0,
            mid_sample_n: 0.0,
            mid_sample_o: 0.0,
            mid_sample_p: 0.0,
            mid_sample_q: 0.0,
            mid_sample_r: 0.0,
            mid_sample_s: 0.0,
            mid_sample_t: 0.0,
            mid_sample_u: 0.0,
            mid_sample_v: 0.0,
            mid_sample_w: 0.0,
            mid_sample_x: 0.0,
            mid_sample_y: 0.0,
            mid_sample_z: 0.0,
            side_sample_a: 0.0,
            side_sample_b: 0.0,
            side_sample_c: 0.0,
            side_sample_d: 0.0,
            side_sample_e: 0.0,
            side_sample_f: 0.0,
            side_sample_g: 0.0,
            side_sample_h: 0.0,
            side_sample_i: 0.0,
            side_sample_j: 0.0,
            side_sample_k: 0.0,
            side_sample_l: 0.0,
            side_sample_m: 0.0,
            side_sample_n: 0.0,
            side_sample_o: 0.0,
            side_sample_p: 0.0,
            side_sample_q: 0.0,
            side_sample_r: 0.0,
            side_sample_s: 0.0,
            side_sample_t: 0.0,
            side_sample_u: 0.0,
            side_sample_v: 0.0,
            side_sample_w: 0.0,
            side_sample_x: 0.0,
            side_sample_y: 0.0,
            side_sample_z: 0.0,
            a_mid: [0.0; 11],
            b_mid: [0.0; 11],
            f_mid: [0.0; 11],
            a_side: [0.0; 11],
            b_side: [0.0; 11],
            f_side: [0.0; 11],
            a_mid_prev: 0.0,
            a_side_prev: 0.0,
            b_mid_prev: 0.0,
            b_side_prev: 0.0,
            fpd_l: 0,
            fpd_r: 0,
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for ToVinyl4 {
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
        let mut fusswithscale: f64 = 50000.0;
        let mut cutofffreq: f64 = (s.p[0] * s.p[0]) as f64 * 290.0 + 10.0;
        let mut resonance: f64 = 0.992;
        let mut mid_amount: f64 = cutofffreq / fusswithscale;
        mid_amount /= overallscale;
        let mut mid_amount_a: f64 = mid_amount * resonance;
        let mut mid_amount_b: f64 = mid_amount_a * resonance;
        let mut mid_amount_c: f64 = mid_amount_b * resonance;
        let mut mid_amount_d: f64 = mid_amount_c * resonance;
        let mut mid_amount_e: f64 = mid_amount_d * resonance;
        let mut mid_amount_f: f64 = mid_amount_e * resonance;
        let mut mid_amount_g: f64 = mid_amount_f * resonance;
        let mut mid_amount_h: f64 = mid_amount_g * resonance;
        let mut mid_amount_i: f64 = mid_amount_h * resonance;
        let mut mid_amount_j: f64 = mid_amount_i * resonance;
        let mut mid_amount_k: f64 = mid_amount_j * resonance;
        let mut mid_amount_l: f64 = mid_amount_k * resonance;
        let mut mid_amount_m: f64 = mid_amount_l * resonance;
        let mut mid_amount_n: f64 = mid_amount_m * resonance;
        let mut mid_amount_o: f64 = mid_amount_n * resonance;
        let mut mid_amount_p: f64 = mid_amount_o * resonance;
        let mut mid_amount_q: f64 = mid_amount_p * resonance;
        let mut mid_amount_r: f64 = mid_amount_q * resonance;
        let mut mid_amount_s: f64 = mid_amount_r * resonance;
        let mut mid_amount_t: f64 = mid_amount_s * resonance;
        let mut mid_amount_u: f64 = mid_amount_t * resonance;
        let mut mid_amount_v: f64 = mid_amount_u * resonance;
        let mut mid_amount_w: f64 = mid_amount_v * resonance;
        let mut mid_amount_x: f64 = mid_amount_w * resonance;
        let mut mid_amount_y: f64 = mid_amount_x * resonance;
        let mut mid_amount_z: f64 = mid_amount_y * resonance;
        let mut midalt_amount_a: f64 = 1.0 - mid_amount_a;
        let mut midalt_amount_b: f64 = 1.0 - mid_amount_b;
        let mut midalt_amount_c: f64 = 1.0 - mid_amount_c;
        let mut midalt_amount_d: f64 = 1.0 - mid_amount_d;
        let mut midalt_amount_e: f64 = 1.0 - mid_amount_e;
        let mut midalt_amount_f: f64 = 1.0 - mid_amount_f;
        let mut midalt_amount_g: f64 = 1.0 - mid_amount_g;
        let mut midalt_amount_h: f64 = 1.0 - mid_amount_h;
        let mut midalt_amount_i: f64 = 1.0 - mid_amount_i;
        let mut midalt_amount_j: f64 = 1.0 - mid_amount_j;
        let mut midalt_amount_k: f64 = 1.0 - mid_amount_k;
        let mut midalt_amount_l: f64 = 1.0 - mid_amount_l;
        let mut midalt_amount_m: f64 = 1.0 - mid_amount_m;
        let mut midalt_amount_n: f64 = 1.0 - mid_amount_n;
        let mut midalt_amount_o: f64 = 1.0 - mid_amount_o;
        let mut midalt_amount_p: f64 = 1.0 - mid_amount_p;
        let mut midalt_amount_q: f64 = 1.0 - mid_amount_q;
        let mut midalt_amount_r: f64 = 1.0 - mid_amount_r;
        let mut midalt_amount_s: f64 = 1.0 - mid_amount_s;
        let mut midalt_amount_t: f64 = 1.0 - mid_amount_t;
        let mut midalt_amount_u: f64 = 1.0 - mid_amount_u;
        let mut midalt_amount_v: f64 = 1.0 - mid_amount_v;
        let mut midalt_amount_w: f64 = 1.0 - mid_amount_w;
        let mut midalt_amount_x: f64 = 1.0 - mid_amount_x;
        let mut midalt_amount_y: f64 = 1.0 - mid_amount_y;
        let mut midalt_amount_z: f64 = 1.0 - mid_amount_z;
        cutofffreq = (s.p[1] * s.p[1]) as f64 * 290.0 + 10.0;
        let mut side_amount: f64 = cutofffreq / fusswithscale;
        side_amount /= overallscale;
        let mut side_amount_a: f64 = side_amount * resonance;
        let mut side_amount_b: f64 = side_amount_a * resonance;
        let mut side_amount_c: f64 = side_amount_b * resonance;
        let mut side_amount_d: f64 = side_amount_c * resonance;
        let mut side_amount_e: f64 = side_amount_d * resonance;
        let mut side_amount_f: f64 = side_amount_e * resonance;
        let mut side_amount_g: f64 = side_amount_f * resonance;
        let mut side_amount_h: f64 = side_amount_g * resonance;
        let mut side_amount_i: f64 = side_amount_h * resonance;
        let mut side_amount_j: f64 = side_amount_i * resonance;
        let mut side_amount_k: f64 = side_amount_j * resonance;
        let mut side_amount_l: f64 = side_amount_k * resonance;
        let mut side_amount_m: f64 = side_amount_l * resonance;
        let mut side_amount_n: f64 = side_amount_m * resonance;
        let mut side_amount_o: f64 = side_amount_n * resonance;
        let mut side_amount_p: f64 = side_amount_o * resonance;
        let mut side_amount_q: f64 = side_amount_p * resonance;
        let mut side_amount_r: f64 = side_amount_q * resonance;
        let mut side_amount_s: f64 = side_amount_r * resonance;
        let mut side_amount_t: f64 = side_amount_s * resonance;
        let mut side_amount_u: f64 = side_amount_t * resonance;
        let mut side_amount_v: f64 = side_amount_u * resonance;
        let mut side_amount_w: f64 = side_amount_v * resonance;
        let mut side_amount_x: f64 = side_amount_w * resonance;
        let mut side_amount_y: f64 = side_amount_x * resonance;
        let mut side_amount_z: f64 = side_amount_y * resonance;
        let mut sidealt_amount_a: f64 = 1.0 - side_amount_a;
        let mut sidealt_amount_b: f64 = 1.0 - side_amount_b;
        let mut sidealt_amount_c: f64 = 1.0 - side_amount_c;
        let mut sidealt_amount_d: f64 = 1.0 - side_amount_d;
        let mut sidealt_amount_e: f64 = 1.0 - side_amount_e;
        let mut sidealt_amount_f: f64 = 1.0 - side_amount_f;
        let mut sidealt_amount_g: f64 = 1.0 - side_amount_g;
        let mut sidealt_amount_h: f64 = 1.0 - side_amount_h;
        let mut sidealt_amount_i: f64 = 1.0 - side_amount_i;
        let mut sidealt_amount_j: f64 = 1.0 - side_amount_j;
        let mut sidealt_amount_k: f64 = 1.0 - side_amount_k;
        let mut sidealt_amount_l: f64 = 1.0 - side_amount_l;
        let mut sidealt_amount_m: f64 = 1.0 - side_amount_m;
        let mut sidealt_amount_n: f64 = 1.0 - side_amount_n;
        let mut sidealt_amount_o: f64 = 1.0 - side_amount_o;
        let mut sidealt_amount_p: f64 = 1.0 - side_amount_p;
        let mut sidealt_amount_q: f64 = 1.0 - side_amount_q;
        let mut sidealt_amount_r: f64 = 1.0 - side_amount_r;
        let mut sidealt_amount_s: f64 = 1.0 - side_amount_s;
        let mut sidealt_amount_t: f64 = 1.0 - side_amount_t;
        let mut sidealt_amount_u: f64 = 1.0 - side_amount_u;
        let mut sidealt_amount_v: f64 = 1.0 - side_amount_v;
        let mut sidealt_amount_w: f64 = 1.0 - side_amount_w;
        let mut sidealt_amount_x: f64 = 1.0 - side_amount_x;
        let mut sidealt_amount_y: f64 = 1.0 - side_amount_y;
        let mut sidealt_amount_z: f64 = 1.0 - side_amount_z;
        let mut temp_mid: f64 = 0.0;
        let mut temp_side: f64 = 0.0;
        let mut intensity: f64 = (s.p[2] as f64).powf(3.0) * (32.0 / overallscale);
        let mut input_sample_l: f64 = 0.0;
        let mut input_sample_r: f64 = 0.0;
        let mut sense_l: f64 = 0.0;
        let mut sense_r: f64 = 0.0;
        let mut smooth_l: f64 = 0.0;
        let mut smooth_r: f64 = 0.0;
        let mut mid: f64 = 0.0;
        let mut side: f64 = 0.0;
        overallscale = s.p[3] as f64 * 9.0 + 1.0;
        let mut gain: f64 = overallscale;
        if gain > 1.0 {
            s.f_mid[0] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[0] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[1] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[1] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[2] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[2] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[3] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[3] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[4] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[4] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[5] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[5] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[6] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[6] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[7] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[7] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[8] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[8] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_mid[9] = 1.0;
            gain -= 1.0;
        } else {
            s.f_mid[9] = gain;
            gain = 0.0;
        }
        if overallscale < 1.0 {
            overallscale = 1.0;
        }
        s.f_mid[0] /= overallscale;
        s.f_mid[1] /= overallscale;
        s.f_mid[2] /= overallscale;
        s.f_mid[3] /= overallscale;
        s.f_mid[4] /= overallscale;
        s.f_mid[5] /= overallscale;
        s.f_mid[6] /= overallscale;
        s.f_mid[7] /= overallscale;
        s.f_mid[8] /= overallscale;
        s.f_mid[9] /= overallscale;
        overallscale = s.p[3] as f64 * 4.5 + 1.0;
        gain = overallscale;
        if gain > 1.0 {
            s.f_side[0] = 1.0;
            gain -= 1.0;
        } else {
            s.f_side[0] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_side[1] = 1.0;
            gain -= 1.0;
        } else {
            s.f_side[1] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_side[2] = 1.0;
            gain -= 1.0;
        } else {
            s.f_side[2] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_side[3] = 1.0;
            gain -= 1.0;
        } else {
            s.f_side[3] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_side[4] = 1.0;
            gain -= 1.0;
        } else {
            s.f_side[4] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_side[5] = 1.0;
            gain -= 1.0;
        } else {
            s.f_side[5] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_side[6] = 1.0;
            gain -= 1.0;
        } else {
            s.f_side[6] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_side[7] = 1.0;
            gain -= 1.0;
        } else {
            s.f_side[7] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_side[8] = 1.0;
            gain -= 1.0;
        } else {
            s.f_side[8] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f_side[9] = 1.0;
            gain -= 1.0;
        } else {
            s.f_side[9] = gain;
            gain = 0.0;
        }
        if overallscale < 1.0 {
            overallscale = 1.0;
        }
        s.f_side[0] /= overallscale;
        s.f_side[1] /= overallscale;
        s.f_side[2] /= overallscale;
        s.f_side[3] /= overallscale;
        s.f_side[4] /= overallscale;
        s.f_side[5] /= overallscale;
        s.f_side[6] /= overallscale;
        s.f_side[7] /= overallscale;
        s.f_side[8] /= overallscale;
        s.f_side[9] /= overallscale;
        let mut temp_sample: f64 = 0.0;
        let mut accumulator_sample: f64 = 0.0;
        let mut mid_correction: f64 = 0.0;
        let mut side_correction: f64 = 0.0;
        let mut correction: f64 = 0.0;
        for smp in 0..l.len() {
            input_sample_l = l[smp] as f64;
            input_sample_r = r[smp] as f64;
            if input_sample_l.abs() < 1.18e-23 {
                input_sample_l = s.fpd_l as f64 * 1.18e-17;
            }
            if input_sample_r.abs() < 1.18e-23 {
                input_sample_r = s.fpd_r as f64 * 1.18e-17;
            }
            s.s3_l = s.s2_l;
            s.s2_l = s.s1_l;
            s.s1_l = input_sample_l;
            smooth_l = (s.s3_l + s.s2_l + s.s1_l) / 3.0;
            s.m1_l = (s.s1_l - s.s2_l) * ((s.s1_l - s.s2_l) / 1.3);
            s.m2_l = (s.s2_l - s.s3_l) * ((s.s1_l - s.s2_l) / 1.3);
            sense_l = (s.m1_l - s.m2_l).abs();
            sense_l = intensity * intensity * sense_l;
            s.o3_l = s.o2_l;
            s.o2_l = s.o1_l;
            s.o1_l = sense_l;
            if s.o2_l > sense_l {
                sense_l = s.o2_l;
            }
            if s.o3_l > sense_l {
                sense_l = s.o3_l;
            }
            s.s3_r = s.s2_r;
            s.s2_r = s.s1_r;
            s.s1_r = input_sample_r;
            smooth_r = (s.s3_r + s.s2_r + s.s1_r) / 3.0;
            s.m1_r = (s.s1_r - s.s2_r) * ((s.s1_r - s.s2_r) / 1.3);
            s.m2_r = (s.s2_r - s.s3_r) * ((s.s1_r - s.s2_r) / 1.3);
            sense_r = (s.m1_r - s.m2_r).abs();
            sense_r = intensity * intensity * sense_r;
            s.o3_r = s.o2_r;
            s.o2_r = s.o1_r;
            s.o1_r = sense_r;
            if s.o2_r > sense_r {
                sense_r = s.o2_r;
            }
            if s.o3_r > sense_r {
                sense_r = s.o3_r;
            }
            if sense_l > 1.0 {
                sense_l = 1.0;
            }
            if sense_r > 1.0 {
                sense_r = 1.0;
            }
            input_sample_l *= 1.0 - sense_l;
            input_sample_r *= 1.0 - sense_r;
            input_sample_l += smooth_l * sense_l;
            input_sample_r += smooth_r * sense_r;
            mid = input_sample_l + input_sample_r;
            temp_mid = mid;
            side = input_sample_l - input_sample_r;
            temp_side = side;
            temp_sample = mid;
            s.mid_sample_a = s.mid_sample_a * midalt_amount_a + temp_sample * mid_amount_a;
            temp_sample -= s.mid_sample_a;
            s.mid_sample_b = s.mid_sample_b * midalt_amount_b + temp_sample * mid_amount_b;
            temp_sample -= s.mid_sample_b;
            s.mid_sample_c = s.mid_sample_c * midalt_amount_c + temp_sample * mid_amount_c;
            temp_sample -= s.mid_sample_c;
            s.mid_sample_d = s.mid_sample_d * midalt_amount_d + temp_sample * mid_amount_d;
            temp_sample -= s.mid_sample_d;
            s.mid_sample_e = s.mid_sample_e * midalt_amount_e + temp_sample * mid_amount_e;
            temp_sample -= s.mid_sample_e;
            s.mid_sample_f = s.mid_sample_f * midalt_amount_f + temp_sample * mid_amount_f;
            temp_sample -= s.mid_sample_f;
            s.mid_sample_g = s.mid_sample_g * midalt_amount_g + temp_sample * mid_amount_g;
            temp_sample -= s.mid_sample_g;
            s.mid_sample_h = s.mid_sample_h * midalt_amount_h + temp_sample * mid_amount_h;
            temp_sample -= s.mid_sample_h;
            s.mid_sample_i = s.mid_sample_i * midalt_amount_i + temp_sample * mid_amount_i;
            temp_sample -= s.mid_sample_i;
            s.mid_sample_j = s.mid_sample_j * midalt_amount_j + temp_sample * mid_amount_j;
            temp_sample -= s.mid_sample_j;
            s.mid_sample_k = s.mid_sample_k * midalt_amount_k + temp_sample * mid_amount_k;
            temp_sample -= s.mid_sample_k;
            s.mid_sample_l = s.mid_sample_l * midalt_amount_l + temp_sample * mid_amount_l;
            temp_sample -= s.mid_sample_l;
            s.mid_sample_m = s.mid_sample_m * midalt_amount_m + temp_sample * mid_amount_m;
            temp_sample -= s.mid_sample_m;
            s.mid_sample_n = s.mid_sample_n * midalt_amount_n + temp_sample * mid_amount_n;
            temp_sample -= s.mid_sample_n;
            s.mid_sample_o = s.mid_sample_o * midalt_amount_o + temp_sample * mid_amount_o;
            temp_sample -= s.mid_sample_o;
            s.mid_sample_p = s.mid_sample_p * midalt_amount_p + temp_sample * mid_amount_p;
            temp_sample -= s.mid_sample_p;
            s.mid_sample_q = s.mid_sample_q * midalt_amount_q + temp_sample * mid_amount_q;
            temp_sample -= s.mid_sample_q;
            s.mid_sample_r = s.mid_sample_r * midalt_amount_r + temp_sample * mid_amount_r;
            temp_sample -= s.mid_sample_r;
            s.mid_sample_s = s.mid_sample_s * midalt_amount_s + temp_sample * mid_amount_s;
            temp_sample -= s.mid_sample_s;
            s.mid_sample_t = s.mid_sample_t * midalt_amount_t + temp_sample * mid_amount_t;
            temp_sample -= s.mid_sample_t;
            s.mid_sample_u = s.mid_sample_u * midalt_amount_u + temp_sample * mid_amount_u;
            temp_sample -= s.mid_sample_u;
            s.mid_sample_v = s.mid_sample_v * midalt_amount_v + temp_sample * mid_amount_v;
            temp_sample -= s.mid_sample_v;
            s.mid_sample_w = s.mid_sample_w * midalt_amount_w + temp_sample * mid_amount_w;
            temp_sample -= s.mid_sample_w;
            s.mid_sample_x = s.mid_sample_x * midalt_amount_x + temp_sample * mid_amount_x;
            temp_sample -= s.mid_sample_x;
            s.mid_sample_y = s.mid_sample_y * midalt_amount_y + temp_sample * mid_amount_y;
            temp_sample -= s.mid_sample_y;
            s.mid_sample_z = s.mid_sample_z * midalt_amount_z + temp_sample * mid_amount_z;
            temp_sample -= s.mid_sample_z;
            mid_correction = mid - temp_sample;
            correction = mid_correction;
            mid -= correction;
            temp_sample = side;
            s.side_sample_a = s.side_sample_a * sidealt_amount_a + temp_sample * side_amount_a;
            temp_sample -= s.side_sample_a;
            s.side_sample_b = s.side_sample_b * sidealt_amount_b + temp_sample * side_amount_b;
            temp_sample -= s.side_sample_b;
            s.side_sample_c = s.side_sample_c * sidealt_amount_c + temp_sample * side_amount_c;
            temp_sample -= s.side_sample_c;
            s.side_sample_d = s.side_sample_d * sidealt_amount_d + temp_sample * side_amount_d;
            temp_sample -= s.side_sample_d;
            s.side_sample_e = s.side_sample_e * sidealt_amount_e + temp_sample * side_amount_e;
            temp_sample -= s.side_sample_e;
            s.side_sample_f = s.side_sample_f * sidealt_amount_f + temp_sample * side_amount_f;
            temp_sample -= s.side_sample_f;
            s.side_sample_g = s.side_sample_g * sidealt_amount_g + temp_sample * side_amount_g;
            temp_sample -= s.side_sample_g;
            s.side_sample_h = s.side_sample_h * sidealt_amount_h + temp_sample * side_amount_h;
            temp_sample -= s.side_sample_h;
            s.side_sample_i = s.side_sample_i * sidealt_amount_i + temp_sample * side_amount_i;
            temp_sample -= s.side_sample_i;
            s.side_sample_j = s.side_sample_j * sidealt_amount_j + temp_sample * side_amount_j;
            temp_sample -= s.side_sample_j;
            s.side_sample_k = s.side_sample_k * sidealt_amount_k + temp_sample * side_amount_k;
            temp_sample -= s.side_sample_k;
            s.side_sample_l = s.side_sample_l * sidealt_amount_l + temp_sample * side_amount_l;
            temp_sample -= s.side_sample_l;
            s.side_sample_m = s.side_sample_m * sidealt_amount_m + temp_sample * side_amount_m;
            temp_sample -= s.side_sample_m;
            s.side_sample_n = s.side_sample_n * sidealt_amount_n + temp_sample * side_amount_n;
            temp_sample -= s.side_sample_n;
            s.side_sample_o = s.side_sample_o * sidealt_amount_o + temp_sample * side_amount_o;
            temp_sample -= s.side_sample_o;
            s.side_sample_p = s.side_sample_p * sidealt_amount_p + temp_sample * side_amount_p;
            temp_sample -= s.side_sample_p;
            s.side_sample_q = s.side_sample_q * sidealt_amount_q + temp_sample * side_amount_q;
            temp_sample -= s.side_sample_q;
            s.side_sample_r = s.side_sample_r * sidealt_amount_r + temp_sample * side_amount_r;
            temp_sample -= s.side_sample_r;
            s.side_sample_s = s.side_sample_s * sidealt_amount_s + temp_sample * side_amount_s;
            temp_sample -= s.side_sample_s;
            s.side_sample_t = s.side_sample_t * sidealt_amount_t + temp_sample * side_amount_t;
            temp_sample -= s.side_sample_t;
            s.side_sample_u = s.side_sample_u * sidealt_amount_u + temp_sample * side_amount_u;
            temp_sample -= s.side_sample_u;
            s.side_sample_v = s.side_sample_v * sidealt_amount_v + temp_sample * side_amount_v;
            temp_sample -= s.side_sample_v;
            s.side_sample_w = s.side_sample_w * sidealt_amount_w + temp_sample * side_amount_w;
            temp_sample -= s.side_sample_w;
            s.side_sample_x = s.side_sample_x * sidealt_amount_x + temp_sample * side_amount_x;
            temp_sample -= s.side_sample_x;
            s.side_sample_y = s.side_sample_y * sidealt_amount_y + temp_sample * side_amount_y;
            temp_sample -= s.side_sample_y;
            s.side_sample_z = s.side_sample_z * sidealt_amount_z + temp_sample * side_amount_z;
            temp_sample -= s.side_sample_z;
            side_correction = side - temp_sample;
            correction = side_correction;
            side -= correction;
            s.a_mid[9] = s.a_mid[8];
            s.a_mid[8] = s.a_mid[7];
            s.a_mid[7] = s.a_mid[6];
            s.a_mid[6] = s.a_mid[5];
            s.a_mid[5] = s.a_mid[4];
            s.a_mid[4] = s.a_mid[3];
            s.a_mid[3] = s.a_mid[2];
            s.a_mid[2] = s.a_mid[1];
            s.a_mid[1] = s.a_mid[0];
            accumulator_sample = mid - s.a_mid_prev;
            s.a_mid[0] = accumulator_sample;
            accumulator_sample *= s.f_mid[0];
            accumulator_sample += s.a_mid[1] * s.f_mid[1];
            accumulator_sample += s.a_mid[2] * s.f_mid[2];
            accumulator_sample += s.a_mid[3] * s.f_mid[3];
            accumulator_sample += s.a_mid[4] * s.f_mid[4];
            accumulator_sample += s.a_mid[5] * s.f_mid[5];
            accumulator_sample += s.a_mid[6] * s.f_mid[6];
            accumulator_sample += s.a_mid[7] * s.f_mid[7];
            accumulator_sample += s.a_mid[8] * s.f_mid[8];
            accumulator_sample += s.a_mid[9] * s.f_mid[9];
            correction = mid - s.a_mid_prev - accumulator_sample;
            mid_correction += correction;
            s.a_mid_prev = mid;
            mid -= correction;
            s.a_side[9] = s.a_side[8];
            s.a_side[8] = s.a_side[7];
            s.a_side[7] = s.a_side[6];
            s.a_side[6] = s.a_side[5];
            s.a_side[5] = s.a_side[4];
            s.a_side[4] = s.a_side[3];
            s.a_side[3] = s.a_side[2];
            s.a_side[2] = s.a_side[1];
            s.a_side[1] = s.a_side[0];
            accumulator_sample = side - s.a_side_prev;
            s.a_side[0] = accumulator_sample;
            accumulator_sample *= s.f_side[0];
            accumulator_sample += s.a_side[1] * s.f_side[1];
            accumulator_sample += s.a_side[2] * s.f_side[2];
            accumulator_sample += s.a_side[3] * s.f_side[3];
            accumulator_sample += s.a_side[4] * s.f_side[4];
            accumulator_sample += s.a_side[5] * s.f_side[5];
            accumulator_sample += s.a_side[6] * s.f_side[6];
            accumulator_sample += s.a_side[7] * s.f_side[7];
            accumulator_sample += s.a_side[8] * s.f_side[8];
            accumulator_sample += s.a_side[9] * s.f_side[9];
            correction = side - s.a_side_prev - accumulator_sample;
            side_correction += correction;
            s.a_side_prev = side;
            side -= correction;
            s.b_mid[9] = s.b_mid[8];
            s.b_mid[8] = s.b_mid[7];
            s.b_mid[7] = s.b_mid[6];
            s.b_mid[6] = s.b_mid[5];
            s.b_mid[5] = s.b_mid[4];
            s.b_mid[4] = s.b_mid[3];
            s.b_mid[3] = s.b_mid[2];
            s.b_mid[2] = s.b_mid[1];
            s.b_mid[1] = s.b_mid[0];
            accumulator_sample = mid - s.b_mid_prev;
            s.b_mid[0] = accumulator_sample;
            accumulator_sample *= s.f_mid[0];
            accumulator_sample += s.b_mid[1] * s.f_mid[1];
            accumulator_sample += s.b_mid[2] * s.f_mid[2];
            accumulator_sample += s.b_mid[3] * s.f_mid[3];
            accumulator_sample += s.b_mid[4] * s.f_mid[4];
            accumulator_sample += s.b_mid[5] * s.f_mid[5];
            accumulator_sample += s.b_mid[6] * s.f_mid[6];
            accumulator_sample += s.b_mid[7] * s.f_mid[7];
            accumulator_sample += s.b_mid[8] * s.f_mid[8];
            accumulator_sample += s.b_mid[9] * s.f_mid[9];
            correction = mid - s.b_mid_prev - accumulator_sample;
            mid_correction += correction;
            s.b_mid_prev = mid;
            s.b_side[9] = s.b_side[8];
            s.b_side[8] = s.b_side[7];
            s.b_side[7] = s.b_side[6];
            s.b_side[6] = s.b_side[5];
            s.b_side[5] = s.b_side[4];
            s.b_side[4] = s.b_side[3];
            s.b_side[3] = s.b_side[2];
            s.b_side[2] = s.b_side[1];
            s.b_side[1] = s.b_side[0];
            accumulator_sample = side - s.b_side_prev;
            s.b_side[0] = accumulator_sample;
            accumulator_sample *= s.f_side[0];
            accumulator_sample += s.b_side[1] * s.f_side[1];
            accumulator_sample += s.b_side[2] * s.f_side[2];
            accumulator_sample += s.b_side[3] * s.f_side[3];
            accumulator_sample += s.b_side[4] * s.f_side[4];
            accumulator_sample += s.b_side[5] * s.f_side[5];
            accumulator_sample += s.b_side[6] * s.f_side[6];
            accumulator_sample += s.b_side[7] * s.f_side[7];
            accumulator_sample += s.b_side[8] * s.f_side[8];
            accumulator_sample += s.b_side[9] * s.f_side[9];
            correction = side - s.b_side_prev - accumulator_sample;
            side_correction += correction;
            s.b_side_prev = side;
            mid = temp_mid - mid_correction;
            side = temp_side - side_correction;
            input_sample_l = (mid + side) / 2.0;
            input_sample_r = (mid - side) / 2.0;
            sense_l /= 2.0;
            sense_r /= 2.0;
            accumulator_sample = s.ata_last_out_l * sense_l + input_sample_l * (1.0 - sense_l);
            s.ata_last_out_l = input_sample_l;
            input_sample_l = accumulator_sample;
            accumulator_sample = s.ata_last_out_r * sense_r + input_sample_r * (1.0 - sense_r);
            s.ata_last_out_r = input_sample_r;
            input_sample_r = accumulator_sample;
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
