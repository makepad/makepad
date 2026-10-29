// Ported from DeRez2 (DeRez2Proc.cpp, DeRez2.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! DeRez (sub-effect id 15): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Rate", 1.0),
    pf("Rez", 1.0),
    pf("Hard", 1.0),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct DeRez2 {
    p: [f32; 4],
    sr: f32,
    last_sample_l: f64,
    held_sample_l: f64,
    last_dry_sample_l: f64,
    last_output_sample_l: f64,
    last_sample_r: f64,
    held_sample_r: f64,
    last_dry_sample_r: f64,
    last_output_sample_r: f64,
    position: f64,
    increment_a: f64,
    increment_b: f64,
    fpd_l: u32,
    fpd_r: u32,
}

impl DeRez2 {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<DeRez2> {
        let mut s = Box::new(DeRez2 {
            p: [1.0, 1.0, 1.0, 1.0],
            sr: sr as f32,
            last_sample_l: 0.0,
            held_sample_l: 0.0,
            last_dry_sample_l: 0.0,
            last_output_sample_l: 0.0,
            last_sample_r: 0.0,
            held_sample_r: 0.0,
            last_dry_sample_r: 0.0,
            last_output_sample_r: 0.0,
            position: 0.0,
            increment_a: 0.0,
            increment_b: 0.0,
            fpd_l: 0,
            fpd_r: 0,
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for DeRez2 {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut target_a: f64 = (s.p[0] as f64).powf(3.0) + 0.0005;
        if target_a > 1.0 {
            target_a = 1.0;
        }
        let mut soften: f64 = (1.0 + target_a) / 2.0;
        let mut target_b: f64 = (1.0 - s.p[1] as f64).powf(3.0) / 3.0;
        let mut hard: f64 = s.p[2] as f64;
        let mut wet: f64 = s.p[3] as f64;
        let mut overallscale: f64 = 1.0;
        overallscale /= 44100.0;
        overallscale *= s.sr as f64;
        target_a /= overallscale;
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
            s.increment_a = (s.increment_a * 999.0 + target_a) / 1000.0;
            s.increment_b = (s.increment_b * 999.0 + target_b) / 1000.0;
            s.position += s.increment_a;
            let mut output_sample_l: f64 = s.held_sample_l;
            let mut output_sample_r: f64 = s.held_sample_r;
            if s.position > 1.0 {
                s.position -= 1.0;
                s.held_sample_l = s.last_sample_l * s.position + input_sample_l * (1.0 - s.position);
                output_sample_l = output_sample_l * (1.0 - soften) + s.held_sample_l * soften;
                s.held_sample_r = s.last_sample_r * s.position + input_sample_r * (1.0 - s.position);
                output_sample_r = output_sample_r * (1.0 - soften) + s.held_sample_r * soften;
            }
            input_sample_l = output_sample_l;
            input_sample_r = output_sample_r;
            let mut temp_l: f64 = input_sample_l;
            let mut temp_r: f64 = input_sample_r;
            if input_sample_l != s.last_output_sample_l {
                temp_l = input_sample_l;
                input_sample_l = input_sample_l * hard + s.last_dry_sample_l * (1.0 - hard);
                s.last_output_sample_l = temp_l;
            } else {
                s.last_output_sample_l = input_sample_l;
            }
            if input_sample_r != s.last_output_sample_r {
                temp_r = input_sample_r;
                input_sample_r = input_sample_r * hard + s.last_dry_sample_r * (1.0 - hard);
                s.last_output_sample_r = temp_r;
            } else {
                s.last_output_sample_r = input_sample_r;
            }
            s.last_dry_sample_l = dry_sample_l;
            s.last_dry_sample_r = dry_sample_r;
            temp_l = input_sample_l;
            temp_r = input_sample_r;
            if input_sample_l > 1.0 {
                input_sample_l = 1.0;
            }
            if input_sample_l < -1.0 {
                input_sample_l = -1.0;
            }
            if input_sample_r > 1.0 {
                input_sample_r = 1.0;
            }
            if input_sample_r < -1.0 {
                input_sample_r = -1.0;
            }
            if input_sample_l > 0.0 {
                input_sample_l = (1.0 + 255.0 * input_sample_l.abs()).ln() / 256.0_f64.ln();
            }
            if input_sample_l < 0.0 {
                input_sample_l = -(1.0 + 255.0 * input_sample_l.abs()).ln() / 256.0_f64.ln();
            }
            if input_sample_r > 0.0 {
                input_sample_r = (1.0 + 255.0 * input_sample_r.abs()).ln() / 256.0_f64.ln();
            }
            if input_sample_r < 0.0 {
                input_sample_r = -(1.0 + 255.0 * input_sample_r.abs()).ln() / 256.0_f64.ln();
            }
            input_sample_l = temp_l * hard + input_sample_l * (1.0 - hard);
            input_sample_r = temp_r * hard + input_sample_r * (1.0 - hard);
            temp_l = input_sample_l;
            temp_r = input_sample_r;
            if s.increment_b > 0.0005 {
                if input_sample_l > 0.0 {
                    temp_l = input_sample_l;
                    while temp_l > 0.0 {
                        temp_l -= s.increment_b;
                    }
                    input_sample_l -= temp_l;
                }
                if input_sample_r > 0.0 {
                    temp_r = input_sample_r;
                    while temp_r > 0.0 {
                        temp_r -= s.increment_b;
                    }
                    input_sample_r -= temp_r;
                }
                if input_sample_l < 0.0 {
                    temp_l = input_sample_l;
                    while temp_l < 0.0 {
                        temp_l += s.increment_b;
                    }
                    input_sample_l -= temp_l;
                }
                if input_sample_r < 0.0 {
                    temp_r = input_sample_r;
                    while temp_r < 0.0 {
                        temp_r += s.increment_b;
                    }
                    input_sample_r -= temp_r;
                }
                input_sample_l *= 1.0 - s.increment_b;
                input_sample_r *= 1.0 - s.increment_b;
            }
            temp_l = input_sample_l;
            temp_r = input_sample_r;
            if input_sample_l > 1.0 {
                input_sample_l = 1.0;
            }
            if input_sample_l < -1.0 {
                input_sample_l = -1.0;
            }
            if input_sample_r > 1.0 {
                input_sample_r = 1.0;
            }
            if input_sample_r < -1.0 {
                input_sample_r = -1.0;
            }
            if input_sample_l > 0.0 {
                input_sample_l = (256.0_f64.powf(input_sample_l.abs()) - 1.0) / 255.0;
            }
            if input_sample_l < 0.0 {
                input_sample_l = -(256.0_f64.powf(input_sample_l.abs()) - 1.0) / 255.0;
            }
            if input_sample_r > 0.0 {
                input_sample_r = (256.0_f64.powf(input_sample_r.abs()) - 1.0) / 255.0;
            }
            if input_sample_r < 0.0 {
                input_sample_r = -(256.0_f64.powf(input_sample_r.abs()) - 1.0) / 255.0;
            }
            input_sample_l = temp_l * hard + input_sample_l * (1.0 - hard);
            input_sample_r = temp_r * hard + input_sample_r * (1.0 - hard);
            if wet != 1.0 {
                input_sample_l = input_sample_l * wet + dry_sample_l * (1.0 - wet);
                input_sample_r = input_sample_r * wet + dry_sample_r * (1.0 - wet);
            }
            s.last_sample_l = dry_sample_l;
            s.last_sample_r = dry_sample_r;
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
