// Ported from GlitchShifter (GlitchShifterProc.cpp, GlitchShifter.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Glitch Shifter (sub-effect id 63): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Note", 0.5),
    pf("Trim", 0.5),
    pf("Tighten", 0.5),
    pf("Feedbck", 0.0),
    pf("Dry/Wet", 0.5),
];

#[allow(dead_code)]
pub struct GlitchShifter {
    p: [f32; 5],
    sr: f32,
    p_l: Vec<i32>,
    offset_l: Vec<i32>,
    pastzero_l: Vec<i32>,
    previous_l: Vec<i32>,
    third_l: Vec<i32>,
    fourth_l: Vec<i32>,
    temp_l: i32,
    lasttemp_l: i32,
    thirdtemp_l: i32,
    fourthtemp_l: i32,
    sincezerocross_l: i32,
    crosses_l: i32,
    realzeroes_l: i32,
    position_l: f64,
    splicing_l: bool,
    air_prev_l: f64,
    air_even_l: f64,
    air_odd_l: f64,
    air_factor_l: f64,
    p_r: Vec<i32>,
    offset_r: Vec<i32>,
    pastzero_r: Vec<i32>,
    previous_r: Vec<i32>,
    third_r: Vec<i32>,
    fourth_r: Vec<i32>,
    temp_r: i32,
    lasttemp_r: i32,
    thirdtemp_r: i32,
    fourthtemp_r: i32,
    sincezerocross_r: i32,
    crosses_r: i32,
    realzeroes_r: i32,
    position_r: f64,
    splicing_r: bool,
    air_prev_r: f64,
    air_even_r: f64,
    air_odd_r: f64,
    air_factor_r: f64,
    gcount: i32,
    lastwidth: i32,
    flip: bool,
    fpd_l: u32,
    fpd_r: u32,
}

impl GlitchShifter {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<GlitchShifter> {
        let mut s = Box::new(GlitchShifter {
            p: [0.5, 0.5, 0.5, 0.0, 0.5],
            sr: sr as f32,
            p_l: vec![0; 131076],
            offset_l: vec![0; 258],
            pastzero_l: vec![0; 258],
            previous_l: vec![0; 258],
            third_l: vec![0; 258],
            fourth_l: vec![0; 258],
            temp_l: 0,
            lasttemp_l: 0,
            thirdtemp_l: 0,
            fourthtemp_l: 0,
            sincezerocross_l: 0,
            crosses_l: 0,
            realzeroes_l: 0,
            position_l: 0.0,
            splicing_l: false,
            air_prev_l: 0.0,
            air_even_l: 0.0,
            air_odd_l: 0.0,
            air_factor_l: 0.0,
            p_r: vec![0; 131076],
            offset_r: vec![0; 258],
            pastzero_r: vec![0; 258],
            previous_r: vec![0; 258],
            third_r: vec![0; 258],
            fourth_r: vec![0; 258],
            temp_r: 0,
            lasttemp_r: 0,
            thirdtemp_r: 0,
            fourthtemp_r: 0,
            sincezerocross_r: 0,
            crosses_r: 0,
            realzeroes_r: 0,
            position_r: 0.0,
            splicing_r: false,
            air_prev_r: 0.0,
            air_even_r: 0.0,
            air_odd_r: 0.0,
            air_factor_r: 0.0,
            gcount: 0,
            lastwidth: 0,
            flip: false,
            fpd_l: 0,
            fpd_r: 0,
        });
        s.lastwidth = 16386;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for GlitchShifter {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut note: i32 = (s.p[0] as f64 * 24.999) as i32 - 12;
        let mut trim: f64 = s.p[1] as f64 * 2.0 - 1.0;
        let mut speed: f64 = 1.0 / 12.0 * note as f64 + trim;
        if speed < 0.0 {
            speed *= 0.5;
        }
        let mut width: i32 = (65536.0 - (1.0 - ((1.0 - s.p[2]) as f64).powf(2.0)) * 65530.0) as i32;
        let mut bias: f64 = (s.p[2] as f64).powf(3.0);
        let mut feedback: f64 = s.p[3] as f64 / 1.5;
        let mut wet: f64 = s.p[4] as f64;
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
            s.air_factor_l = s.air_prev_l - input_sample_l;
            if s.flip {
                s.air_even_l += s.air_factor_l;
                s.air_odd_l -= s.air_factor_l;
                s.air_factor_l = s.air_even_l;
            } else {
                s.air_odd_l += s.air_factor_l;
                s.air_even_l -= s.air_factor_l;
                s.air_factor_l = s.air_odd_l;
            }
            s.air_odd_l = (s.air_odd_l - (s.air_odd_l - s.air_even_l) / 256.0) / 1.0001;
            s.air_even_l = (s.air_even_l - (s.air_even_l - s.air_odd_l) / 256.0) / 1.0001;
            s.air_prev_l = input_sample_l;
            input_sample_l += s.air_factor_l;
            s.air_factor_r = s.air_prev_r - input_sample_r;
            if s.flip {
                s.air_even_r += s.air_factor_r;
                s.air_odd_r -= s.air_factor_r;
                s.air_factor_r = s.air_even_r;
            } else {
                s.air_odd_r += s.air_factor_r;
                s.air_even_r -= s.air_factor_r;
                s.air_factor_r = s.air_odd_r;
            }
            s.air_odd_r = (s.air_odd_r - (s.air_odd_r - s.air_even_r) / 256.0) / 1.0001;
            s.air_even_r = (s.air_even_r - (s.air_even_r - s.air_odd_r) / 256.0) / 1.0001;
            s.air_prev_r = input_sample_r;
            input_sample_r += s.air_factor_r;
            s.flip = !s.flip;
            if s.lastwidth != width {
                s.crosses_l = 0;
                s.realzeroes_l = 0;
                s.crosses_r = 0;
                s.realzeroes_r = 0;
                s.lastwidth = width;
            }
            s.gcount += 1;
            if s.gcount < 0 || s.gcount > width {
                s.gcount = 0;
            }
            let mut count: i32 = s.gcount;
            let mut countone: i32 = count - 1;
            let mut counttwo: i32 = count - 2;
            while count < 0 {
                count += width;
            }
            while countone < 0 {
                countone += width;
            }
            while counttwo < 0 {
                counttwo += width;
            }
            while count > width {
                count -= width;
            }
            while countone > width {
                countone -= width;
            }
            while counttwo > width {
                counttwo -= width;
            }
            s.p_l[count as usize] = (input_sample_l * 8388352.0 + s.lasttemp_l as f64 * feedback) as i32;
            s.p_l[(count + width) as usize] = s.p_l[count as usize];
            s.p_r[count as usize] = (input_sample_r * 8388352.0 + s.lasttemp_r as f64 * feedback) as i32;
            s.p_r[(count + width) as usize] = s.p_r[count as usize];
            if s.p_l[countone as usize] > 0 && s.p_l[count as usize] < 0 || s.p_l[countone as usize] < 0 && s.p_l[count as usize] > 0 {
                s.crosses_l += 1;
                s.realzeroes_l += 1;
                if s.crosses_l > 256 {
                    s.crosses_l = 0;
                }
                if s.realzeroes_l > 256 {
                    s.realzeroes_l = 256;
                }
                s.offset_l[s.crosses_l as usize] = count;
                s.pastzero_l[s.crosses_l as usize] = s.p_l[count as usize];
                s.previous_l[s.crosses_l as usize] = s.p_l[countone as usize];
                s.third_l[s.crosses_l as usize] = s.p_l[counttwo as usize];
            }
            if s.p_r[countone as usize] > 0 && s.p_r[count as usize] < 0 || s.p_r[countone as usize] < 0 && s.p_r[count as usize] > 0 {
                s.crosses_r += 1;
                s.realzeroes_r += 1;
                if s.crosses_r > 256 {
                    s.crosses_r = 0;
                }
                if s.realzeroes_r > 256 {
                    s.realzeroes_r = 256;
                }
                s.offset_r[s.crosses_r as usize] = count;
                s.pastzero_r[s.crosses_r as usize] = s.p_r[count as usize];
                s.previous_r[s.crosses_r as usize] = s.p_r[countone as usize];
                s.third_r[s.crosses_r as usize] = s.p_r[counttwo as usize];
            }
            s.position_l -= speed;
            if s.position_l > width as f64 {
                if s.realzeroes_l > 0 {
                    s.position_l = 0.0;
                    let mut diff: f64 = 99999999.0;
                    let mut best: i32 = 0;
                    let mut scan: i32 = 0;
                    {
                        scan = s.realzeroes_l - 1;
                        while scan >= 0 {
                            let mut scanone: i32 = scan + s.crosses_l;
                            if scanone > 256 {
                                scanone -= 256;
                            }
                            let mut howdiff: f64 = (s.temp_l - s.pastzero_l[scanone as usize] + (s.lasttemp_l - s.previous_l[scanone as usize]) + (s.thirdtemp_l - s.third_l[scanone as usize]) + (s.fourthtemp_l - s.fourth_l[scanone as usize])) as f64;
                            howdiff -= scan as f64 * bias;
                            if howdiff < diff {
                                diff = howdiff;
                                best = scanone;
                            }
                            scan -= 1;
                        }
                    }
                    s.position_l = (s.offset_l[best as usize] - s.sincezerocross_l) as f64;
                    s.crosses_l = 0;
                    s.realzeroes_l = 0;
                    s.splicing_l = true;
                } else {
                    s.position_l -= width as f64;
                    s.crosses_l = 0;
                    s.realzeroes_l = 0;
                    s.splicing_l = true;
                }
            }
            if s.position_l < 0.0 {
                if s.realzeroes_l > 0 {
                    s.position_l = 0.0;
                    let mut diff: f64 = 99999999.0;
                    let mut best: i32 = 0;
                    let mut scan: i32 = 0;
                    {
                        scan = s.realzeroes_l - 1;
                        while scan >= 0 {
                            let mut scanone: i32 = scan + s.crosses_l;
                            if scanone > 256 {
                                scanone -= 256;
                            }
                            let mut howdiff: f64 = (s.temp_l - s.pastzero_l[scanone as usize] + (s.lasttemp_l - s.previous_l[scanone as usize]) + (s.thirdtemp_l - s.third_l[scanone as usize]) + (s.fourthtemp_l - s.fourth_l[scanone as usize])) as f64;
                            howdiff -= scan as f64 * bias;
                            if howdiff < diff {
                                diff = howdiff;
                                best = scanone;
                            }
                            scan -= 1;
                        }
                    }
                    s.position_l = (s.offset_l[best as usize] - s.sincezerocross_l) as f64;
                    s.crosses_l = 0;
                    s.realzeroes_l = 0;
                    s.splicing_l = true;
                } else {
                    s.position_l += width as f64;
                    s.crosses_l = 0;
                    s.realzeroes_l = 0;
                    s.splicing_l = true;
                }
            }
            s.position_r -= speed;
            if s.position_r > width as f64 {
                if s.realzeroes_r > 0 {
                    s.position_r = 0.0;
                    let mut diff: f64 = 99999999.0;
                    let mut best: i32 = 0;
                    let mut scan: i32 = 0;
                    {
                        scan = s.realzeroes_r - 1;
                        while scan >= 0 {
                            let mut scanone: i32 = scan + s.crosses_r;
                            if scanone > 256 {
                                scanone -= 256;
                            }
                            let mut howdiff: f64 = (s.temp_r - s.pastzero_r[scanone as usize] + (s.lasttemp_r - s.previous_r[scanone as usize]) + (s.thirdtemp_r - s.third_r[scanone as usize]) + (s.fourthtemp_r - s.fourth_r[scanone as usize])) as f64;
                            howdiff -= scan as f64 * bias;
                            if howdiff < diff {
                                diff = howdiff;
                                best = scanone;
                            }
                            scan -= 1;
                        }
                    }
                    s.position_r = (s.offset_r[best as usize] - s.sincezerocross_r) as f64;
                    s.crosses_r = 0;
                    s.realzeroes_r = 0;
                    s.splicing_r = true;
                } else {
                    s.position_r -= width as f64;
                    s.crosses_r = 0;
                    s.realzeroes_r = 0;
                    s.splicing_r = true;
                }
            }
            if s.position_r < 0.0 {
                if s.realzeroes_r > 0 {
                    s.position_r = 0.0;
                    let mut diff: f64 = 99999999.0;
                    let mut best: i32 = 0;
                    let mut scan: i32 = 0;
                    {
                        scan = s.realzeroes_r - 1;
                        while scan >= 0 {
                            let mut scanone: i32 = scan + s.crosses_r;
                            if scanone > 256 {
                                scanone -= 256;
                            }
                            let mut howdiff: f64 = (s.temp_r - s.pastzero_r[scanone as usize] + (s.lasttemp_r - s.previous_r[scanone as usize]) + (s.thirdtemp_r - s.third_r[scanone as usize]) + (s.fourthtemp_r - s.fourth_r[scanone as usize])) as f64;
                            howdiff -= scan as f64 * bias;
                            if howdiff < diff {
                                diff = howdiff;
                                best = scanone;
                            }
                            scan -= 1;
                        }
                    }
                    s.position_r = (s.offset_r[best as usize] - s.sincezerocross_r) as f64;
                    s.crosses_r = 0;
                    s.realzeroes_r = 0;
                    s.splicing_r = true;
                } else {
                    s.position_r += width as f64;
                    s.crosses_r = 0;
                    s.realzeroes_r = 0;
                    s.splicing_r = true;
                }
            }
            count = s.gcount - s.position_l.floor() as i32;
            countone = count + 1;
            counttwo = count + 2;
            while count < 0 {
                count += width;
            }
            while countone < 0 {
                countone += width;
            }
            while counttwo < 0 {
                counttwo += width;
            }
            while count > width {
                count -= width;
            }
            while countone > width {
                countone -= width;
            }
            while counttwo > width {
                counttwo -= width;
            }
            s.temp_l = 0;
            s.temp_l += (s.p_l[count as usize] as f64 * (1.0 - (s.position_l - s.position_l.floor()))) as i32;
            s.temp_l += s.p_l[(count + 1) as usize];
            s.temp_l += (s.p_l[(count + 2) as usize] as f64 * (s.position_l - s.position_l.floor())) as i32;
            s.temp_l -= (s.p_l[count as usize] - s.p_l[(count + 1) as usize] - (s.p_l[(count + 1) as usize] - s.p_l[(count + 2) as usize])) / 50;
            s.temp_l /= 2;
            if s.temp_l.abs() as f64 > 8388352.0 {
                s.temp_l = s.lasttemp_l + (s.lasttemp_l - s.thirdtemp_l);
            }
            s.sincezerocross_l += 1;
            if s.sincezerocross_l < 0 || s.sincezerocross_l > width {
                s.sincezerocross_l = 0;
            }
            if s.splicing_l {
                s.temp_l = (s.temp_l + (s.lasttemp_l + (s.lasttemp_l - s.thirdtemp_l))) / 2;
                s.splicing_l = false;
            }
            if s.lasttemp_l > 0 && s.temp_l < 0 || s.lasttemp_l < 0 && s.temp_l > 0 {
                s.sincezerocross_l = 0;
            }
            count = s.gcount - s.position_r.floor() as i32;
            countone = count + 1;
            counttwo = count + 2;
            while count < 0 {
                count += width;
            }
            while countone < 0 {
                countone += width;
            }
            while counttwo < 0 {
                counttwo += width;
            }
            while count > width {
                count -= width;
            }
            while countone > width {
                countone -= width;
            }
            while counttwo > width {
                counttwo -= width;
            }
            s.temp_r = 0;
            s.temp_r += (s.p_r[count as usize] as f64 * (1.0 - (s.position_r - s.position_r.floor()))) as i32;
            s.temp_r += s.p_r[(count + 1) as usize];
            s.temp_r += (s.p_r[(count + 2) as usize] as f64 * (s.position_r - s.position_r.floor())) as i32;
            s.temp_r -= (s.p_r[count as usize] - s.p_r[(count + 1) as usize] - (s.p_r[(count + 1) as usize] - s.p_r[(count + 2) as usize])) / 50;
            s.temp_r /= 2;
            if s.temp_r.abs() as f64 > 8388352.0 {
                s.temp_r = s.lasttemp_r + (s.lasttemp_r - s.thirdtemp_r);
            }
            s.sincezerocross_r += 1;
            if s.sincezerocross_r < 0 || s.sincezerocross_r > width {
                s.sincezerocross_r = 0;
            }
            if s.splicing_r {
                s.temp_r = (s.temp_r + (s.lasttemp_r + (s.lasttemp_r - s.thirdtemp_r))) / 2;
                s.splicing_r = false;
            }
            if s.lasttemp_r > 0 && s.temp_r < 0 || s.lasttemp_r < 0 && s.temp_r > 0 {
                s.sincezerocross_r = 0;
            }
            s.fourthtemp_l = s.thirdtemp_l;
            s.thirdtemp_l = s.lasttemp_l;
            s.lasttemp_l = s.temp_l;
            s.fourthtemp_r = s.thirdtemp_r;
            s.thirdtemp_r = s.lasttemp_r;
            s.lasttemp_r = s.temp_r;
            input_sample_l = dry_sample_l * (1.0 - wet) + s.temp_l as f64 / 8388352.0 * wet;
            if input_sample_l > 4.0 {
                input_sample_l = 4.0;
            }
            if input_sample_l < -4.0 {
                input_sample_l = -4.0;
            }
            input_sample_r = dry_sample_r * (1.0 - wet) + s.temp_r as f64 / 8388352.0 * wet;
            if input_sample_r > 4.0 {
                input_sample_r = 4.0;
            }
            if input_sample_r < -4.0 {
                input_sample_r = -4.0;
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
