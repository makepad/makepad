// Ported from DubCenter (DubCenterProc.cpp, DubCenter.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Dub Center (sub-effect id 61): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("TGrind", 0.9),
    pf("Grd/Out", 0.5),
    pf("XOver", 0.74),
    pf("BsDrive", 1.0),
    pf("BsVoice", 0.95),
    pf("BassOut", 0.5),
    pf("SbDrive", 0.2),
    pf("SbVoice", 0.2),
    pf("SubOut", 0.5),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct DubCenter {
    p: [f32; 10],
    sr: f32,
    iir_drive_sample_al: f64,
    iir_drive_sample_bl: f64,
    iir_drive_sample_cl: f64,
    iir_drive_sample_dl: f64,
    iir_drive_sample_el: f64,
    iir_drive_sample_fl: f64,
    iir_drive_sample_ar: f64,
    iir_drive_sample_br: f64,
    iir_drive_sample_cr: f64,
    iir_drive_sample_dr: f64,
    iir_drive_sample_er: f64,
    iir_drive_sample_fr: f64,
    flip: bool,
    bflip: i32,
    was_negative: bool,
    sub_octave: bool,
    iir_head_bump_a: f64,
    iir_head_bump_b: f64,
    iir_head_bump_c: f64,
    iir_sub_bump_a: f64,
    iir_sub_bump_b: f64,
    iir_sub_bump_c: f64,
    last_head_bump: f64,
    last_sub_bump: f64,
    iir_sample_a: f64,
    iir_sample_b: f64,
    iir_sample_c: f64,
    iir_sample_d: f64,
    iir_sample_e: f64,
    iir_sample_f: f64,
    iir_sample_g: f64,
    iir_sample_h: f64,
    iir_sample_i: f64,
    iir_sample_j: f64,
    iir_sample_k: f64,
    iir_sample_l: f64,
    iir_sample_m: f64,
    iir_sample_n: f64,
    iir_sample_o: f64,
    iir_sample_p: f64,
    iir_sample_q: f64,
    iir_sample_r: f64,
    iir_sample_s: f64,
    iir_sample_t: f64,
    iir_sample_u: f64,
    iir_sample_v: f64,
    iir_sample_w: f64,
    iir_sample_x: f64,
    iir_sample_y: f64,
    iir_sample_z: f64,
    osc_gate: f64,
    fpd_l: u32,
    fpd_r: u32,
}

impl DubCenter {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<DubCenter> {
        let mut s = Box::new(DubCenter {
            p: [0.9, 0.5, 0.74, 1.0, 0.95, 0.5, 0.2, 0.2, 0.5, 1.0],
            sr: sr as f32,
            iir_drive_sample_al: 0.0,
            iir_drive_sample_bl: 0.0,
            iir_drive_sample_cl: 0.0,
            iir_drive_sample_dl: 0.0,
            iir_drive_sample_el: 0.0,
            iir_drive_sample_fl: 0.0,
            iir_drive_sample_ar: 0.0,
            iir_drive_sample_br: 0.0,
            iir_drive_sample_cr: 0.0,
            iir_drive_sample_dr: 0.0,
            iir_drive_sample_er: 0.0,
            iir_drive_sample_fr: 0.0,
            flip: false,
            bflip: 0,
            was_negative: false,
            sub_octave: false,
            iir_head_bump_a: 0.0,
            iir_head_bump_b: 0.0,
            iir_head_bump_c: 0.0,
            iir_sub_bump_a: 0.0,
            iir_sub_bump_b: 0.0,
            iir_sub_bump_c: 0.0,
            last_head_bump: 0.0,
            last_sub_bump: 0.0,
            iir_sample_a: 0.0,
            iir_sample_b: 0.0,
            iir_sample_c: 0.0,
            iir_sample_d: 0.0,
            iir_sample_e: 0.0,
            iir_sample_f: 0.0,
            iir_sample_g: 0.0,
            iir_sample_h: 0.0,
            iir_sample_i: 0.0,
            iir_sample_j: 0.0,
            iir_sample_k: 0.0,
            iir_sample_l: 0.0,
            iir_sample_m: 0.0,
            iir_sample_n: 0.0,
            iir_sample_o: 0.0,
            iir_sample_p: 0.0,
            iir_sample_q: 0.0,
            iir_sample_r: 0.0,
            iir_sample_s: 0.0,
            iir_sample_t: 0.0,
            iir_sample_u: 0.0,
            iir_sample_v: 0.0,
            iir_sample_w: 0.0,
            iir_sample_x: 0.0,
            iir_sample_y: 0.0,
            iir_sample_z: 0.0,
            osc_gate: 0.0,
            fpd_l: 0,
            fpd_r: 0,
        });
        s.osc_gate = 1.0;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for DubCenter {
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
        let mut driveone: f64 = (s.p[0] as f64 * 3.0).powf(2.0);
        let mut driveoutput: f64 = s.p[1] as f64 * 2.0 - 1.0;
        let mut iir_amount: f64 = (s.p[2] as f64 * 0.33 + 0.1) / overallscale;
        let mut ata_lowpass_l: f64 = 0.0;
        let mut ata_lowpass_r: f64 = 0.0;
        let mut ata_lowpass: f64 = 0.0;
        let mut randy: f64 = 0.0;
        let mut invrandy: f64 = 0.0;
        let mut head_bump: f64 = 0.0;
        let mut bass_gain: f64 = s.p[3] as f64 * 0.1;
        let mut head_bump_freq: f64 = (s.p[4] as f64 * 0.1 + 0.0001) / overallscale;
        let mut iir_bmount: f64 = head_bump_freq / 44.1;
        let mut alt_bmount: f64 = 1.0 - iir_bmount;
        let mut bass_out_gain: f64 = s.p[5] as f64 * 2.0 - 1.0;
        let mut sub_bump: f64 = 0.0;
        let mut sub_gain: f64 = s.p[6] as f64 * 0.1;
        let mut sub_bump_freq: f64 = (s.p[7] as f64 * 0.1 + 0.0001) / overallscale;
        let mut iir_cmount: f64 = sub_bump_freq / 44.1;
        let mut alt_cmount: f64 = 1.0 - iir_cmount;
        let mut sub_out_gain: f64 = s.p[8] as f64 * 2.0 - 1.0;
        let mut clamp: f64 = 0.0;
        let mut out: f64 = 0.0;
        let mut fuzz: f64 = 0.111;
        let mut wet: f64 = s.p[9] as f64;
        let mut glitch: f64 = 0.60;
        let mut temp_sample: f64 = 0.0;
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
            s.osc_gate += ((input_sample_l + input_sample_r) * 5.0).abs();
            s.osc_gate -= 0.001;
            if s.osc_gate > 1.0 {
                s.osc_gate = 1.0;
            }
            if s.osc_gate < 0.0 {
                s.osc_gate = 0.0;
            }
            clamp = 1.0 - s.osc_gate;
            clamp *= 0.00001;
            if s.flip {
                temp_sample = input_sample_l;
                s.iir_drive_sample_al = s.iir_drive_sample_al * (1.0 - iir_amount) + input_sample_l * iir_amount;
                input_sample_l -= s.iir_drive_sample_al;
                s.iir_drive_sample_cl = s.iir_drive_sample_cl * (1.0 - iir_amount) + input_sample_l * iir_amount;
                input_sample_l -= s.iir_drive_sample_cl;
                s.iir_drive_sample_el = s.iir_drive_sample_el * (1.0 - iir_amount) + input_sample_l * iir_amount;
                input_sample_l -= s.iir_drive_sample_el;
                ata_lowpass_l = temp_sample - input_sample_l;
                temp_sample = input_sample_r;
                s.iir_drive_sample_ar = s.iir_drive_sample_ar * (1.0 - iir_amount) + input_sample_r * iir_amount;
                input_sample_r -= s.iir_drive_sample_ar;
                s.iir_drive_sample_cr = s.iir_drive_sample_cr * (1.0 - iir_amount) + input_sample_r * iir_amount;
                input_sample_r -= s.iir_drive_sample_cr;
                s.iir_drive_sample_er = s.iir_drive_sample_er * (1.0 - iir_amount) + input_sample_r * iir_amount;
                input_sample_r -= s.iir_drive_sample_er;
                ata_lowpass_r = temp_sample - input_sample_r;
            } else {
                temp_sample = input_sample_l;
                s.iir_drive_sample_bl = s.iir_drive_sample_bl * (1.0 - iir_amount) + input_sample_l * iir_amount;
                input_sample_l -= s.iir_drive_sample_bl;
                s.iir_drive_sample_dl = s.iir_drive_sample_dl * (1.0 - iir_amount) + input_sample_l * iir_amount;
                input_sample_l -= s.iir_drive_sample_dl;
                s.iir_drive_sample_fl = s.iir_drive_sample_fl * (1.0 - iir_amount) + input_sample_l * iir_amount;
                input_sample_l -= s.iir_drive_sample_fl;
                ata_lowpass_l = temp_sample - input_sample_l;
                temp_sample = input_sample_r;
                s.iir_drive_sample_br = s.iir_drive_sample_br * (1.0 - iir_amount) + input_sample_r * iir_amount;
                input_sample_r -= s.iir_drive_sample_br;
                s.iir_drive_sample_dr = s.iir_drive_sample_dr * (1.0 - iir_amount) + input_sample_r * iir_amount;
                input_sample_r -= s.iir_drive_sample_dr;
                s.iir_drive_sample_fr = s.iir_drive_sample_fr * (1.0 - iir_amount) + input_sample_r * iir_amount;
                input_sample_r -= s.iir_drive_sample_fr;
                ata_lowpass_r = temp_sample - input_sample_r;
            }
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
            out = driveone;
            while out > glitch {
                out -= glitch;
                input_sample_l -= input_sample_l * (input_sample_l.abs() * glitch) * (input_sample_l.abs() * glitch);
                input_sample_l *= 1.0 + glitch;
            }
            input_sample_l -= input_sample_l * (input_sample_l.abs() * out) * (input_sample_l.abs() * out);
            input_sample_l *= 1.0 + out;
            out = driveone;
            while out > glitch {
                out -= glitch;
                input_sample_r -= input_sample_r * (input_sample_r.abs() * glitch) * (input_sample_r.abs() * glitch);
                input_sample_r *= 1.0 + glitch;
            }
            input_sample_r -= input_sample_r * (input_sample_r.abs() * out) * (input_sample_r.abs() * out);
            input_sample_r *= 1.0 + out;
            ata_lowpass = (ata_lowpass_l + ata_lowpass_r) / 2.0;
            if ata_lowpass > 0.0 {
                if s.was_negative {
                    s.sub_octave = !s.sub_octave;
                }
                s.was_negative = false;
            } else {
                s.was_negative = true;
            }
            randy = s.fpd_l as f64 / u32::MAX as f64 * fuzz;
            invrandy = 1.0 - randy;
            randy /= 2.0;
            s.iir_sample_a = s.iir_sample_a * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_a;
            s.iir_sample_b = s.iir_sample_b * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_b;
            s.iir_sample_c = s.iir_sample_c * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_c;
            s.iir_sample_d = s.iir_sample_d * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_d;
            s.iir_sample_e = s.iir_sample_e * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_e;
            s.iir_sample_f = s.iir_sample_f * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_f;
            s.iir_sample_g = s.iir_sample_g * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_g;
            s.iir_sample_h = s.iir_sample_h * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_h;
            s.iir_sample_i = s.iir_sample_i * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_i;
            s.iir_sample_j = s.iir_sample_j * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_j;
            s.iir_sample_k = s.iir_sample_k * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_k;
            s.iir_sample_l = s.iir_sample_l * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_l;
            s.iir_sample_m = s.iir_sample_m * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_m;
            s.iir_sample_n = s.iir_sample_n * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_n;
            s.iir_sample_o = s.iir_sample_o * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_o;
            s.iir_sample_p = s.iir_sample_p * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_p;
            s.iir_sample_q = s.iir_sample_q * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_q;
            s.iir_sample_r = s.iir_sample_r * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_r;
            s.iir_sample_s = s.iir_sample_s * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_s;
            s.iir_sample_t = s.iir_sample_t * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_t;
            s.iir_sample_u = s.iir_sample_u * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_u;
            s.iir_sample_v = s.iir_sample_v * alt_bmount + ata_lowpass * iir_bmount;
            ata_lowpass -= s.iir_sample_v;
            match s.bflip {
                1 => {
                    s.iir_head_bump_a += ata_lowpass * bass_gain;
                    s.iir_head_bump_a -= s.iir_head_bump_a * s.iir_head_bump_a * s.iir_head_bump_a * head_bump_freq;
                    s.iir_head_bump_a = invrandy * s.iir_head_bump_a + randy * s.iir_head_bump_b + randy * s.iir_head_bump_c;
                    if s.iir_head_bump_a > 0.0 {
                        s.iir_head_bump_a -= clamp;
                    }
                    if s.iir_head_bump_a < 0.0 {
                        s.iir_head_bump_a += clamp;
                    }
                    head_bump = s.iir_head_bump_a;
                }
                2 => {
                    s.iir_head_bump_b += ata_lowpass * bass_gain;
                    s.iir_head_bump_b -= s.iir_head_bump_b * s.iir_head_bump_b * s.iir_head_bump_b * head_bump_freq;
                    s.iir_head_bump_b = randy * s.iir_head_bump_a + invrandy * s.iir_head_bump_b + randy * s.iir_head_bump_c;
                    if s.iir_head_bump_b > 0.0 {
                        s.iir_head_bump_b -= clamp;
                    }
                    if s.iir_head_bump_b < 0.0 {
                        s.iir_head_bump_b += clamp;
                    }
                    head_bump = s.iir_head_bump_b;
                }
                3 => {
                    s.iir_head_bump_c += ata_lowpass * bass_gain;
                    s.iir_head_bump_c -= s.iir_head_bump_c * s.iir_head_bump_c * s.iir_head_bump_c * head_bump_freq;
                    s.iir_head_bump_c = randy * s.iir_head_bump_a + randy * s.iir_head_bump_b + invrandy * s.iir_head_bump_c;
                    if s.iir_head_bump_c > 0.0 {
                        s.iir_head_bump_c -= clamp;
                    }
                    if s.iir_head_bump_c < 0.0 {
                        s.iir_head_bump_c += clamp;
                    }
                    head_bump = s.iir_head_bump_c;
                }
                _ => {}
            }
            s.iir_sample_w = s.iir_sample_w * alt_bmount + head_bump * iir_bmount;
            head_bump -= s.iir_sample_w;
            s.iir_sample_x = s.iir_sample_x * alt_bmount + head_bump * iir_bmount;
            head_bump -= s.iir_sample_x;
            sub_bump = head_bump;
            s.iir_sample_y = s.iir_sample_y * alt_cmount + sub_bump * iir_cmount;
            sub_bump -= s.iir_sample_y;
            sub_bump = sub_bump.abs();
            if s.sub_octave as i32 == false as i32 {
                sub_bump = -sub_bump;
            }
            match s.bflip {
                1 => {
                    s.iir_sub_bump_a += sub_bump * sub_gain;
                    s.iir_sub_bump_a -= s.iir_sub_bump_a * s.iir_sub_bump_a * s.iir_sub_bump_a * sub_bump_freq;
                    s.iir_sub_bump_a = invrandy * s.iir_sub_bump_a + randy * s.iir_sub_bump_b + randy * s.iir_sub_bump_c;
                    if s.iir_sub_bump_a > 0.0 {
                        s.iir_sub_bump_a -= clamp;
                    }
                    if s.iir_sub_bump_a < 0.0 {
                        s.iir_sub_bump_a += clamp;
                    }
                    sub_bump = s.iir_sub_bump_a;
                }
                2 => {
                    s.iir_sub_bump_b += sub_bump * sub_gain;
                    s.iir_sub_bump_b -= s.iir_sub_bump_b * s.iir_sub_bump_b * s.iir_sub_bump_b * sub_bump_freq;
                    s.iir_sub_bump_b = randy * s.iir_sub_bump_a + invrandy * s.iir_sub_bump_b + randy * s.iir_sub_bump_c;
                    if s.iir_sub_bump_b > 0.0 {
                        s.iir_sub_bump_b -= clamp;
                    }
                    if s.iir_sub_bump_b < 0.0 {
                        s.iir_sub_bump_b += clamp;
                    }
                    sub_bump = s.iir_sub_bump_b;
                }
                3 => {
                    s.iir_sub_bump_c += sub_bump * sub_gain;
                    s.iir_sub_bump_c -= s.iir_sub_bump_c * s.iir_sub_bump_c * s.iir_sub_bump_c * sub_bump_freq;
                    s.iir_sub_bump_c = randy * s.iir_sub_bump_a + randy * s.iir_sub_bump_b + invrandy * s.iir_sub_bump_c;
                    if s.iir_sub_bump_c > 0.0 {
                        s.iir_sub_bump_c -= clamp;
                    }
                    if s.iir_sub_bump_c < 0.0 {
                        s.iir_sub_bump_c += clamp;
                    }
                    sub_bump = s.iir_sub_bump_c;
                }
                _ => {}
            }
            s.iir_sample_z = s.iir_sample_z * alt_cmount + sub_bump * iir_cmount;
            sub_bump -= s.iir_sample_z;
            input_sample_l *= driveoutput;
            input_sample_r *= driveoutput;
            input_sample_l += (head_bump + s.last_head_bump) * bass_out_gain;
            input_sample_l += (sub_bump + s.last_sub_bump) * sub_out_gain;
            input_sample_r += (head_bump + s.last_head_bump) * bass_out_gain;
            input_sample_r += (sub_bump + s.last_sub_bump) * sub_out_gain;
            s.last_head_bump = head_bump;
            s.last_sub_bump = sub_bump;
            if wet != 1.0 {
                input_sample_l = input_sample_l * wet + dry_sample_l * (1.0 - wet);
                input_sample_r = input_sample_r * wet + dry_sample_r * (1.0 - wet);
            }
            s.flip = !s.flip;
            s.bflip += 1;
            if s.bflip < 1 || s.bflip > 3 {
                s.bflip = 1;
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
