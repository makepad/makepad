// Ported from BitGlitter (BitGlitterProc.cpp, BitGlitter.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Bit Glitter (sub-effect id 13): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Input", 0.5),
    pf("Glitter", 0.0),
    pf("Output", 0.5),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct BitGlitter {
    p: [f32; 4],
    sr: f32,
    fpd_l: u32,
    fpd_r: u32,
    ata_last_sample_l: f64,
    ata_halfway_sample_l: f64,
    ata_dry_sample_l: f64,
    last_sample_l: f64,
    held_sample_al: f64,
    held_sample_bl: f64,
    position_al: f64,
    position_bl: f64,
    last_output_sample_l: f64,
    ata_last_sample_r: f64,
    ata_halfway_sample_r: f64,
    ata_dry_sample_r: f64,
    last_sample_r: f64,
    held_sample_ar: f64,
    held_sample_br: f64,
    position_ar: f64,
    position_br: f64,
    last_output_sample_r: f64,
}

impl BitGlitter {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<BitGlitter> {
        let mut s = Box::new(BitGlitter {
            p: [0.5, 0.0, 0.5, 1.0],
            sr: sr as f32,
            fpd_l: 0,
            fpd_r: 0,
            ata_last_sample_l: 0.0,
            ata_halfway_sample_l: 0.0,
            ata_dry_sample_l: 0.0,
            last_sample_l: 0.0,
            held_sample_al: 0.0,
            held_sample_bl: 0.0,
            position_al: 0.0,
            position_bl: 0.0,
            last_output_sample_l: 0.0,
            ata_last_sample_r: 0.0,
            ata_halfway_sample_r: 0.0,
            ata_dry_sample_r: 0.0,
            last_sample_r: 0.0,
            held_sample_ar: 0.0,
            held_sample_br: 0.0,
            position_ar: 0.0,
            position_br: 0.0,
            last_output_sample_r: 0.0,
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for BitGlitter {
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
        let mut factor: f64 = s.p[1] as f64 + 1.0;
        factor = factor.powf(7.0) + 2.0;
        let mut divvy: i32 = (factor * overallscale) as i32;
        let mut rate_a: f64 = 1.0 / divvy as f64;
        let mut rez_a: f64 = 0.0016666666666667;
        let mut rate_b: f64 = 1.61803398875 / divvy as f64;
        let mut rez_b: f64 = 0.0026666666666667;
        let mut offset: f64 = 0.0;
        let mut ingain: f64 = 10.0_f64.powf((s.p[0] as f64 * 36.0 - 18.0) / 14.0);
        let mut outgain: f64 = 10.0_f64.powf((s.p[2] as f64 * 36.0 - 18.0) / 14.0);
        let mut wet: f64 = s.p[3] as f64;
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
            input_sample_l *= ingain;
            input_sample_r *= ingain;
            if input_sample_l > 1.0 {
                input_sample_l = 1.0;
            }
            if input_sample_l < -1.0 {
                input_sample_l = -1.0;
            }
            input_sample_l *= 1.2533141373155;
            input_sample_l = (input_sample_l * input_sample_l.abs()).sin() / (if input_sample_l.abs() == 0.0 { 1.0 } else { input_sample_l.abs() });
            if input_sample_r > 1.0 {
                input_sample_r = 1.0;
            }
            if input_sample_r < -1.0 {
                input_sample_r = -1.0;
            }
            input_sample_r *= 1.2533141373155;
            input_sample_r = (input_sample_r * input_sample_r.abs()).sin() / (if input_sample_r.abs() == 0.0 { 1.0 } else { input_sample_r.abs() });
            s.ata_dry_sample_l = input_sample_l;
            s.ata_halfway_sample_l = (input_sample_l + s.ata_last_sample_l) / 2.0;
            s.ata_last_sample_l = input_sample_l;
            s.ata_dry_sample_r = input_sample_r;
            s.ata_halfway_sample_r = (input_sample_r + s.ata_last_sample_r) / 2.0;
            s.ata_last_sample_r = input_sample_r;
            s.position_al += rate_a;
            let mut output_sample_l: f64 = s.held_sample_al;
            if s.position_al > 1.0 {
                s.position_al -= 1.0;
                s.held_sample_al = s.last_sample_l * s.position_al + input_sample_l * (1.0 - s.position_al);
                output_sample_l = output_sample_l * 0.5 + s.held_sample_al * 0.5;
            }
            if output_sample_l > 0.0 {
                offset = output_sample_l;
                while offset > 0.0 {
                    offset -= rez_a;
                }
                output_sample_l -= offset;
            }
            if output_sample_l < 0.0 {
                offset = output_sample_l;
                while offset < 0.0 {
                    offset += rez_a;
                }
                output_sample_l -= offset;
            }
            output_sample_l *= 1.0 - rez_a;
            if output_sample_l.abs() < rez_a {
                output_sample_l = 0.0;
            }
            input_sample_l = output_sample_l;
            s.position_ar += rate_a;
            let mut output_sample_r: f64 = s.held_sample_ar;
            if s.position_ar > 1.0 {
                s.position_ar -= 1.0;
                s.held_sample_ar = s.last_sample_r * s.position_ar + input_sample_r * (1.0 - s.position_ar);
                output_sample_r = output_sample_r * 0.5 + s.held_sample_ar * 0.5;
            }
            if output_sample_r > 0.0 {
                offset = output_sample_r;
                while offset > 0.0 {
                    offset -= rez_a;
                }
                output_sample_r -= offset;
            }
            if output_sample_r < 0.0 {
                offset = output_sample_r;
                while offset < 0.0 {
                    offset += rez_a;
                }
                output_sample_r -= offset;
            }
            output_sample_r *= 1.0 - rez_a;
            if output_sample_r.abs() < rez_a {
                output_sample_r = 0.0;
            }
            input_sample_r = output_sample_r;
            s.position_bl += rate_b;
            output_sample_l = s.held_sample_bl;
            if s.position_bl > 1.0 {
                s.position_bl -= 1.0;
                s.held_sample_bl = s.last_sample_l * s.position_bl + s.ata_halfway_sample_l * (1.0 - s.position_bl);
                output_sample_l = output_sample_l * 0.5 + s.held_sample_bl * 0.5;
            }
            if output_sample_l > 0.0 {
                offset = output_sample_l;
                while offset > 0.0 {
                    offset -= rez_b;
                }
                output_sample_l -= offset;
            }
            if output_sample_l < 0.0 {
                offset = output_sample_l;
                while offset < 0.0 {
                    offset += rez_b;
                }
                output_sample_l -= offset;
            }
            output_sample_l *= 1.0 - rez_b;
            if output_sample_l.abs() < rez_b {
                output_sample_l = 0.0;
            }
            s.ata_halfway_sample_l = output_sample_l;
            s.position_br += rate_b;
            output_sample_r = s.held_sample_br;
            if s.position_br > 1.0 {
                s.position_br -= 1.0;
                s.held_sample_br = s.last_sample_r * s.position_br + s.ata_halfway_sample_r * (1.0 - s.position_br);
                output_sample_r = output_sample_r * 0.5 + s.held_sample_br * 0.5;
            }
            if output_sample_r > 0.0 {
                offset = output_sample_r;
                while offset > 0.0 {
                    offset -= rez_b;
                }
                output_sample_r -= offset;
            }
            if output_sample_r < 0.0 {
                offset = output_sample_r;
                while offset < 0.0 {
                    offset += rez_b;
                }
                output_sample_r -= offset;
            }
            output_sample_r *= 1.0 - rez_b;
            if output_sample_r.abs() < rez_b {
                output_sample_r = 0.0;
            }
            s.ata_halfway_sample_r = output_sample_r;
            input_sample_l += s.ata_halfway_sample_l;
            input_sample_l /= 2.0;
            input_sample_r += s.ata_halfway_sample_r;
            input_sample_r /= 2.0;
            output_sample_l = input_sample_l * (1.0 - wet / 2.0) + s.last_output_sample_l * (wet / 2.0);
            s.last_output_sample_l = input_sample_l;
            output_sample_l *= outgain;
            output_sample_r = input_sample_r * (1.0 - wet / 2.0) + s.last_output_sample_r * (wet / 2.0);
            s.last_output_sample_r = input_sample_r;
            output_sample_r *= outgain;
            if wet < 1.0 {
                output_sample_l = dry_sample_l * (1.0 - wet) + output_sample_l * wet;
                output_sample_r = dry_sample_r * (1.0 - wet) + output_sample_r * wet;
            }
            input_sample_l = output_sample_l;
            input_sample_r = output_sample_r;
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
