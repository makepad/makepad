// Ported from Deckwrecka (DeckwreckaProc.cpp, Deckwrecka.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Deck Wrecka (sub-effect id 16): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Wreck", 0.0),
];

#[allow(dead_code)]
pub struct Deckwrecka {
    p: [f32; 1],
    sr: f32,
    fpd_l: u32,
    fpd_r: u32,
    bflip: i32,
    iir_head_bump_al: f64,
    iir_head_bump_bl: f64,
    iir_head_bump_cl: f64,
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
    iir_head_bump_ar: f64,
    iir_head_bump_br: f64,
    iir_head_bump_cr: f64,
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
}

impl Deckwrecka {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Deckwrecka> {
        let mut s = Box::new(Deckwrecka {
            p: [0.0],
            sr: sr as f32,
            fpd_l: 0,
            fpd_r: 0,
            bflip: 0,
            iir_head_bump_al: 0.0,
            iir_head_bump_bl: 0.0,
            iir_head_bump_cl: 0.0,
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
            iir_head_bump_ar: 0.0,
            iir_head_bump_br: 0.0,
            iir_head_bump_cr: 0.0,
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
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s.bflip = 1;
        s
    }
}

impl Sub for Deckwrecka {
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
        let mut wreck: f64 = (s.p[0] as f64).powf(2.0);
        let mut randy_l: f64 = 0.0;
        let mut randy_r: f64 = 0.0;
        let mut head_bump_l: f64 = 0.0;
        let mut head_bump_r: f64 = 0.0;
        let mut head_bump_freq: f64 = 0.01 / overallscale;
        let mut iir_hpamount: f64 = 0.0000014 / overallscale;
        let mut alt_hpamount: f64 = 1.0 - iir_hpamount;
        let mut temp_sample: f64 = 0.0;
        let mut bridgerectifier: f64 = 0.0;
        let mut correction: f64 = 0.0;
        for smp in 0..l.len() {
            let mut input_sample_l: f64 = l[smp] as f64;
            let mut input_sample_r: f64 = r[smp] as f64;
            if input_sample_l.abs() < 1.18e-23 {
                input_sample_l = s.fpd_l as f64 * 1.18e-17;
            }
            if input_sample_r.abs() < 1.18e-23 {
                input_sample_r = s.fpd_r as f64 * 1.18e-17;
            }
            s.bflip += 1;
            if s.bflip < 1 || s.bflip > 3 {
                s.bflip = 1;
            }
            randy_l = s.fpd_l as f64 / u32::MAX as f64;
            s.fpd_l ^= s.fpd_l << 13;
            s.fpd_l ^= s.fpd_l >> 17;
            s.fpd_l ^= s.fpd_l << 5;
            randy_l += s.fpd_l as f64 / u32::MAX as f64;
            s.fpd_l ^= s.fpd_l << 13;
            s.fpd_l ^= s.fpd_l >> 17;
            s.fpd_l ^= s.fpd_l << 5;
            randy_l += s.fpd_l as f64 / u32::MAX as f64;
            s.fpd_l ^= s.fpd_l << 13;
            s.fpd_l ^= s.fpd_l >> 17;
            s.fpd_l ^= s.fpd_l << 5;
            randy_l += s.fpd_l as f64 / u32::MAX as f64;
            s.fpd_l ^= s.fpd_l << 13;
            s.fpd_l ^= s.fpd_l >> 17;
            s.fpd_l ^= s.fpd_l << 5;
            randy_l += s.fpd_l as f64 / u32::MAX as f64;
            s.fpd_l ^= s.fpd_l << 13;
            s.fpd_l ^= s.fpd_l >> 17;
            s.fpd_l ^= s.fpd_l << 5;
            randy_l += s.fpd_l as f64 / u32::MAX as f64;
            randy_l /= 6.0;
            randy_l *= wreck;
            randy_r = s.fpd_r as f64 / u32::MAX as f64;
            s.fpd_r ^= s.fpd_r << 13;
            s.fpd_r ^= s.fpd_r >> 17;
            s.fpd_r ^= s.fpd_r << 5;
            randy_r += s.fpd_r as f64 / u32::MAX as f64;
            s.fpd_r ^= s.fpd_r << 13;
            s.fpd_r ^= s.fpd_r >> 17;
            s.fpd_r ^= s.fpd_r << 5;
            randy_r += s.fpd_r as f64 / u32::MAX as f64;
            s.fpd_r ^= s.fpd_r << 13;
            s.fpd_r ^= s.fpd_r >> 17;
            s.fpd_r ^= s.fpd_r << 5;
            randy_r += s.fpd_r as f64 / u32::MAX as f64;
            s.fpd_r ^= s.fpd_r << 13;
            s.fpd_r ^= s.fpd_r >> 17;
            s.fpd_r ^= s.fpd_r << 5;
            randy_r += s.fpd_r as f64 / u32::MAX as f64;
            s.fpd_r ^= s.fpd_r << 13;
            s.fpd_r ^= s.fpd_r >> 17;
            s.fpd_r ^= s.fpd_r << 5;
            randy_r += s.fpd_r as f64 / u32::MAX as f64;
            randy_r /= 6.0;
            randy_r *= wreck;
            match s.bflip {
                1 => {
                    s.iir_head_bump_al += input_sample_l * randy_l;
                    s.iir_head_bump_al -= s.iir_head_bump_al * s.iir_head_bump_al * s.iir_head_bump_al * head_bump_freq;
                    s.iir_head_bump_ar += input_sample_r * randy_r;
                    s.iir_head_bump_ar -= s.iir_head_bump_ar * s.iir_head_bump_ar * s.iir_head_bump_ar * head_bump_freq;
                }
                2 => {
                    s.iir_head_bump_bl += input_sample_l * randy_l;
                    s.iir_head_bump_bl -= s.iir_head_bump_bl * s.iir_head_bump_bl * s.iir_head_bump_bl * head_bump_freq;
                    s.iir_head_bump_br += input_sample_r * randy_r;
                    s.iir_head_bump_br -= s.iir_head_bump_br * s.iir_head_bump_br * s.iir_head_bump_br * head_bump_freq;
                }
                3 => {
                    s.iir_head_bump_cl += input_sample_l * randy_l;
                    s.iir_head_bump_cl -= s.iir_head_bump_cl * s.iir_head_bump_cl * s.iir_head_bump_cl * head_bump_freq;
                    s.iir_head_bump_cr += input_sample_r * randy_r;
                    s.iir_head_bump_cr -= s.iir_head_bump_cr * s.iir_head_bump_cr * s.iir_head_bump_cr * head_bump_freq;
                }
                _ => {}
            }
            head_bump_l = s.iir_head_bump_al + s.iir_head_bump_bl + s.iir_head_bump_cl;
            head_bump_r = s.iir_head_bump_ar + s.iir_head_bump_br + s.iir_head_bump_cr;
            temp_sample = head_bump_l;
            s.iir_sample_al = s.iir_sample_al * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_al;
            correction = s.iir_sample_al;
            s.iir_sample_bl = s.iir_sample_bl * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_bl;
            correction += s.iir_sample_bl;
            s.iir_sample_cl = s.iir_sample_cl * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_cl;
            correction += s.iir_sample_cl;
            s.iir_sample_dl = s.iir_sample_dl * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_dl;
            correction += s.iir_sample_dl;
            s.iir_sample_el = s.iir_sample_el * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_el;
            correction += s.iir_sample_el;
            s.iir_sample_fl = s.iir_sample_fl * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_fl;
            correction += s.iir_sample_fl;
            s.iir_sample_gl = s.iir_sample_gl * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_gl;
            correction += s.iir_sample_gl;
            s.iir_sample_hl = s.iir_sample_hl * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_hl;
            correction += s.iir_sample_hl;
            s.iir_sample_il = s.iir_sample_il * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_il;
            correction += s.iir_sample_il;
            s.iir_sample_jl = s.iir_sample_jl * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_jl;
            correction += s.iir_sample_jl;
            s.iir_sample_kl = s.iir_sample_kl * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_kl;
            correction += s.iir_sample_kl;
            s.iir_sample_ll = s.iir_sample_ll * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_ll;
            correction += s.iir_sample_ll;
            s.iir_sample_ml = s.iir_sample_ml * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_ml;
            correction += s.iir_sample_ml;
            s.iir_sample_nl = s.iir_sample_nl * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_nl;
            correction += s.iir_sample_nl;
            s.iir_sample_ol = s.iir_sample_ol * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_ol;
            correction += s.iir_sample_ol;
            s.iir_sample_pl = s.iir_sample_pl * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_pl;
            correction += s.iir_sample_pl;
            s.iir_sample_ql = s.iir_sample_ql * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_ql;
            correction += s.iir_sample_ql;
            s.iir_sample_rl = s.iir_sample_rl * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_rl;
            correction += s.iir_sample_rl;
            s.iir_sample_sl = s.iir_sample_sl * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_sl;
            correction += s.iir_sample_sl;
            s.iir_sample_tl = s.iir_sample_tl * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_tl;
            correction += s.iir_sample_tl;
            s.iir_sample_ul = s.iir_sample_ul * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_ul;
            correction += s.iir_sample_ul;
            s.iir_sample_vl = s.iir_sample_vl * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_vl;
            correction += s.iir_sample_vl;
            s.iir_sample_wl = s.iir_sample_wl * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_wl;
            correction += s.iir_sample_wl;
            s.iir_sample_xl = s.iir_sample_xl * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_xl;
            correction += s.iir_sample_xl;
            s.iir_sample_yl = s.iir_sample_yl * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_yl;
            correction += s.iir_sample_yl;
            s.iir_sample_zl = s.iir_sample_zl * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_zl;
            correction += s.iir_sample_zl;
            head_bump_l -= correction;
            head_bump_l *= 1.0 - wreck / 2.0;
            temp_sample = head_bump_r;
            s.iir_sample_ar = s.iir_sample_ar * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_ar;
            correction = s.iir_sample_ar;
            s.iir_sample_br = s.iir_sample_br * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_br;
            correction += s.iir_sample_br;
            s.iir_sample_cr = s.iir_sample_cr * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_cr;
            correction += s.iir_sample_cr;
            s.iir_sample_dr = s.iir_sample_dr * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_dr;
            correction += s.iir_sample_dr;
            s.iir_sample_er = s.iir_sample_er * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_er;
            correction += s.iir_sample_er;
            s.iir_sample_fr = s.iir_sample_fr * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_fr;
            correction += s.iir_sample_fr;
            s.iir_sample_gr = s.iir_sample_gr * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_gr;
            correction += s.iir_sample_gr;
            s.iir_sample_hr = s.iir_sample_hr * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_hr;
            correction += s.iir_sample_hr;
            s.iir_sample_ir = s.iir_sample_ir * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_ir;
            correction += s.iir_sample_ir;
            s.iir_sample_jr = s.iir_sample_jr * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_jr;
            correction += s.iir_sample_jr;
            s.iir_sample_kr = s.iir_sample_kr * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_kr;
            correction += s.iir_sample_kr;
            s.iir_sample_lr = s.iir_sample_lr * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_lr;
            correction += s.iir_sample_lr;
            s.iir_sample_mr = s.iir_sample_mr * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_mr;
            correction += s.iir_sample_mr;
            s.iir_sample_nr = s.iir_sample_nr * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_nr;
            correction += s.iir_sample_nr;
            s.iir_sample_or = s.iir_sample_or * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_or;
            correction += s.iir_sample_or;
            s.iir_sample_pr = s.iir_sample_pr * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_pr;
            correction += s.iir_sample_pr;
            s.iir_sample_qr = s.iir_sample_qr * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_qr;
            correction += s.iir_sample_qr;
            s.iir_sample_rr = s.iir_sample_rr * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_rr;
            correction += s.iir_sample_rr;
            s.iir_sample_sr = s.iir_sample_sr * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_sr;
            correction += s.iir_sample_sr;
            s.iir_sample_tr = s.iir_sample_tr * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_tr;
            correction += s.iir_sample_tr;
            s.iir_sample_ur = s.iir_sample_ur * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_ur;
            correction += s.iir_sample_ur;
            s.iir_sample_vr = s.iir_sample_vr * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_vr;
            correction += s.iir_sample_vr;
            s.iir_sample_wr = s.iir_sample_wr * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_wr;
            correction += s.iir_sample_wr;
            s.iir_sample_xr = s.iir_sample_xr * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_xr;
            correction += s.iir_sample_xr;
            s.iir_sample_yr = s.iir_sample_yr * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_yr;
            correction += s.iir_sample_yr;
            s.iir_sample_zr = s.iir_sample_zr * alt_hpamount + temp_sample * iir_hpamount;
            temp_sample -= s.iir_sample_zr;
            correction += s.iir_sample_zr;
            head_bump_r -= correction;
            head_bump_r *= 1.0 - wreck / 2.0;
            bridgerectifier = head_bump_l.abs();
            if bridgerectifier > 1.57079633 {
                bridgerectifier = 1.57079633;
            }
            bridgerectifier = bridgerectifier.sin();
            if head_bump_l > 0.0 {
                head_bump_l = bridgerectifier;
            } else {
                head_bump_l = -bridgerectifier;
            }
            bridgerectifier = head_bump_r.abs();
            if bridgerectifier > 1.57079633 {
                bridgerectifier = 1.57079633;
            }
            bridgerectifier = bridgerectifier.sin();
            if head_bump_r > 0.0 {
                head_bump_r = bridgerectifier;
            } else {
                head_bump_r = -bridgerectifier;
            }
            input_sample_l *= 1.0 - wreck;
            input_sample_l += head_bump_l * wreck;
            input_sample_r *= 1.0 - wreck;
            input_sample_r += head_bump_r * wreck;
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
