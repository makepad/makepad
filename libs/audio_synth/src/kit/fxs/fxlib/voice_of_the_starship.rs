// Ported from VoiceOfTheStarship (VoiceOfTheStarshipProc.cpp, VoiceOfTheStarship.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Voice Of The Starship (sub-effect id 20): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Filter", 0.5),
    pf("Algrthm", 0.0),
];

#[allow(dead_code)]
pub struct VoiceOfTheStarship {
    p: [f32; 2],
    sr: f32,
    noise_al: f64,
    noise_bl: f64,
    noise_cl: f64,
    noise_ar: f64,
    noise_br: f64,
    noise_cr: f64,
    position: i32,
    quadratic: i32,
    flip_l: bool,
    flip_r: bool,
    filterflip: bool,
    b_l: [f64; 11],
    b_r: [f64; 11],
    f: [f64; 11],
    last_algorithm: i32,
    fpd_l: u32,
    fpd_r: u32,
}

impl VoiceOfTheStarship {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<VoiceOfTheStarship> {
        let mut s = Box::new(VoiceOfTheStarship {
            p: [0.5, 0.0],
            sr: sr as f32,
            noise_al: 0.0,
            noise_bl: 0.0,
            noise_cl: 0.0,
            noise_ar: 0.0,
            noise_br: 0.0,
            noise_cr: 0.0,
            position: 0,
            quadratic: 0,
            flip_l: false,
            flip_r: false,
            filterflip: false,
            b_l: [0.0; 11],
            b_r: [0.0; 11],
            f: [0.0; 11],
            last_algorithm: 0,
            fpd_l: 0,
            fpd_r: 0,
        });
        s.position = 99999999;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for VoiceOfTheStarship {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut cutoff: f64 = (s.p[0] as f64 * 0.89 + 0.1).powf(3.0);
        if cutoff > 1.0 {
            cutoff = 1.0;
        }
        let mut invcutoff: f64 = 1.0 - cutoff;
        let mut overallscale: f64 = (1.0 - s.p[0] as f64) * 9.0 + 1.0;
        let mut gain: f64 = overallscale;
        if gain > 1.0 {
            s.f[0] = 1.0;
            gain -= 1.0;
        } else {
            s.f[0] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f[1] = 1.0;
            gain -= 1.0;
        } else {
            s.f[1] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f[2] = 1.0;
            gain -= 1.0;
        } else {
            s.f[2] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f[3] = 1.0;
            gain -= 1.0;
        } else {
            s.f[3] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f[4] = 1.0;
            gain -= 1.0;
        } else {
            s.f[4] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f[5] = 1.0;
            gain -= 1.0;
        } else {
            s.f[5] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f[6] = 1.0;
            gain -= 1.0;
        } else {
            s.f[6] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f[7] = 1.0;
            gain -= 1.0;
        } else {
            s.f[7] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f[8] = 1.0;
            gain -= 1.0;
        } else {
            s.f[8] = gain;
            gain = 0.0;
        }
        if gain > 1.0 {
            s.f[9] = 1.0;
            gain -= 1.0;
        } else {
            s.f[9] = gain;
            gain = 0.0;
        }
        if overallscale < 1.0 {
            overallscale = 1.0;
        }
        s.f[0] /= overallscale;
        s.f[1] /= overallscale;
        s.f[2] /= overallscale;
        s.f[3] /= overallscale;
        s.f[4] /= overallscale;
        s.f[5] /= overallscale;
        s.f[6] /= overallscale;
        s.f[7] /= overallscale;
        s.f[8] /= overallscale;
        s.f[9] /= overallscale;
        let mut lowcut: i32 = (s.p[1] as f64 * 16.9).floor() as i32;
        if s.last_algorithm != lowcut {
            s.noise_al = 0.0;
            s.noise_bl = 0.0;
            s.noise_cl = 0.0;
            s.noise_ar = 0.0;
            s.noise_br = 0.0;
            s.noise_cr = 0.0;
            {
                let mut count: i32 = 0;
                while count < 11 {
                    s.b_l[count as usize] = 0.0;
                    s.b_r[count as usize] = 0.0;
                    count += 1;
                }
            }
            s.last_algorithm = lowcut;
        }
        let mut dcut: i32 = 0;
        if lowcut > 15 {
            lowcut = 1151;
            dcut = 11517;
        }
        if lowcut == 15 {
            lowcut = 113;
            dcut = 1151;
        }
        if lowcut == 14 {
            lowcut = 71;
            dcut = 719;
        }
        if lowcut == 13 {
            lowcut = 53;
            dcut = 541;
        }
        if lowcut == 12 {
            lowcut = 31;
            dcut = 311;
        }
        if lowcut == 11 {
            lowcut = 23;
            dcut = 233;
        }
        if lowcut == 10 {
            lowcut = 19;
            dcut = 191;
        }
        if lowcut == 9 {
            lowcut = 17;
            dcut = 173;
        }
        if lowcut == 8 {
            lowcut = 13;
            dcut = 131;
        }
        if lowcut == 7 {
            lowcut = 11;
            dcut = 113;
        }
        if lowcut == 6 {
            lowcut = 7;
            dcut = 79;
        }
        if lowcut == 5 {
            lowcut = 6;
            dcut = 67;
        }
        if lowcut == 4 {
            lowcut = 5;
            dcut = 59;
        }
        if lowcut == 3 {
            lowcut = 4;
            dcut = 43;
        }
        if lowcut == 2 {
            lowcut = 3;
            dcut = 37;
        }
        if lowcut == 1 {
            lowcut = 2;
            dcut = 23;
        }
        if lowcut < 1 {
            lowcut = 1;
            dcut = 11;
        }
        let mut rumbletrim: f64 = (lowcut as f64).sqrt();
        let mut input_sample_l: f64 = 0.0;
        let mut input_sample_r: f64 = 0.0;
        for smp in 0..l.len() {
            input_sample_l = l[smp] as f64;
            input_sample_r = r[smp] as f64;
            s.quadratic -= 1;
            if s.quadratic < 0 {
                s.position += 1;
                s.quadratic = s.position.wrapping_mul(s.position);
                s.quadratic = s.quadratic % 170003;
                s.quadratic *= s.quadratic;
                s.quadratic = s.quadratic % 17011;
                s.quadratic *= s.quadratic;
                s.quadratic = s.quadratic % 1709;
                s.quadratic *= s.quadratic;
                s.quadratic = s.quadratic % dcut;
                s.quadratic *= s.quadratic;
                s.quadratic = s.quadratic % lowcut;
                if s.noise_al < 0.0 {
                    s.flip_l = true;
                } else {
                    s.flip_l = false;
                }
                if s.noise_ar < 0.0 {
                    s.flip_r = true;
                } else {
                    s.flip_r = false;
                }
            }
            if s.flip_l {
                s.noise_al += s.fpd_l as f64 / u32::MAX as f64;
            } else {
                s.noise_al -= s.fpd_l as f64 / u32::MAX as f64;
            }
            if s.flip_r {
                s.noise_ar += s.fpd_r as f64 / u32::MAX as f64;
            } else {
                s.noise_ar -= s.fpd_r as f64 / u32::MAX as f64;
            }
            if s.filterflip {
                s.noise_bl *= invcutoff;
                s.noise_bl += s.noise_al * cutoff;
                input_sample_l = s.noise_bl;
                s.noise_br *= invcutoff;
                s.noise_br += s.noise_ar * cutoff;
                input_sample_r = s.noise_br;
            } else {
                s.noise_cl *= invcutoff;
                s.noise_cl += s.noise_al * cutoff;
                input_sample_l = s.noise_cl;
                s.noise_cr *= invcutoff;
                s.noise_cr += s.noise_ar * cutoff;
                input_sample_r = s.noise_cr;
            }
            s.b_l[9] = s.b_l[8];
            s.b_l[8] = s.b_l[7];
            s.b_l[7] = s.b_l[6];
            s.b_l[6] = s.b_l[5];
            s.b_l[5] = s.b_l[4];
            s.b_l[4] = s.b_l[3];
            s.b_l[3] = s.b_l[2];
            s.b_l[2] = s.b_l[1];
            s.b_l[1] = s.b_l[0];
            s.b_l[0] = input_sample_l;
            s.b_r[9] = s.b_r[8];
            s.b_r[8] = s.b_r[7];
            s.b_r[7] = s.b_r[6];
            s.b_r[6] = s.b_r[5];
            s.b_r[5] = s.b_r[4];
            s.b_r[4] = s.b_r[3];
            s.b_r[3] = s.b_r[2];
            s.b_r[2] = s.b_r[1];
            s.b_r[1] = s.b_r[0];
            s.b_r[0] = input_sample_r;
            input_sample_l *= s.f[0];
            input_sample_l += s.b_l[1] * s.f[1];
            input_sample_l += s.b_l[2] * s.f[2];
            input_sample_l += s.b_l[3] * s.f[3];
            input_sample_l += s.b_l[4] * s.f[4];
            input_sample_l += s.b_l[5] * s.f[5];
            input_sample_l += s.b_l[6] * s.f[6];
            input_sample_l += s.b_l[7] * s.f[7];
            input_sample_l += s.b_l[8] * s.f[8];
            input_sample_l += s.b_l[9] * s.f[9];
            input_sample_r *= s.f[0];
            input_sample_r += s.b_r[1] * s.f[1];
            input_sample_r += s.b_r[2] * s.f[2];
            input_sample_r += s.b_r[3] * s.f[3];
            input_sample_r += s.b_r[4] * s.f[4];
            input_sample_r += s.b_r[5] * s.f[5];
            input_sample_r += s.b_r[6] * s.f[6];
            input_sample_r += s.b_r[7] * s.f[7];
            input_sample_r += s.b_r[8] * s.f[8];
            input_sample_r += s.b_r[9] * s.f[9];
            input_sample_l *= 0.1;
            input_sample_r *= 0.1;
            input_sample_l *= invcutoff;
            input_sample_r *= invcutoff;
            input_sample_l /= rumbletrim;
            input_sample_r /= rumbletrim;
            s.flip_l = !s.flip_l;
            s.flip_r = !s.flip_r;
            s.filterflip = !s.filterflip;
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
