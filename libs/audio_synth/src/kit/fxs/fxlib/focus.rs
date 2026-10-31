// Ported from Focus (FocusProc.cpp, Focus.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Focus (sub-effect id 31): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp};
use crate::kit::core::SlotDef;
#[allow(unused_imports)]
use std::f64::consts::{PI};

pub const PARAMS: &[SlotDef] = &[
    SlotDef::float("Boost", 0.0, 1.0, 0.0, crate::kit::core::SlotUnit::None),
    SlotDef::float("Focus", 0.0, 1.0, 0.5, crate::kit::core::SlotUnit::None),
    SlotDef::choice("Mode", &["Density", "Drive", "Spiral", "Mojo", "Dyno"], 2.0),
    SlotDef::float("Output", 0.0, 1.0, 1.0, crate::kit::core::SlotUnit::None),
    SlotDef::float("Dry/Wet", 0.0, 1.0, 1.0, crate::kit::core::SlotUnit::None),
];

#[allow(dead_code)]
pub struct Focus {
    p: [f32; 5],
    sr: f32,
    figure_l: [f64; 9],
    figure_r: [f64; 9],
    fpd_l: u32,
    fpd_r: u32,
}

impl Focus {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Focus> {
        let mut s = Box::new(Focus {
            p: [0.0, 0.5, 0.5, 1.0, 1.0],
            sr: sr as f32,
            figure_l: [0.0; 9],
            figure_r: [0.0; 9],
            fpd_l: 0,
            fpd_r: 0,
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for Focus {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut boost: f64 = 10.0_f64.powf(s.p[0] as f64 * 12.0 / 20.0);
        s.figure_r[0] = 3515.775 / s.sr as f64;
        s.figure_l[0] = s.figure_r[0];
        s.figure_r[1] = ((s.p[1] as f64).powf(3.0) * 2.0).powf(2.0) + 0.0001;
        s.figure_l[1] = s.figure_r[1];
        let mut mode: i32 = (s.p[2] as f64 * 4.999) as i32;
        let mut output: f64 = s.p[3] as f64;
        let mut wet: f64 = s.p[4] as f64;
        let mut k: f64 = (PI * s.figure_r[0]).tan();
        let mut norm: f64 = 1.0 / (1.0 + k / s.figure_r[1] + k * k);
        s.figure_r[2] = k / s.figure_r[1] * norm;
        s.figure_l[2] = s.figure_r[2];
        s.figure_r[4] = -s.figure_r[2];
        s.figure_l[4] = s.figure_r[4];
        s.figure_r[5] = 2.0 * (k * k - 1.0) * norm;
        s.figure_l[5] = s.figure_r[5];
        s.figure_r[6] = (1.0 - k / s.figure_r[1] + k * k) * norm;
        s.figure_l[6] = s.figure_r[6];
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
            input_sample_l = input_sample_l.sin();
            input_sample_r = input_sample_r.sin();
            let mut temp_sample: f64 = input_sample_l * s.figure_l[2] + s.figure_l[7];
            s.figure_l[7] = -(temp_sample * s.figure_l[5]) + s.figure_l[8];
            s.figure_l[8] = input_sample_l * s.figure_l[4] - temp_sample * s.figure_l[6];
            input_sample_l = temp_sample;
            temp_sample = input_sample_r * s.figure_r[2] + s.figure_r[7];
            s.figure_r[7] = -(temp_sample * s.figure_r[5]) + s.figure_r[8];
            s.figure_r[8] = input_sample_r * s.figure_r[4] - temp_sample * s.figure_r[6];
            input_sample_r = temp_sample;
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
            input_sample_l = input_sample_l.asin();
            input_sample_r = input_sample_r.asin();
            let mut ground_sample_l: f64 = dry_sample_l - input_sample_l;
            let mut ground_sample_r: f64 = dry_sample_r - input_sample_r;
            input_sample_l *= boost;
            input_sample_r *= boost;
            match mode {
                0 => {
                    if input_sample_l > 1.570796326794897 {
                        input_sample_l = 1.570796326794897;
                    }
                    if input_sample_l < -1.570796326794897 {
                        input_sample_l = -1.570796326794897;
                    }
                    if input_sample_r > 1.570796326794897 {
                        input_sample_r = 1.570796326794897;
                    }
                    if input_sample_r < -1.570796326794897 {
                        input_sample_r = -1.570796326794897;
                    }
                    input_sample_l = input_sample_l.sin();
                    input_sample_r = input_sample_r.sin();
                }
                1 => {
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
                    input_sample_l -= input_sample_l * (input_sample_l.abs() * 0.6) * (input_sample_l.abs() * 0.6);
                    input_sample_r -= input_sample_r * (input_sample_r.abs() * 0.6) * (input_sample_r.abs() * 0.6);
                    input_sample_l *= 1.6;
                    input_sample_r *= 1.6;
                }
                2 => {
                    if input_sample_l > 1.2533141373155 {
                        input_sample_l = 1.2533141373155;
                    }
                    if input_sample_l < -1.2533141373155 {
                        input_sample_l = -1.2533141373155;
                    }
                    if input_sample_r > 1.2533141373155 {
                        input_sample_r = 1.2533141373155;
                    }
                    if input_sample_r < -1.2533141373155 {
                        input_sample_r = -1.2533141373155;
                    }
                    input_sample_l = (input_sample_l * input_sample_l.abs()).sin() / (if input_sample_l.abs() == 0.0 { 1.0 } else { input_sample_l.abs() });
                    input_sample_r = (input_sample_r * input_sample_r.abs()).sin() / (if input_sample_r.abs() == 0.0 { 1.0 } else { input_sample_r.abs() });
                }
                3 => {
                    let mut mojo: f64 = 0.0;
                    mojo = input_sample_l.abs().powf(0.25);
                    if mojo > 0.0 {
                        input_sample_l = (input_sample_l * mojo * PI * 0.5).sin() / mojo * 0.987654321;
                    }
                    mojo = input_sample_r.abs().powf(0.25);
                    if mojo > 0.0 {
                        input_sample_r = (input_sample_r * mojo * PI * 0.5).sin() / mojo * 0.987654321;
                    }
                }
                4 => {
                    let mut dyno: f64 = 0.0;
                    dyno = input_sample_l.abs().powf(4.0);
                    if dyno > 0.0 {
                        input_sample_l = (input_sample_l * dyno).sin() / dyno * 1.1654321;
                    }
                    dyno = input_sample_r.abs().powf(4.0);
                    if dyno > 0.0 {
                        input_sample_r = (input_sample_r * dyno).sin() / dyno * 1.1654321;
                    }
                }
                _ => {}
            }
            if output != 1.0 {
                input_sample_l *= output;
                input_sample_r *= output;
            }
            input_sample_l += ground_sample_l;
            input_sample_r += ground_sample_r;
            if wet != 1.0 {
                input_sample_l = input_sample_l * wet + dry_sample_l * (1.0 - wet);
                input_sample_r = input_sample_r * wet + dry_sample_r * (1.0 - wet);
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
