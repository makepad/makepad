// Ported from DubSub (DubSubProc.cpp, DubSub.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Dub Sub (sub-effect id 60): the upstream float processing path, ported as written
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
pub struct DubSub {
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
    was_negative_l: bool,
    sub_octave_l: bool,
    was_negative_r: bool,
    sub_octave_r: bool,
    iir_head_bump_al: f64,
    iir_head_bump_bl: f64,
    iir_head_bump_cl: f64,
    iir_head_bump_ar: f64,
    iir_head_bump_br: f64,
    iir_head_bump_cr: f64,
    iir_sub_bump_al: f64,
    iir_sub_bump_bl: f64,
    iir_sub_bump_cl: f64,
    iir_sub_bump_ar: f64,
    iir_sub_bump_br: f64,
    iir_sub_bump_cr: f64,
    last_head_bump_l: f64,
    last_sub_bump_l: f64,
    last_head_bump_r: f64,
    last_sub_bump_r: f64,
    iir_sample_al: f64,
    iir_sample_bl: f64,
    iir_sample_cl: f64,
    iir_sample_dl: f64,
    iir_sample_el: f64,
    iir_sample_fl: f64,
    iir_sample_gl: f64,
    iir_sample_hl: f64,
    iir_sample_il: f64,
    iir_sample_jl: f64,
    iir_sample_kl: f64,
    iir_sample_ll: f64,
    iir_sample_ml: f64,
    iir_sample_nl: f64,
    iir_sample_ol: f64,
    iir_sample_pl: f64,
    iir_sample_ql: f64,
    iir_sample_rl: f64,
    iir_sample_sl: f64,
    iir_sample_tl: f64,
    iir_sample_ul: f64,
    iir_sample_vl: f64,
    iir_sample_wl: f64,
    iir_sample_xl: f64,
    iir_sample_yl: f64,
    iir_sample_zl: f64,
    iir_sample_ar: f64,
    iir_sample_br: f64,
    iir_sample_cr: f64,
    iir_sample_dr: f64,
    iir_sample_er: f64,
    iir_sample_fr: f64,
    iir_sample_gr: f64,
    iir_sample_hr: f64,
    iir_sample_ir: f64,
    iir_sample_jr: f64,
    iir_sample_kr: f64,
    iir_sample_lr: f64,
    iir_sample_mr: f64,
    iir_sample_nr: f64,
    iir_sample_or: f64,
    iir_sample_pr: f64,
    iir_sample_qr: f64,
    iir_sample_rr: f64,
    iir_sample_sr: f64,
    iir_sample_tr: f64,
    iir_sample_ur: f64,
    iir_sample_vr: f64,
    iir_sample_wr: f64,
    iir_sample_xr: f64,
    iir_sample_yr: f64,
    iir_sample_zr: f64,
    osc_gate_l: f64,
    osc_gate_r: f64,
    fpd_l: u32,
    fpd_r: u32,
    /// upstream: a function-local static (process-wide there; per instance here)
    noisesource_l: i32,
    /// upstream: a function-local static (process-wide there; per instance here)
    noisesource_r: i32,
}

impl DubSub {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<DubSub> {
        let mut s = Box::new(DubSub {
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
            was_negative_l: false,
            sub_octave_l: false,
            was_negative_r: false,
            sub_octave_r: false,
            iir_head_bump_al: 0.0,
            iir_head_bump_bl: 0.0,
            iir_head_bump_cl: 0.0,
            iir_head_bump_ar: 0.0,
            iir_head_bump_br: 0.0,
            iir_head_bump_cr: 0.0,
            iir_sub_bump_al: 0.0,
            iir_sub_bump_bl: 0.0,
            iir_sub_bump_cl: 0.0,
            iir_sub_bump_ar: 0.0,
            iir_sub_bump_br: 0.0,
            iir_sub_bump_cr: 0.0,
            last_head_bump_l: 0.0,
            last_sub_bump_l: 0.0,
            last_head_bump_r: 0.0,
            last_sub_bump_r: 0.0,
            iir_sample_al: 0.0,
            iir_sample_bl: 0.0,
            iir_sample_cl: 0.0,
            iir_sample_dl: 0.0,
            iir_sample_el: 0.0,
            iir_sample_fl: 0.0,
            iir_sample_gl: 0.0,
            iir_sample_hl: 0.0,
            iir_sample_il: 0.0,
            iir_sample_jl: 0.0,
            iir_sample_kl: 0.0,
            iir_sample_ll: 0.0,
            iir_sample_ml: 0.0,
            iir_sample_nl: 0.0,
            iir_sample_ol: 0.0,
            iir_sample_pl: 0.0,
            iir_sample_ql: 0.0,
            iir_sample_rl: 0.0,
            iir_sample_sl: 0.0,
            iir_sample_tl: 0.0,
            iir_sample_ul: 0.0,
            iir_sample_vl: 0.0,
            iir_sample_wl: 0.0,
            iir_sample_xl: 0.0,
            iir_sample_yl: 0.0,
            iir_sample_zl: 0.0,
            iir_sample_ar: 0.0,
            iir_sample_br: 0.0,
            iir_sample_cr: 0.0,
            iir_sample_dr: 0.0,
            iir_sample_er: 0.0,
            iir_sample_fr: 0.0,
            iir_sample_gr: 0.0,
            iir_sample_hr: 0.0,
            iir_sample_ir: 0.0,
            iir_sample_jr: 0.0,
            iir_sample_kr: 0.0,
            iir_sample_lr: 0.0,
            iir_sample_mr: 0.0,
            iir_sample_nr: 0.0,
            iir_sample_or: 0.0,
            iir_sample_pr: 0.0,
            iir_sample_qr: 0.0,
            iir_sample_rr: 0.0,
            iir_sample_sr: 0.0,
            iir_sample_tr: 0.0,
            iir_sample_ur: 0.0,
            iir_sample_vr: 0.0,
            iir_sample_wr: 0.0,
            iir_sample_xr: 0.0,
            iir_sample_yr: 0.0,
            iir_sample_zr: 0.0,
            osc_gate_l: 0.0,
            osc_gate_r: 0.0,
            fpd_l: 0,
            fpd_r: 0,
            noisesource_l: 0,
            noisesource_r: 850010,
        });
        s.osc_gate_l = 1.0;
        s.osc_gate_r = 1.0;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for DubSub {
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
        let mut randy_l: f64 = 0.0;
        let mut invrandy_l: f64 = 0.0;
        let mut randy_r: f64 = 0.0;
        let mut invrandy_r: f64 = 0.0;
        let mut head_bump_l: f64 = 0.0;
        let mut head_bump_r: f64 = 0.0;
        let mut bass_gain: f64 = s.p[3] as f64 * 0.1;
        let mut head_bump_freq: f64 = (s.p[4] as f64 * 0.1 + 0.0001) / overallscale;
        let mut iir_bmount: f64 = head_bump_freq / 44.1;
        let mut alt_bmount: f64 = 1.0 - iir_bmount;
        let mut bass_out_gain: f64 = s.p[5] as f64 * 2.0 - 1.0;
        let mut sub_bump_l: f64 = 0.0;
        let mut sub_bump_r: f64 = 0.0;
        let mut sub_gain: f64 = s.p[6] as f64 * 0.1;
        let mut sub_bump_freq: f64 = (s.p[7] as f64 * 0.1 + 0.0001) / overallscale;
        let mut iir_cmount: f64 = sub_bump_freq / 44.1;
        let mut alt_cmount: f64 = 1.0 - iir_cmount;
        let mut sub_out_gain: f64 = s.p[8] as f64 * 2.0 - 1.0;
        let mut clamp_l: f64 = 0.0;
        let mut clamp_r: f64 = 0.0;
        let mut out: f64 = 0.0;
        let mut fuzz: f64 = 0.111;
        let mut wet: f64 = s.p[9] as f64;
        let mut glitch: f64 = 0.60;
        let mut temp_sample_l: f64 = 0.0;
        let mut temp_sample_r: f64 = 0.0;
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
            s.osc_gate_l += (input_sample_l * 10.0).abs();
            s.osc_gate_l -= 0.001;
            if s.osc_gate_l > 1.0 {
                s.osc_gate_l = 1.0;
            }
            if s.osc_gate_l < 0.0 {
                s.osc_gate_l = 0.0;
            }
            s.osc_gate_r += (input_sample_r * 10.0).abs();
            s.osc_gate_r -= 0.001;
            if s.osc_gate_r > 1.0 {
                s.osc_gate_r = 1.0;
            }
            if s.osc_gate_r < 0.0 {
                s.osc_gate_r = 0.0;
            }
            clamp_l = 1.0 - s.osc_gate_l;
            clamp_l *= 0.00001;
            clamp_r = 1.0 - s.osc_gate_r;
            clamp_r *= 0.00001;
            if s.flip {
                temp_sample_l = input_sample_l;
                s.iir_drive_sample_al = s.iir_drive_sample_al * (1.0 - iir_amount) + input_sample_l * iir_amount;
                input_sample_l -= s.iir_drive_sample_al;
                s.iir_drive_sample_cl = s.iir_drive_sample_cl * (1.0 - iir_amount) + input_sample_l * iir_amount;
                input_sample_l -= s.iir_drive_sample_cl;
                s.iir_drive_sample_el = s.iir_drive_sample_el * (1.0 - iir_amount) + input_sample_l * iir_amount;
                input_sample_l -= s.iir_drive_sample_el;
                ata_lowpass_l = temp_sample_l - input_sample_l;
                temp_sample_r = input_sample_r;
                s.iir_drive_sample_ar = s.iir_drive_sample_ar * (1.0 - iir_amount) + input_sample_r * iir_amount;
                input_sample_r -= s.iir_drive_sample_ar;
                s.iir_drive_sample_cr = s.iir_drive_sample_cr * (1.0 - iir_amount) + input_sample_r * iir_amount;
                input_sample_r -= s.iir_drive_sample_cr;
                s.iir_drive_sample_er = s.iir_drive_sample_er * (1.0 - iir_amount) + input_sample_r * iir_amount;
                input_sample_r -= s.iir_drive_sample_er;
                ata_lowpass_r = temp_sample_r - input_sample_r;
            } else {
                temp_sample_l = input_sample_l;
                s.iir_drive_sample_bl = s.iir_drive_sample_bl * (1.0 - iir_amount) + input_sample_l * iir_amount;
                input_sample_l -= s.iir_drive_sample_bl;
                s.iir_drive_sample_dl = s.iir_drive_sample_dl * (1.0 - iir_amount) + input_sample_l * iir_amount;
                input_sample_l -= s.iir_drive_sample_dl;
                s.iir_drive_sample_fl = s.iir_drive_sample_fl * (1.0 - iir_amount) + input_sample_l * iir_amount;
                input_sample_l -= s.iir_drive_sample_fl;
                ata_lowpass_l = temp_sample_l - input_sample_l;
                temp_sample_r = input_sample_r;
                s.iir_drive_sample_br = s.iir_drive_sample_br * (1.0 - iir_amount) + input_sample_r * iir_amount;
                input_sample_r -= s.iir_drive_sample_br;
                s.iir_drive_sample_dr = s.iir_drive_sample_dr * (1.0 - iir_amount) + input_sample_r * iir_amount;
                input_sample_r -= s.iir_drive_sample_dr;
                s.iir_drive_sample_fr = s.iir_drive_sample_fr * (1.0 - iir_amount) + input_sample_r * iir_amount;
                input_sample_r -= s.iir_drive_sample_fr;
                ata_lowpass_r = temp_sample_r - input_sample_r;
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
                input_sample_r -= input_sample_r * (input_sample_r.abs() * glitch) * (input_sample_r.abs() * glitch);
                input_sample_l *= 1.0 + glitch;
                input_sample_r *= 1.0 + glitch;
            }
            input_sample_l -= input_sample_l * (input_sample_l.abs() * out) * (input_sample_l.abs() * out);
            input_sample_r -= input_sample_r * (input_sample_r.abs() * out) * (input_sample_r.abs() * out);
            input_sample_l *= 1.0 + out;
            input_sample_r *= 1.0 + out;
            if ata_lowpass_l > 0.0 {
                if s.was_negative_l {
                    s.sub_octave_l = !s.sub_octave_l;
                }
                s.was_negative_l = false;
            } else {
                s.was_negative_l = true;
            }
            if ata_lowpass_r > 0.0 {
                if s.was_negative_r {
                    s.sub_octave_r = !s.sub_octave_r;
                }
                s.was_negative_r = false;
            } else {
                s.was_negative_r = true;
            }
            randy_l = s.fpd_l as f64 / u32::MAX as f64 * fuzz;
            invrandy_l = 1.0 - randy_l;
            randy_l /= 2.0;
            randy_r = s.fpd_r as f64 / u32::MAX as f64 * fuzz;
            invrandy_r = 1.0 - randy_r;
            randy_r /= 2.0;
            s.iir_sample_al = s.iir_sample_al * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_al;
            s.iir_sample_bl = s.iir_sample_bl * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_bl;
            s.iir_sample_cl = s.iir_sample_cl * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_cl;
            s.iir_sample_dl = s.iir_sample_dl * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_dl;
            s.iir_sample_el = s.iir_sample_el * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_el;
            s.iir_sample_fl = s.iir_sample_fl * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_fl;
            s.iir_sample_gl = s.iir_sample_gl * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_gl;
            s.iir_sample_hl = s.iir_sample_hl * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_hl;
            s.iir_sample_il = s.iir_sample_il * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_il;
            s.iir_sample_jl = s.iir_sample_jl * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_jl;
            s.iir_sample_kl = s.iir_sample_kl * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_kl;
            s.iir_sample_ll = s.iir_sample_ll * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_ll;
            s.iir_sample_ml = s.iir_sample_ml * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_ml;
            s.iir_sample_nl = s.iir_sample_nl * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_nl;
            s.iir_sample_ol = s.iir_sample_ol * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_ol;
            s.iir_sample_pl = s.iir_sample_pl * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_pl;
            s.iir_sample_ql = s.iir_sample_ql * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_ql;
            s.iir_sample_rl = s.iir_sample_rl * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_rl;
            s.iir_sample_sl = s.iir_sample_sl * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_sl;
            s.iir_sample_tl = s.iir_sample_tl * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_tl;
            s.iir_sample_ul = s.iir_sample_ul * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_ul;
            s.iir_sample_vl = s.iir_sample_vl * alt_bmount + ata_lowpass_l * iir_bmount;
            ata_lowpass_l -= s.iir_sample_vl;
            s.iir_sample_ar = s.iir_sample_ar * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_ar;
            s.iir_sample_br = s.iir_sample_br * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_br;
            s.iir_sample_cr = s.iir_sample_cr * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_cr;
            s.iir_sample_dr = s.iir_sample_dr * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_dr;
            s.iir_sample_er = s.iir_sample_er * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_er;
            s.iir_sample_fr = s.iir_sample_fr * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_fr;
            s.iir_sample_gr = s.iir_sample_gr * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_gr;
            s.iir_sample_hr = s.iir_sample_hr * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_hr;
            s.iir_sample_ir = s.iir_sample_ir * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_ir;
            s.iir_sample_jr = s.iir_sample_jr * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_jr;
            s.iir_sample_kr = s.iir_sample_kr * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_kr;
            s.iir_sample_lr = s.iir_sample_lr * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_lr;
            s.iir_sample_mr = s.iir_sample_mr * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_mr;
            s.iir_sample_nr = s.iir_sample_nr * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_nr;
            s.iir_sample_or = s.iir_sample_or * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_or;
            s.iir_sample_pr = s.iir_sample_pr * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_pr;
            s.iir_sample_qr = s.iir_sample_qr * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_qr;
            s.iir_sample_rr = s.iir_sample_rr * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_rr;
            s.iir_sample_sr = s.iir_sample_sr * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_sr;
            s.iir_sample_tr = s.iir_sample_tr * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_tr;
            s.iir_sample_ur = s.iir_sample_ur * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_ur;
            s.iir_sample_vr = s.iir_sample_vr * alt_bmount + ata_lowpass_r * iir_bmount;
            ata_lowpass_r -= s.iir_sample_vr;
            match s.bflip {
                1 => {
                    s.iir_head_bump_al += ata_lowpass_l * bass_gain;
                    s.iir_head_bump_al -= s.iir_head_bump_al * s.iir_head_bump_al * s.iir_head_bump_al * head_bump_freq;
                    s.iir_head_bump_al = invrandy_l * s.iir_head_bump_al + randy_l * s.iir_head_bump_bl + randy_l * s.iir_head_bump_cl;
                    if s.iir_head_bump_al > 0.0 {
                        s.iir_head_bump_al -= clamp_l;
                    }
                    if s.iir_head_bump_al < 0.0 {
                        s.iir_head_bump_al += clamp_l;
                    }
                    head_bump_l = s.iir_head_bump_al;
                    s.iir_head_bump_ar += ata_lowpass_r * bass_gain;
                    s.iir_head_bump_ar -= s.iir_head_bump_ar * s.iir_head_bump_ar * s.iir_head_bump_ar * head_bump_freq;
                    s.iir_head_bump_ar = invrandy_r * s.iir_head_bump_ar + randy_r * s.iir_head_bump_br + randy_r * s.iir_head_bump_cr;
                    if s.iir_head_bump_ar > 0.0 {
                        s.iir_head_bump_ar -= clamp_r;
                    }
                    if s.iir_head_bump_ar < 0.0 {
                        s.iir_head_bump_ar += clamp_r;
                    }
                    head_bump_r = s.iir_head_bump_ar;
                }
                2 => {
                    s.iir_head_bump_bl += ata_lowpass_l * bass_gain;
                    s.iir_head_bump_bl -= s.iir_head_bump_bl * s.iir_head_bump_bl * s.iir_head_bump_bl * head_bump_freq;
                    s.iir_head_bump_bl = randy_l * s.iir_head_bump_al + invrandy_l * s.iir_head_bump_bl + randy_l * s.iir_head_bump_cl;
                    if s.iir_head_bump_bl > 0.0 {
                        s.iir_head_bump_bl -= clamp_l;
                    }
                    if s.iir_head_bump_bl < 0.0 {
                        s.iir_head_bump_bl += clamp_l;
                    }
                    head_bump_l = s.iir_head_bump_bl;
                    s.iir_head_bump_br += ata_lowpass_r * bass_gain;
                    s.iir_head_bump_br -= s.iir_head_bump_br * s.iir_head_bump_br * s.iir_head_bump_br * head_bump_freq;
                    s.iir_head_bump_br = randy_r * s.iir_head_bump_ar + invrandy_r * s.iir_head_bump_br + randy_r * s.iir_head_bump_cr;
                    if s.iir_head_bump_br > 0.0 {
                        s.iir_head_bump_br -= clamp_r;
                    }
                    if s.iir_head_bump_br < 0.0 {
                        s.iir_head_bump_br += clamp_r;
                    }
                    head_bump_r = s.iir_head_bump_br;
                }
                3 => {
                    s.iir_head_bump_cl += ata_lowpass_l * bass_gain;
                    s.iir_head_bump_cl -= s.iir_head_bump_cl * s.iir_head_bump_cl * s.iir_head_bump_cl * head_bump_freq;
                    s.iir_head_bump_cl = randy_l * s.iir_head_bump_al + randy_l * s.iir_head_bump_bl + invrandy_l * s.iir_head_bump_cl;
                    if s.iir_head_bump_cl > 0.0 {
                        s.iir_head_bump_cl -= clamp_l;
                    }
                    if s.iir_head_bump_cl < 0.0 {
                        s.iir_head_bump_cl += clamp_l;
                    }
                    head_bump_l = s.iir_head_bump_cl;
                    s.iir_head_bump_cr += ata_lowpass_r * bass_gain;
                    s.iir_head_bump_cr -= s.iir_head_bump_cr * s.iir_head_bump_cr * s.iir_head_bump_cr * head_bump_freq;
                    s.iir_head_bump_cr = randy_r * s.iir_head_bump_ar + randy_r * s.iir_head_bump_br + invrandy_r * s.iir_head_bump_cr;
                    if s.iir_head_bump_cr > 0.0 {
                        s.iir_head_bump_cr -= clamp_r;
                    }
                    if s.iir_head_bump_cr < 0.0 {
                        s.iir_head_bump_cr += clamp_r;
                    }
                    head_bump_r = s.iir_head_bump_cr;
                }
                _ => {}
            }
            s.iir_sample_wl = s.iir_sample_wl * alt_bmount + head_bump_l * iir_bmount;
            head_bump_l -= s.iir_sample_wl;
            s.iir_sample_xl = s.iir_sample_xl * alt_bmount + head_bump_l * iir_bmount;
            head_bump_l -= s.iir_sample_xl;
            s.iir_sample_wr = s.iir_sample_wr * alt_bmount + head_bump_r * iir_bmount;
            head_bump_r -= s.iir_sample_wr;
            s.iir_sample_xr = s.iir_sample_xr * alt_bmount + head_bump_r * iir_bmount;
            head_bump_r -= s.iir_sample_xr;
            sub_bump_l = head_bump_l;
            s.iir_sample_yl = s.iir_sample_yl * alt_cmount + sub_bump_l * iir_cmount;
            sub_bump_l -= s.iir_sample_yl;
            sub_bump_r = head_bump_r;
            s.iir_sample_yr = s.iir_sample_yr * alt_cmount + sub_bump_r * iir_cmount;
            sub_bump_r -= s.iir_sample_yr;
            sub_bump_l = sub_bump_l.abs();
            if s.sub_octave_l as i32 == false as i32 {
                sub_bump_l = -sub_bump_l;
            }
            sub_bump_r = sub_bump_r.abs();
            if s.sub_octave_r as i32 == false as i32 {
                sub_bump_r = -sub_bump_r;
            }
            match s.bflip {
                1 => {
                    s.iir_sub_bump_al += sub_bump_l * sub_gain;
                    s.iir_sub_bump_al -= s.iir_sub_bump_al * s.iir_sub_bump_al * s.iir_sub_bump_al * sub_bump_freq;
                    s.iir_sub_bump_al = invrandy_l * s.iir_sub_bump_al + randy_l * s.iir_sub_bump_bl + randy_l * s.iir_sub_bump_cl;
                    if s.iir_sub_bump_al > 0.0 {
                        s.iir_sub_bump_al -= clamp_l;
                    }
                    if s.iir_sub_bump_al < 0.0 {
                        s.iir_sub_bump_al += clamp_l;
                    }
                    sub_bump_l = s.iir_sub_bump_al;
                    s.iir_sub_bump_ar += sub_bump_r * sub_gain;
                    s.iir_sub_bump_ar -= s.iir_sub_bump_ar * s.iir_sub_bump_ar * s.iir_sub_bump_ar * sub_bump_freq;
                    s.iir_sub_bump_ar = invrandy_r * s.iir_sub_bump_ar + randy_r * s.iir_sub_bump_br + randy_r * s.iir_sub_bump_cr;
                    if s.iir_sub_bump_ar > 0.0 {
                        s.iir_sub_bump_ar -= clamp_r;
                    }
                    if s.iir_sub_bump_ar < 0.0 {
                        s.iir_sub_bump_ar += clamp_r;
                    }
                    sub_bump_r = s.iir_sub_bump_ar;
                }
                2 => {
                    s.iir_sub_bump_bl += sub_bump_l * sub_gain;
                    s.iir_sub_bump_bl -= s.iir_sub_bump_bl * s.iir_sub_bump_bl * s.iir_sub_bump_bl * sub_bump_freq;
                    s.iir_sub_bump_bl = randy_l * s.iir_sub_bump_al + invrandy_l * s.iir_sub_bump_bl + randy_l * s.iir_sub_bump_cl;
                    if s.iir_sub_bump_bl > 0.0 {
                        s.iir_sub_bump_bl -= clamp_l;
                    }
                    if s.iir_sub_bump_bl < 0.0 {
                        s.iir_sub_bump_bl += clamp_l;
                    }
                    sub_bump_l = s.iir_sub_bump_bl;
                    s.iir_sub_bump_br += sub_bump_r * sub_gain;
                    s.iir_sub_bump_br -= s.iir_sub_bump_br * s.iir_sub_bump_br * s.iir_sub_bump_br * sub_bump_freq;
                    s.iir_sub_bump_br = randy_r * s.iir_sub_bump_ar + invrandy_r * s.iir_sub_bump_br + randy_r * s.iir_sub_bump_cr;
                    if s.iir_sub_bump_br > 0.0 {
                        s.iir_sub_bump_br -= clamp_r;
                    }
                    if s.iir_sub_bump_br < 0.0 {
                        s.iir_sub_bump_br += clamp_r;
                    }
                    sub_bump_r = s.iir_sub_bump_br;
                }
                3 => {
                    s.iir_sub_bump_cl += sub_bump_l * sub_gain;
                    s.iir_sub_bump_cl -= s.iir_sub_bump_cl * s.iir_sub_bump_cl * s.iir_sub_bump_cl * sub_bump_freq;
                    s.iir_sub_bump_cl = randy_l * s.iir_sub_bump_al + randy_l * s.iir_sub_bump_bl + invrandy_l * s.iir_sub_bump_cl;
                    if s.iir_sub_bump_cl > 0.0 {
                        s.iir_sub_bump_cl -= clamp_l;
                    }
                    if s.iir_sub_bump_cl < 0.0 {
                        s.iir_sub_bump_cl += clamp_l;
                    }
                    sub_bump_l = s.iir_sub_bump_cl;
                    s.iir_sub_bump_cr += sub_bump_r * sub_gain;
                    s.iir_sub_bump_cr -= s.iir_sub_bump_cr * s.iir_sub_bump_cr * s.iir_sub_bump_cr * sub_bump_freq;
                    s.iir_sub_bump_cr = randy_r * s.iir_sub_bump_ar + randy_r * s.iir_sub_bump_br + invrandy_r * s.iir_sub_bump_cr;
                    if s.iir_sub_bump_cr > 0.0 {
                        s.iir_sub_bump_cr -= clamp_r;
                    }
                    if s.iir_sub_bump_cr < 0.0 {
                        s.iir_sub_bump_cr += clamp_r;
                    }
                    sub_bump_r = s.iir_sub_bump_cr;
                }
                _ => {}
            }
            s.iir_sample_zl = s.iir_sample_zl * alt_cmount + sub_bump_l * iir_cmount;
            sub_bump_l -= s.iir_sample_zl;
            s.iir_sample_zr = s.iir_sample_zr * alt_cmount + sub_bump_r * iir_cmount;
            sub_bump_r -= s.iir_sample_zr;
            input_sample_l *= driveoutput;
            input_sample_r *= driveoutput;
            input_sample_l += (head_bump_l + s.last_head_bump_l) * bass_out_gain;
            input_sample_l += (sub_bump_l + s.last_sub_bump_l) * sub_out_gain;
            input_sample_r += (head_bump_r + s.last_head_bump_r) * bass_out_gain;
            input_sample_r += (sub_bump_r + s.last_sub_bump_r) * sub_out_gain;
            s.last_head_bump_l = head_bump_l;
            s.last_sub_bump_l = sub_bump_l;
            s.last_head_bump_r = head_bump_r;
            s.last_sub_bump_r = sub_bump_r;
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
