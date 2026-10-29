// Ported from ButterComp2 (ButterComp2Proc.cpp, ButterComp2.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Butter Comp (sub-effect id 2): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Compress", 0.0),
    pf("Output", 0.5),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct ButterComp2 {
    p: [f32; 3],
    sr: f32,
    control_apos_l: f64,
    control_aneg_l: f64,
    control_bpos_l: f64,
    control_bneg_l: f64,
    targetpos_l: f64,
    targetneg_l: f64,
    last_output_l: f64,
    control_apos_r: f64,
    control_aneg_r: f64,
    control_bpos_r: f64,
    control_bneg_r: f64,
    targetpos_r: f64,
    targetneg_r: f64,
    last_output_r: f64,
    flip: bool,
    fpd_l: u32,
    fpd_r: u32,
    /// upstream: a function-local static (process-wide there; per instance here)
    noisesource_l: i32,
    /// upstream: a function-local static (process-wide there; per instance here)
    noisesource_r: i32,
}

impl ButterComp2 {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<ButterComp2> {
        let mut s = Box::new(ButterComp2 {
            p: [0.0, 0.5, 1.0],
            sr: sr as f32,
            control_apos_l: 0.0,
            control_aneg_l: 0.0,
            control_bpos_l: 0.0,
            control_bneg_l: 0.0,
            targetpos_l: 0.0,
            targetneg_l: 0.0,
            last_output_l: 0.0,
            control_apos_r: 0.0,
            control_aneg_r: 0.0,
            control_bpos_r: 0.0,
            control_bneg_r: 0.0,
            targetpos_r: 0.0,
            targetneg_r: 0.0,
            last_output_r: 0.0,
            flip: false,
            fpd_l: 0,
            fpd_r: 0,
            noisesource_l: 0,
            noisesource_r: 850010,
        });
        s.control_apos_l = 1.0;
        s.control_aneg_l = 1.0;
        s.control_bpos_l = 1.0;
        s.control_bneg_l = 1.0;
        s.targetpos_l = 1.0;
        s.targetneg_l = 1.0;
        s.control_apos_r = 1.0;
        s.control_aneg_r = 1.0;
        s.control_bpos_r = 1.0;
        s.control_bneg_r = 1.0;
        s.targetpos_r = 1.0;
        s.targetneg_r = 1.0;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for ButterComp2 {
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
        let mut inputgain: f64 = 10.0_f64.powf(s.p[0] as f64 * 14.0 / 20.0);
        let mut compfactor: f64 = 0.012 * (s.p[0] as f64 / 135.0);
        let mut output: f64 = s.p[1] as f64 * 2.0;
        let mut wet: f64 = s.p[2] as f64;
        let mut outputgain: f64 = inputgain;
        outputgain -= 1.0;
        outputgain /= 1.5;
        outputgain += 1.0;
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
            input_sample_l *= inputgain;
            input_sample_r *= inputgain;
            let mut divisor: f64 = compfactor / (1.0 + s.last_output_l.abs());
            divisor /= overallscale;
            let mut remainder: f64 = divisor;
            divisor = 1.0 - divisor;
            let mut inputpos_l: f64 = input_sample_l + 1.0;
            if inputpos_l < 0.0 {
                inputpos_l = 0.0;
            }
            let mut outputpos_l: f64 = inputpos_l / 2.0;
            if outputpos_l > 1.0 {
                outputpos_l = 1.0;
            }
            inputpos_l *= inputpos_l;
            s.targetpos_l *= divisor;
            s.targetpos_l += inputpos_l * remainder;
            let mut calcpos_l: f64 = (1.0 / s.targetpos_l).powf(2.0);
            let mut inputneg_l: f64 = -input_sample_l + 1.0;
            if inputneg_l < 0.0 {
                inputneg_l = 0.0;
            }
            let mut outputneg_l: f64 = inputneg_l / 2.0;
            if outputneg_l > 1.0 {
                outputneg_l = 1.0;
            }
            inputneg_l *= inputneg_l;
            s.targetneg_l *= divisor;
            s.targetneg_l += inputneg_l * remainder;
            let mut calcneg_l: f64 = (1.0 / s.targetneg_l).powf(2.0);
            if input_sample_l > 0.0 {
                if s.flip {
                    s.control_apos_l *= divisor;
                    s.control_apos_l += calcpos_l * remainder;
                } else {
                    s.control_bpos_l *= divisor;
                    s.control_bpos_l += calcpos_l * remainder;
                }
            } else {
                if s.flip {
                    s.control_aneg_l *= divisor;
                    s.control_aneg_l += calcneg_l * remainder;
                } else {
                    s.control_bneg_l *= divisor;
                    s.control_bneg_l += calcneg_l * remainder;
                }
            }
            divisor = compfactor / (1.0 + s.last_output_r.abs());
            divisor /= overallscale;
            remainder = divisor;
            divisor = 1.0 - divisor;
            let mut inputpos_r: f64 = input_sample_r + 1.0;
            if inputpos_r < 0.0 {
                inputpos_r = 0.0;
            }
            let mut outputpos_r: f64 = inputpos_r / 2.0;
            if outputpos_r > 1.0 {
                outputpos_r = 1.0;
            }
            inputpos_r *= inputpos_r;
            s.targetpos_r *= divisor;
            s.targetpos_r += inputpos_r * remainder;
            let mut calcpos_r: f64 = (1.0 / s.targetpos_r).powf(2.0);
            let mut inputneg_r: f64 = -input_sample_r + 1.0;
            if inputneg_r < 0.0 {
                inputneg_r = 0.0;
            }
            let mut outputneg_r: f64 = inputneg_r / 2.0;
            if outputneg_r > 1.0 {
                outputneg_r = 1.0;
            }
            inputneg_r *= inputneg_r;
            s.targetneg_r *= divisor;
            s.targetneg_r += inputneg_r * remainder;
            let mut calcneg_r: f64 = (1.0 / s.targetneg_r).powf(2.0);
            if input_sample_r > 0.0 {
                if s.flip {
                    s.control_apos_r *= divisor;
                    s.control_apos_r += calcpos_r * remainder;
                } else {
                    s.control_bpos_r *= divisor;
                    s.control_bpos_r += calcpos_r * remainder;
                }
            } else {
                if s.flip {
                    s.control_aneg_r *= divisor;
                    s.control_aneg_r += calcneg_r * remainder;
                } else {
                    s.control_bneg_r *= divisor;
                    s.control_bneg_r += calcneg_r * remainder;
                }
            }
            let mut totalmultiplier_l: f64 = 0.0;
            let mut totalmultiplier_r: f64 = 0.0;
            if s.flip {
                totalmultiplier_l = s.control_apos_l * outputpos_l + s.control_aneg_l * outputneg_l;
                totalmultiplier_r = s.control_apos_r * outputpos_r + s.control_aneg_r * outputneg_r;
            } else {
                totalmultiplier_l = s.control_bpos_l * outputpos_l + s.control_bneg_l * outputneg_l;
                totalmultiplier_r = s.control_bpos_r * outputpos_r + s.control_bneg_r * outputneg_r;
            }
            input_sample_l *= totalmultiplier_l;
            input_sample_l /= outputgain;
            input_sample_r *= totalmultiplier_r;
            input_sample_r /= outputgain;
            if output != 1.0 {
                input_sample_l *= output;
                input_sample_r *= output;
            }
            if wet != 1.0 {
                input_sample_l = input_sample_l * wet + dry_sample_l * (1.0 - wet);
                input_sample_r = input_sample_r * wet + dry_sample_r * (1.0 - wet);
            }
            s.last_output_l = input_sample_l;
            s.last_output_r = input_sample_r;
            s.flip = !s.flip;
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
