// Ported from DeBess (DeBessProc.cpp, DeBess.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! De-Bess (sub-effect id 38): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Intense", 0.0),
    pf("Sharp", 0.5),
    pf("Depth", 0.5),
    pf("Filter", 0.5),
    pf("Sense", 0.0),
];

#[allow(dead_code)]
pub struct DeBess {
    p: [f32; 5],
    sr: f32,
    s_l: [f64; 41],
    m_l: [f64; 41],
    c_l: [f64; 41],
    ratio_al: f64,
    ratio_bl: f64,
    iir_sample_al: f64,
    iir_sample_bl: f64,
    s_r: [f64; 41],
    m_r: [f64; 41],
    c_r: [f64; 41],
    ratio_ar: f64,
    ratio_br: f64,
    iir_sample_ar: f64,
    iir_sample_br: f64,
    flip: bool,
    fpd_l: u32,
    fpd_r: u32,
}

impl DeBess {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<DeBess> {
        let mut s = Box::new(DeBess {
            p: [0.0, 0.5, 0.5, 0.5, 0.0],
            sr: sr as f32,
            s_l: [0.0; 41],
            m_l: [0.0; 41],
            c_l: [0.0; 41],
            ratio_al: 0.0,
            ratio_bl: 0.0,
            iir_sample_al: 0.0,
            iir_sample_bl: 0.0,
            s_r: [0.0; 41],
            m_r: [0.0; 41],
            c_r: [0.0; 41],
            ratio_ar: 0.0,
            ratio_br: 0.0,
            iir_sample_ar: 0.0,
            iir_sample_br: 0.0,
            flip: false,
            fpd_l: 0,
            fpd_r: 0,
        });
        s.ratio_bl = 1.0;
        s.ratio_al = s.ratio_bl;
        s.ratio_br = 1.0;
        s.ratio_ar = s.ratio_br;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for DeBess {
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
        let mut intensity: f64 = (s.p[0] as f64).powf(5.0) * (8192.0 / overallscale);
        let mut sharpness: f64 = s.p[1] as f64 * 40.0;
        if sharpness < 2.0 {
            sharpness = 2.0;
        }
        let mut speed: f64 = 0.1 / sharpness;
        let mut depth: f64 = 1.0 / (s.p[2] as f64 + 0.0001);
        let mut iir_amount: f64 = s.p[3] as f64;
        let mut monitoring: f32 = s.p[4];
        for smp in 0..l.len() {
            let mut input_sample_l: f64 = l[smp] as f64;
            let mut input_sample_r: f64 = r[smp] as f64;
            if input_sample_l.abs() < 1.18e-23 {
                input_sample_l = s.fpd_l as f64 * 1.18e-17;
            }
            if input_sample_r.abs() < 1.18e-23 {
                input_sample_r = s.fpd_r as f64 * 1.18e-17;
            }
            s.s_l[0] = input_sample_l;
            s.s_r[0] = input_sample_r;
            {
                let mut x: i32 = sharpness as i32;
                while x > 0 {
                    s.s_l[x as usize] = s.s_l[(x - 1) as usize];
                    s.s_r[x as usize] = s.s_r[(x - 1) as usize];
                    x -= 1;
                }
            }
            s.m_l[1] = (s.s_l[1] - s.s_l[2]) * ((s.s_l[1] - s.s_l[2]) / 1.3);
            s.m_r[1] = (s.s_r[1] - s.s_r[2]) * ((s.s_r[1] - s.s_r[2]) / 1.3);
            {
                let mut x: i32 = (sharpness - 1.0) as i32;
                while x > 1 {
                    s.m_l[x as usize] = (s.s_l[x as usize] - s.s_l[(x + 1) as usize]) * ((s.s_l[(x - 1) as usize] - s.s_l[x as usize]) / 1.3);
                    s.m_r[x as usize] = (s.s_r[x as usize] - s.s_r[(x + 1) as usize]) * ((s.s_r[(x - 1) as usize] - s.s_r[x as usize]) / 1.3);
                    x -= 1;
                }
            }
            let mut sense_l: f64 = (s.m_l[1] - s.m_l[2]).abs() * sharpness * sharpness;
            let mut sense_r: f64 = (s.m_r[1] - s.m_r[2]).abs() * sharpness * sharpness;
            {
                let mut x: i32 = (sharpness - 1.0) as i32;
                while x > 0 {
                    let mut mult_l: f64 = (s.m_l[x as usize] - s.m_l[(x + 1) as usize]).abs() * sharpness * sharpness;
                    if mult_l < 1.0 {
                        sense_l *= mult_l;
                    }
                    let mut mult_r: f64 = (s.m_r[x as usize] - s.m_r[(x + 1) as usize]).abs() * sharpness * sharpness;
                    if mult_r < 1.0 {
                        sense_r *= mult_r;
                    }
                    x -= 1;
                }
            }
            sense_l = 1.0 + intensity * intensity * sense_l;
            if sense_l > intensity {
                sense_l = intensity;
            }
            sense_r = 1.0 + intensity * intensity * sense_r;
            if sense_r > intensity {
                sense_r = intensity;
            }
            if s.flip {
                s.iir_sample_al = s.iir_sample_al * (1.0 - iir_amount) + input_sample_l * iir_amount;
                s.iir_sample_ar = s.iir_sample_ar * (1.0 - iir_amount) + input_sample_r * iir_amount;
                s.ratio_al = s.ratio_al * (1.0 - speed) + sense_l * speed;
                s.ratio_ar = s.ratio_ar * (1.0 - speed) + sense_r * speed;
                if s.ratio_al > depth {
                    s.ratio_al = depth;
                }
                if s.ratio_ar > depth {
                    s.ratio_ar = depth;
                }
                if s.ratio_al > 1.0 {
                    input_sample_l = s.iir_sample_al + (input_sample_l - s.iir_sample_al) / s.ratio_al;
                }
                if s.ratio_ar > 1.0 {
                    input_sample_r = s.iir_sample_ar + (input_sample_r - s.iir_sample_ar) / s.ratio_ar;
                }
            } else {
                s.iir_sample_bl = s.iir_sample_bl * (1.0 - iir_amount) + input_sample_l * iir_amount;
                s.iir_sample_br = s.iir_sample_br * (1.0 - iir_amount) + input_sample_r * iir_amount;
                s.ratio_bl = s.ratio_bl * (1.0 - speed) + sense_l * speed;
                s.ratio_br = s.ratio_br * (1.0 - speed) + sense_r * speed;
                if s.ratio_bl > depth {
                    s.ratio_bl = depth;
                }
                if s.ratio_br > depth {
                    s.ratio_br = depth;
                }
                if s.ratio_al > 1.0 {
                    input_sample_l = s.iir_sample_bl + (input_sample_l - s.iir_sample_bl) / s.ratio_bl;
                }
                if s.ratio_ar > 1.0 {
                    input_sample_r = s.iir_sample_br + (input_sample_r - s.iir_sample_br) / s.ratio_br;
                }
            }
            s.flip = !s.flip;
            if monitoring as f64 > 0.49999 {
                input_sample_l = l[smp] as f64 - input_sample_l;
                input_sample_r = r[smp] as f64 - input_sample_r;
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
