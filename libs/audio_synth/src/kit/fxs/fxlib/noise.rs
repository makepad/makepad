// Ported from Noise (NoiseProc.cpp, Noise.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Noise (sub-effect id 19): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("HighCut", 0.5),
    pf("LowCut", 0.5),
    pf("LShape", 0.5),
    pf("Decay", 1.0),
    pf("Distnc", 0.0),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct Noise {
    p: [f32; 6],
    sr: f32,
    noise_al: f64,
    noise_bl: f64,
    noise_cl: f64,
    rumble_al: f64,
    rumble_bl: f64,
    swell_l: f64,
    noise_ar: f64,
    noise_br: f64,
    noise_cr: f64,
    rumble_ar: f64,
    rumble_br: f64,
    swell_r: f64,
    position: i32,
    quadratic: i32,
    flip_l: bool,
    flip_r: bool,
    filterflip: bool,
    b_l: [f64; 11],
    b_r: [f64; 11],
    f: [f64; 11],
    fpd_l: u32,
    fpd_r: u32,
}

impl Noise {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Noise> {
        let mut s = Box::new(Noise {
            p: [0.5, 0.5, 0.5, 1.0, 0.0, 1.0],
            sr: sr as f32,
            noise_al: 0.0,
            noise_bl: 0.0,
            noise_cl: 0.0,
            rumble_al: 0.0,
            rumble_bl: 0.0,
            swell_l: 0.0,
            noise_ar: 0.0,
            noise_br: 0.0,
            noise_cr: 0.0,
            rumble_ar: 0.0,
            rumble_br: 0.0,
            swell_r: 0.0,
            position: 0,
            quadratic: 0,
            flip_l: false,
            flip_r: false,
            filterflip: false,
            b_l: [0.0; 11],
            b_r: [0.0; 11],
            f: [0.0; 11],
            fpd_l: 0,
            fpd_r: 0,
        });
        s.position = 99999999;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for Noise {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut cutoff_l: f64 = 0.0;
        let mut cutoff_r: f64 = 0.0;
        let mut cutofftarget: f64 = s.p[0] as f64 * 3.5;
        let mut rumblecutoff: f64 = cutofftarget * 0.005;
        let mut invcutoff_l: f64 = 0.0;
        let mut invcutoff_r: f64 = 0.0;
        let mut dry_sample_l: f64 = 0.0;
        let mut dry_sample_r: f64 = 0.0;
        let mut input_sample_l: f64 = 0.0;
        let mut input_sample_r: f64 = 0.0;
        let mut highpass: f64 = s.p[2] as f64 * 38.0;
        let mut lowcut: i32 = highpass.floor() as i32;
        let mut dcut: i32 = 0;
        if lowcut > 37 {
            dcut = 1151;
        }
        if lowcut == 37 {
            dcut = 1091;
        }
        if lowcut == 36 {
            dcut = 1087;
        }
        if lowcut == 35 {
            dcut = 1031;
        }
        if lowcut == 34 {
            dcut = 1013;
        }
        if lowcut == 33 {
            dcut = 971;
        }
        if lowcut == 32 {
            dcut = 907;
        }
        if lowcut == 31 {
            dcut = 839;
        }
        if lowcut == 30 {
            dcut = 797;
        }
        if lowcut == 29 {
            dcut = 733;
        }
        if lowcut == 28 {
            dcut = 719;
        }
        if lowcut == 27 {
            dcut = 673;
        }
        if lowcut == 26 {
            dcut = 613;
        }
        if lowcut == 25 {
            dcut = 593;
        }
        if lowcut == 24 {
            dcut = 541;
        }
        if lowcut == 23 {
            dcut = 479;
        }
        if lowcut == 22 {
            dcut = 431;
        }
        if lowcut == 21 {
            dcut = 419;
        }
        if lowcut == 20 {
            dcut = 373;
        }
        if lowcut == 19 {
            dcut = 311;
        }
        if lowcut == 18 {
            dcut = 293;
        }
        if lowcut == 17 {
            dcut = 233;
        }
        if lowcut == 16 {
            dcut = 191;
        }
        if lowcut == 15 {
            dcut = 173;
        }
        if lowcut == 14 {
            dcut = 131;
        }
        if lowcut == 13 {
            dcut = 113;
        }
        if lowcut == 12 {
            dcut = 71;
        }
        if lowcut == 11 {
            dcut = 53;
        }
        if lowcut == 10 {
            dcut = 31;
        }
        if lowcut == 9 {
            dcut = 27;
        }
        if lowcut == 8 {
            dcut = 23;
        }
        if lowcut == 7 {
            dcut = 19;
        }
        if lowcut == 6 {
            dcut = 17;
        }
        if lowcut == 5 {
            dcut = 13;
        }
        if lowcut == 4 {
            dcut = 11;
        }
        if lowcut == 3 {
            dcut = 7;
        }
        if lowcut == 2 {
            dcut = 5;
        }
        if lowcut < 2 {
            dcut = 3;
        }
        highpass = s.p[1] as f64 * 22.0;
        lowcut = (highpass.floor() + 1.0) as i32;
        let mut decay: f64 = 0.001 - (1.0 - (1.0 - s.p[3] as f64).powf(3.0)) * 0.001;
        if decay == 0.001 {
            decay = 0.1;
        }
        let mut wet: f64 = s.p[5] as f64;
        wet *= 0.01;
        let mut correction_sample: f64 = 0.0;
        let mut accumulator_sample_l: f64 = 0.0;
        let mut accumulator_sample_r: f64 = 0.0;
        let mut overallscale: f64 = s.p[4] as f64 * 9.0 + 1.0;
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
            if s.swell_l < input_sample_l.abs() {
                s.swell_l += s.fpd_l as f64 / u32::MAX as f64 * (input_sample_l.abs() - s.swell_l);
                if s.swell_l > 1.0 {
                    s.swell_l = 1.0;
                }
            } else {
                s.swell_l -= s.fpd_l as f64 / u32::MAX as f64 * (s.swell_l - input_sample_l.abs()) * decay;
                if s.swell_l < 0.0 {
                    s.swell_l = 0.0;
                }
            }
            cutoff_l = (cutofftarget * s.swell_l).powf(5.0);
            if cutoff_l > 1.0 {
                cutoff_l = 1.0;
            }
            invcutoff_l = 1.0 - cutoff_l;
            if s.swell_r < input_sample_r.abs() {
                s.swell_r += s.fpd_r as f64 / u32::MAX as f64 * (input_sample_r.abs() - s.swell_r);
                if s.swell_r > 1.0 {
                    s.swell_r = 1.0;
                }
            } else {
                s.swell_r -= s.fpd_r as f64 / u32::MAX as f64 * (s.swell_r - input_sample_r.abs()) * decay;
                if s.swell_r < 0.0 {
                    s.swell_r = 0.0;
                }
            }
            cutoff_r = (cutofftarget * s.swell_r).powf(5.0);
            if cutoff_r > 1.0 {
                cutoff_r = 1.0;
            }
            invcutoff_r = 1.0 - cutoff_r;
            s.flip_l = !s.flip_l;
            s.flip_r = !s.flip_r;
            s.filterflip = !s.filterflip;
            s.quadratic -= 1;
            if s.quadratic < 0 {
                s.position += 1;
                s.quadratic = s.position.wrapping_mul(s.position);
                s.quadratic = s.quadratic % 170003;
                s.quadratic *= s.quadratic;
                s.quadratic = s.quadratic % 17011;
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
            s.fpd_l ^= s.fpd_l << 13;
            s.fpd_l ^= s.fpd_l >> 17;
            s.fpd_l ^= s.fpd_l << 5;
            s.fpd_r ^= s.fpd_r << 13;
            s.fpd_r ^= s.fpd_r >> 17;
            s.fpd_r ^= s.fpd_r << 5;
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
                s.noise_bl *= invcutoff_l;
                s.noise_bl += s.noise_al * cutoff_l;
                input_sample_l = s.noise_bl + s.noise_cl;
                s.rumble_al *= 1.0 - rumblecutoff;
                s.rumble_al += input_sample_l * rumblecutoff;
                s.noise_br *= invcutoff_r;
                s.noise_br += s.noise_ar * cutoff_r;
                input_sample_r = s.noise_br + s.noise_cr;
                s.rumble_ar *= 1.0 - rumblecutoff;
                s.rumble_ar += input_sample_r * rumblecutoff;
            } else {
                s.noise_cl *= invcutoff_l;
                s.noise_cl += s.noise_al * cutoff_l;
                input_sample_l = s.noise_bl + s.noise_cl;
                s.rumble_bl *= 1.0 - rumblecutoff;
                s.rumble_bl += input_sample_l * rumblecutoff;
                s.noise_cr *= invcutoff_r;
                s.noise_cr += s.noise_ar * cutoff_r;
                input_sample_r = s.noise_br + s.noise_cr;
                s.rumble_br *= 1.0 - rumblecutoff;
                s.rumble_br += input_sample_r * rumblecutoff;
            }
            input_sample_l -= s.rumble_al + s.rumble_bl;
            input_sample_l *= 1.0 - rumblecutoff;
            input_sample_r -= s.rumble_ar + s.rumble_br;
            input_sample_r *= 1.0 - rumblecutoff;
            input_sample_l *= wet;
            input_sample_l += dry_sample_l * (1.0 - wet);
            input_sample_r *= wet;
            input_sample_r += dry_sample_r * (1.0 - wet);
            s.b_l[9] = s.b_l[8];
            s.b_l[8] = s.b_l[7];
            s.b_l[7] = s.b_l[6];
            s.b_l[6] = s.b_l[5];
            s.b_l[5] = s.b_l[4];
            s.b_l[4] = s.b_l[3];
            s.b_l[3] = s.b_l[2];
            s.b_l[2] = s.b_l[1];
            s.b_l[1] = s.b_l[0];
            accumulator_sample_l = input_sample_l;
            s.b_l[0] = accumulator_sample_l;
            s.b_r[9] = s.b_r[8];
            s.b_r[8] = s.b_r[7];
            s.b_r[7] = s.b_r[6];
            s.b_r[6] = s.b_r[5];
            s.b_r[5] = s.b_r[4];
            s.b_r[4] = s.b_r[3];
            s.b_r[3] = s.b_r[2];
            s.b_r[2] = s.b_r[1];
            s.b_r[1] = s.b_r[0];
            accumulator_sample_r = input_sample_r;
            s.b_r[0] = accumulator_sample_r;
            accumulator_sample_l *= s.f[0];
            accumulator_sample_l += s.b_l[1] * s.f[1];
            accumulator_sample_l += s.b_l[2] * s.f[2];
            accumulator_sample_l += s.b_l[3] * s.f[3];
            accumulator_sample_l += s.b_l[4] * s.f[4];
            accumulator_sample_l += s.b_l[5] * s.f[5];
            accumulator_sample_l += s.b_l[6] * s.f[6];
            accumulator_sample_l += s.b_l[7] * s.f[7];
            accumulator_sample_l += s.b_l[8] * s.f[8];
            accumulator_sample_l += s.b_l[9] * s.f[9];
            accumulator_sample_r *= s.f[0];
            accumulator_sample_r += s.b_r[1] * s.f[1];
            accumulator_sample_r += s.b_r[2] * s.f[2];
            accumulator_sample_r += s.b_r[3] * s.f[3];
            accumulator_sample_r += s.b_r[4] * s.f[4];
            accumulator_sample_r += s.b_r[5] * s.f[5];
            accumulator_sample_r += s.b_r[6] * s.f[6];
            accumulator_sample_r += s.b_r[7] * s.f[7];
            accumulator_sample_r += s.b_r[8] * s.f[8];
            accumulator_sample_r += s.b_r[9] * s.f[9];
            correction_sample = input_sample_l - accumulator_sample_l;
            input_sample_l -= correction_sample;
            correction_sample = input_sample_r - accumulator_sample_r;
            input_sample_r -= correction_sample;
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
